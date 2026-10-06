//! Dependency DAG, bounded in-task futures, and process-wide resource leases.
use crate::{AgentError, AgentEvent, ApprovalPolicy, event::fetch_diagnostics, tool_activity};
use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use model::{Message, ToolCall};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
use tool::{ResourceAccess, Tool, ToolError, ToolOutput, ToolRegistry};

pub(super) struct Job {
    id: String,
    name: String,
    tool: Option<Arc<dyn Tool>>,
    input: Value,
    dependencies: Vec<usize>,
    input_dependencies: Vec<usize>,
    resources: Vec<ResourceAccess>,
    /// Index of an earlier call in the same round whose result this call
    /// reuses instead of repeating an identical read-only traversal.
    reuse: Option<usize>,
}

/// Advertise typed result references as part of the tool protocol. Scheduling
/// decisions still depend on declared effects and the runtime DAG, not prose.
pub(super) fn input_schema(mut schema: Value) -> Value {
    fn references(schema: &mut Value) {
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            for property in properties.values_mut() {
                references(property);
                let original = property.take();
                *property = serde_json::json!({"anyOf":[original, {
                    "type":"object","properties":{"$tool_result":{"type":"string"},"pointer":{"type":"string"}},
                    "required":["$tool_result"],"additionalProperties":false
                }]});
            }
        }
        if let Some(items) = schema.get_mut("items") {
            references(items);
            let original = items.take();
            *items = serde_json::json!({"anyOf":[original, {
                "type":"object","properties":{"$tool_result":{"type":"string"},"pointer":{"type":"string"}},
                "required":["$tool_result"],"additionalProperties":false
            }]});
        }
    }
    references(&mut schema);
    if schema["type"] == "object" && schema.get("properties").is_none() {
        schema["properties"] = serde_json::json!({});
    }
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert("_ax_execution".into(), serde_json::json!({"type":"object","properties":{"goal_id":{"type":"string"},"step":{"type":"string"},"expected_output":{"type":"string"},"scope":{"type":"array","items":{"type":"string"}}},"required":["goal_id","step","expected_output","scope"],"additionalProperties":false,"description":"Bind a step to the original goal ID, output and directories. Runtime enforces resource scope. Any observation permits retry, replan or a new step."}));
        properties.insert("_ax_observe".into(), serde_json::json!({"type":"string","description":"JSON pointer to a true boolean in the actual result verifying the expected output; observation alone is not progress."}));
        properties.insert("_ax_depends_on".into(), serde_json::json!({"type":"array","items":{"type":"string"},"description":"Tool call IDs whose successful completion is required by this call."}));
    }
    schema
}

pub(super) fn prepare(calls: &[ToolCall], tools: &ToolRegistry) -> Result<Vec<Job>, AgentError> {
    let ids: HashMap<_, _> = calls
        .iter()
        .enumerate()
        .map(|(index, call)| (call.id.as_str(), index))
        .collect();
    if ids.len() != calls.len() {
        return Err(invalid("duplicate tool_call_id in one round"));
    }
    let mut jobs = Vec::new();
    for call in calls {
        let mut input: Value =
            serde_json::from_str(&call.function.arguments).map_err(|source| {
                AgentError::InvalidToolArguments {
                    tool: call.function.name.clone(),
                    source,
                }
            })?;
        let mut dependencies = Vec::new();
        if let Some(object) = input.as_object_mut()
            && let Some(explicit) = object.remove("_ax_depends_on")
        {
            let Some(array) = explicit.as_array() else {
                return Err(invalid("_ax_depends_on must be an array of tool call IDs"));
            };
            for dependency in array {
                dependencies.push(
                    dependency
                        .as_str()
                        .ok_or_else(|| invalid("dependency ID must be a string"))?
                        .to_owned(),
                );
            }
        }
        collect_references(&input, &mut dependencies)?;
        let tool = tools.get(&call.function.name);
        // Unresolved paths/effects are conservative until the producing calls
        // finish. Execution reacquires concrete resources after substitution.
        let resources = if dependencies.is_empty() {
            tool.as_ref().map_or_else(
                || vec![ResourceAccess::exclusive()],
                |tool| tool.resources(&input),
            )
        } else {
            vec![ResourceAccess::exclusive()]
        };
        let dependencies = dependencies
            .iter()
            .map(|id| {
                ids.get(id.as_str())
                    .copied()
                    .ok_or_else(|| invalid(&format!("unknown tool result dependency: {id}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        jobs.push(Job {
            id: call.id.clone(),
            name: call.function.name.clone(),
            tool,
            input,
            input_dependencies: dependencies.clone(),
            dependencies,
            resources,
            reuse: None,
        });
    }
    check_acyclic(&jobs)?;
    // One round never walks the same tree twice for the same request: an
    // identical read-only discovery call is ordered after the first and reuses
    // its result. This is state reuse only — it never chooses which tool the
    // model should have called, and it never narrows a scope.
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut duplicates = Vec::new();
    for (index, job) in jobs.iter().enumerate() {
        let discovery = job
            .tool
            .as_ref()
            .is_some_and(|tool| tool.recursive_search())
            && !job.resources.iter().any(|access| access.write);
        if !discovery {
            continue;
        }
        let key = (job.name.clone(), dedup_key(&job.input));
        match seen.get(&key).copied() {
            Some(first) => duplicates.push((index, first)),
            None => {
                seen.insert(key, index);
            }
        }
    }
    for (index, first) in duplicates {
        jobs[index].reuse = Some(first);
        jobs[index].dependencies.push(first);
    }
    check_acyclic(&jobs)?;
    // Preserve model order for conflicting effects unless an explicit data
    // dependency already requires the reverse order.
    for left in 0..jobs.len() {
        for right in left + 1..jobs.len() {
            if conflict(&jobs[left].resources, &jobs[right].resources)
                && !depends_on(&jobs, left, right)
            {
                jobs[right].dependencies.push(left);
            }
        }
    }
    check_acyclic(&jobs)?;
    Ok(jobs)
}

fn invalid(message: &str) -> AgentError {
    ToolError::InvalidInput(message.into()).into()
}

/// Comparable form of a call's arguments. Runtime-only metadata is dropped so
/// two calls that differ only in orchestration notes still compare equal.
fn dedup_key(input: &Value) -> String {
    let mut input = input.clone();
    if let Some(object) = input.as_object_mut() {
        object.remove("_ax_observe");
        object.remove("_ax_execution");
    }
    input.to_string()
}

fn collect_references(input: &Value, dependencies: &mut Vec<String>) -> Result<(), AgentError> {
    match input {
        Value::Object(object) if object.contains_key("$tool_result") => {
            let id = object["$tool_result"]
                .as_str()
                .ok_or_else(|| invalid("$tool_result must name a tool_call_id"))?;
            if object
                .keys()
                .any(|key| key != "$tool_result" && key != "pointer")
                || object
                    .get("pointer")
                    .is_some_and(|pointer| !pointer.is_string())
            {
                return Err(invalid("invalid tool result reference"));
            }
            dependencies.push(id.into());
        }
        Value::Object(object) => {
            for value in object.values() {
                collect_references(value, dependencies)?;
            }
        }
        Value::Array(array) => {
            for value in array {
                collect_references(value, dependencies)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn depends_on(jobs: &[Job], job: usize, dependency: usize) -> bool {
    let mut pending = jobs[job].dependencies.clone();
    let mut visited = vec![false; jobs.len()];
    while let Some(index) = pending.pop() {
        if index == dependency {
            return true;
        }
        if !visited[index] {
            visited[index] = true;
            pending.extend(&jobs[index].dependencies);
        }
    }
    false
}

fn check_acyclic(jobs: &[Job]) -> Result<(), AgentError> {
    for index in 0..jobs.len() {
        if depends_on(jobs, index, index) {
            return Err(invalid("cyclic tool call dependencies"));
        }
    }
    Ok(())
}

fn conflict(left: &[ResourceAccess], right: &[ResourceAccess]) -> bool {
    left.iter()
        .any(|left| right.iter().any(|right| left.conflicts(right)))
}

fn resolve(input: &Value, jobs: &[Job], outputs: &[Option<Message>]) -> Result<Value, ToolError> {
    match input {
        Value::Object(object) if object.contains_key("$tool_result") => {
            let id = object["$tool_result"].as_str().unwrap_or_default();
            let output = jobs
                .iter()
                .position(|job| job.id == id)
                .and_then(|index| outputs[index].as_ref())
                .ok_or_else(|| ToolError::InvalidInput(format!("unresolved tool result: {id}")))?;
            let raw = serde_json::from_str::<tool::ToolResult>(&output.content)
                .map_or_else(|_| output.content.clone(), |result| result.raw_output);
            let value = serde_json::from_str(&raw).unwrap_or(Value::String(raw));
            if let Some(pointer) = object.get("pointer").and_then(Value::as_str) {
                return value.pointer(pointer).cloned().ok_or_else(|| {
                    ToolError::InvalidInput(format!(
                        "tool result {id} has no JSON pointer {pointer}"
                    ))
                });
            }
            Ok(value)
        }
        Value::Object(object) => Ok(Value::Object(
            object
                .iter()
                .map(|(key, value)| Ok((key.clone(), resolve(value, jobs, outputs)?)))
                .collect::<Result<_, ToolError>>()?,
        )),
        Value::Array(array) => Ok(Value::Array(
            array
                .iter()
                .map(|value| resolve(value, jobs, outputs))
                .collect::<Result<_, _>>()?,
        )),
        value => Ok(value.clone()),
    }
}

#[allow(clippy::too_many_arguments)] // The scheduler accepts runtime boundaries explicitly.
pub(super) async fn run<F, H>(
    jobs: Vec<Job>,
    approval: Arc<dyn ApprovalPolicy>,
    profiles: Vec<tool::PermissionProfile>,
    concurrency: usize,
    timeout_secs: u64,
    emit: &Mutex<F>,
    execution: Option<Arc<Mutex<crate::ExecutionState>>>,
    mut completed: H,
) -> Result<Vec<Message>, AgentError>
where
    F: FnMut(AgentEvent) + Send,
    H: FnMut(Message, &str, &Value) -> Result<(), AgentError>,
{
    let mut outputs: Vec<Option<Message>> = vec![None; jobs.len()];
    let mut succeeded = vec![false; jobs.len()];
    let mut started = vec![false; jobs.len()];
    let approval_gate = Arc::new(tokio::sync::Mutex::new(()));
    let mut running: FuturesUnordered<BoxFuture<'_, (usize, bool, Message, Value)>> =
        FuturesUnordered::new();
    loop {
        for index in 0..jobs.len() {
            let job = &jobs[index];
            let independent = |i: usize| {
                jobs[i]
                    .tool
                    .as_ref()
                    .is_some_and(|tool| tool.independent_remote_execution())
            };
            let bounded_active = (0..jobs.len())
                .filter(|i| started[*i] && outputs[*i].is_none() && !independent(*i))
                .count();
            if !independent(index) && bounded_active >= concurrency.max(1) {
                continue;
            }
            if started[index]
                || !job
                    .dependencies
                    .iter()
                    .all(|index| outputs[*index].is_some())
            {
                continue;
            }
            started[index] = true;
            // An identical discovery call that already succeeded reuses that
            // result instead of traversing the same scope again. A failed
            // first call is not reused: the duplicate still runs normally.
            if let Some(first) = jobs[index].reuse
                && succeeded[first]
                && let Some(output) = outputs[first].clone()
            {
                let message = emit_reuse(emit, &jobs[index], &jobs[first], &output);
                succeeded[index] = true;
                outputs[index] = Some(message.clone());
                completed(message, &jobs[index].name, &jobs[index].input)?;
                continue;
            }
            let input = if job
                .input_dependencies
                .iter()
                .any(|index| !succeeded[*index])
            {
                Err(ToolError::Execution(
                    "tool input dependency failed; operation was not executed".into(),
                ))
            } else {
                resolve(&job.input, &jobs, &outputs)
            };
            let profiles = profiles.clone();
            let approval = Arc::clone(&approval);
            let approval_gate = Arc::clone(&approval_gate);
            let execution = execution.clone();
            running.push(Box::pin(async move {
                let proposed_input = input.as_ref().unwrap_or(&job.input).clone();
                let input = input.and_then(|input| {
                    if let (Some(state), Some(tool)) = (&execution, &job.tool) {
                        state.lock().unwrap().prepare(tool.as_ref(), input)
                    } else {
                        Ok(input)
                    }
                });
                let actual_input = input.as_ref().unwrap_or(&proposed_input).clone();
                let result = execute(
                    job,
                    input,
                    approval,
                    profiles,
                    approval_gate,
                    timeout_secs,
                    emit,
                )
                .await;
                let success = result.is_ok();
                let envelope = envelope(&result);
                (emit.lock().unwrap())(AgentEvent::ToolFinished {
                    id: job.id.clone(),
                    name: job.name.clone(),
                    success,
                    diagnostics: fetch_diagnostics(&job.name, &result),
                    result: envelope,
                });
                (index, success, message(&job.id, result), actual_input)
            }));
        }
        let Some((index, success, message, input)) = running.next().await else {
            break;
        };
        succeeded[index] = success;
        outputs[index] = Some(message.clone());
        completed(message, &jobs[index].name, &input)?;
    }
    Ok(outputs.into_iter().flatten().collect())
}

async fn execute<F: FnMut(AgentEvent) + Send>(
    job: &Job,
    input: Result<Value, ToolError>,
    approval: Arc<dyn ApprovalPolicy>,
    profiles: Vec<tool::PermissionProfile>,
    approval_gate: Arc<tokio::sync::Mutex<()>>,
    timeout_secs: u64,
    emit: &Mutex<F>,
) -> Result<ToolOutput, ToolError> {
    (emit.lock().unwrap())(AgentEvent::ToolStarted {
        id: job.id.clone(),
        name: job.name.clone(),
        detail: tool_activity(&job.name, input.as_ref().unwrap_or(&job.input)),
        input: input.as_ref().unwrap_or(&job.input).clone(),
    });
    let mut input = input?;
    if let Some(object) = input.as_object_mut() {
        object.remove("_ax_observe");
        object.remove("_ax_execution");
    }
    let tool = job
        .tool
        .as_ref()
        .ok_or_else(|| ToolError::Unknown(job.name.clone()))?;
    let permission = tool.permission(&input);
    let decisions: Vec<_> = profiles
        .iter()
        .map(|p| {
            p.decision(
                &job.name,
                &input,
                permission.capability,
                &tool.resources(&input),
            )
        })
        .collect();
    let outcome = tool::resolve_profiles(&decisions);
    if outcome == tool::ProfileDecision::Deny
        || approval.capability_decision(permission.capability)
            == Some(tool::PermissionDecision::Deny)
    {
        return Err(ToolError::PermissionDenied(job.name.clone()));
    }
    let approved = {
        let _guard = approval_gate.lock().await;
        match outcome {
            tool::ProfileDecision::Ask => approval.ask(&job.name, &input, permission).await,
            tool::ProfileDecision::Allowed => true,
            tool::ProfileDecision::Deny | tool::ProfileDecision::Unspecified => {
                approval.approve(&job.name, &input, permission).await
            }
        }
    };
    if !approved {
        return Err(ToolError::PermissionDenied(job.name.clone()));
    }
    let _lease = locks().acquire(tool.resources(&input)).await;
    let _timer = tool::telemetry::Timer::new(format!("tool.{}", tool.name()));
    if timeout_secs == 0 {
        tool.execute_output_constrained(input, &profiles).await
    } else {
        tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            tool.execute_output_constrained(input, &profiles),
        )
        .await
        .unwrap_or_else(|_| Err(ToolError::Execution("tool timeout".into())))
    }
}

/// Emit the reuse of an earlier discovery result and return the tool response
/// for the duplicate call. The envelope shape is preserved so the model sees an
/// ordinary success.
fn emit_reuse<F: FnMut(AgentEvent) + Send>(
    emit: &Mutex<F>,
    duplicate: &Job,
    first: &Job,
    output: &Message,
) -> Message {
    let (message, result) = reused_message(&duplicate.id, &first.id, output);
    (emit.lock().unwrap())(AgentEvent::ToolStarted {
        id: duplicate.id.clone(),
        name: duplicate.name.clone(),
        detail: format!("reusing result of {}", first.id),
        input: duplicate.input.clone(),
    });
    (emit.lock().unwrap())(AgentEvent::ToolFinished {
        id: duplicate.id.clone(),
        name: duplicate.name.clone(),
        success: true,
        diagnostics: Vec::new(),
        result,
    });
    message
}

/// Reuse a completed discovery result for an identical call in the same round.
fn reused_message(id: &str, first: &str, output: &Message) -> (Message, tool::ToolResult) {
    let mut result = serde_json::from_str::<tool::ToolResult>(&output.content)
        .unwrap_or_else(|_| tool::ToolResult::new(true, output.content.clone()));
    result.summary = format!(
        "Identical to tool call {first}; its result was reused instead of re-scanning the same scope"
    );
    let message = Message::tool(id, serde_json::to_string(&result).unwrap_or_default());
    (message, result)
}

fn message(id: &str, result: Result<ToolOutput, ToolError>) -> Message {
    let envelope = envelope(&result);
    match result.unwrap_or_else(|error| ToolOutput::Text(error.to_string())) {
        ToolOutput::Text(_) => {
            Message::tool(id, serde_json::to_string(&envelope).unwrap_or_default())
        }
        ToolOutput::Image {
            description,
            media_type,
            data,
        } => {
            let mut message = Message::tool(id, description.clone());
            message.parts = vec![
                model::ContentPart::Text { text: description },
                model::ContentPart::Image { media_type, data },
            ];
            message
        }
    }
}

fn envelope(result: &Result<ToolOutput, ToolError>) -> tool::ToolResult {
    let raw = match result {
        Ok(
            ToolOutput::Text(text)
            | ToolOutput::Image {
                description: text, ..
            },
        )
        | Err(ToolError::Execution(text) | ToolError::InvalidInput(text)) => text.clone(),
        Err(error) => error.to_string(),
    };
    let mut envelope = tool::ToolResult::new(result.is_ok(), raw);
    if let Err(ToolError::GlobalBlocked(reason)) = result {
        envelope.global_blocker = Some(reason.clone());
    }
    envelope
}

#[derive(Default)]
struct ResourceLocks {
    active: Mutex<Vec<Arc<Vec<ResourceAccess>>>>,
    changed: tokio::sync::Notify,
}

fn locks() -> &'static ResourceLocks {
    static LOCKS: OnceLock<ResourceLocks> = OnceLock::new();
    LOCKS.get_or_init(ResourceLocks::default)
}

impl ResourceLocks {
    async fn acquire(&self, resources: Vec<ResourceAccess>) -> Lease<'_> {
        let resources = Arc::new(resources);
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut active = self
                    .active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !active.iter().any(|held| conflict(held, &resources)) {
                    active.push(Arc::clone(&resources));
                    return Lease {
                        locks: self,
                        resources,
                    };
                }
            }
            notified.await;
        }
    }
}

struct Lease<'a> {
    locks: &'a ResourceLocks,
    resources: Arc<Vec<ResourceAccess>>,
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.locks
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|held| !Arc::ptr_eq(held, &self.resources));
        self.locks.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use model::FunctionCall;
    use serde_json::json;
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    use tool::{Capability, Resource, SafetyLevel};

    static HANG_STARTED: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(0);

    #[derive(Default)]
    struct State {
        active: AtomicUsize,
        peak: AtomicUsize,
        executions: AtomicUsize,
        trace: Mutex<Vec<String>>,
    }
    struct Active(Arc<State>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct Probe(Arc<State>);
    #[async_trait]
    impl Tool for Probe {
        fn name(&self) -> &'static str {
            "probe"
        }
        fn description(&self) -> &'static str {
            "scheduler test"
        }
        fn input_schema(&self) -> Value {
            json!({"type":"object"})
        }
        fn safety(&self, _: &Value) -> SafetyLevel {
            SafetyLevel::Safe
        }
        fn capability(&self, _: &Value) -> Capability {
            Capability::Network
        }
        fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
            let resource = input.get("path").and_then(Value::as_str).map_or_else(
                || Resource::Named(input["resource"].as_str().unwrap_or("fixture").into()),
                Resource::path,
            );
            vec![if input["write"] == true {
                ResourceAccess::write(resource)
            } else {
                ResourceAccess::read(resource)
            }]
        }
        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            let id = input["label"].as_str().unwrap();
            let active = self.0.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.0.peak.fetch_max(active, Ordering::SeqCst);
            let _active = Active(Arc::clone(&self.0));
            self.0.trace.lock().unwrap().push(format!("start:{id}"));
            if id == "hang" {
                HANG_STARTED.add_permits(1);
            }
            tokio::time::sleep(Duration::from_millis(input["delay"].as_u64().unwrap_or(20))).await;
            self.0.trace.lock().unwrap().push(format!("end:{id}"));
            if input["fail"] == true {
                return Err(ToolError::Execution("fixture failed".into()));
            }
            Ok(json!({"value":input["value"],"label":id}).to_string())
        }
    }

    struct Undeclared(Probe);
    #[async_trait]
    impl Tool for Undeclared {
        fn name(&self) -> &'static str {
            "unknown-effects"
        }
        fn description(&self) -> &'static str {
            "undeclared effects fixture"
        }
        fn input_schema(&self) -> Value {
            json!({"type":"object"})
        }
        fn safety(&self, _: &Value) -> SafetyLevel {
            SafetyLevel::Safe
        }
        fn capability(&self, _: &Value) -> Capability {
            Capability::Network
        }
        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            self.0.execute(input).await
        }
    }
    /// The shipped discovery tool with concurrency instrumentation, so these
    /// tests exercise the real resource declaration rather than a stand-in.
    struct CountingSearch {
        inner: tool::SearchTool,
        state: Arc<State>,
    }
    #[async_trait]
    impl Tool for CountingSearch {
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn description(&self) -> &str {
            self.inner.description()
        }
        fn input_schema(&self) -> Value {
            self.inner.input_schema()
        }
        fn safety(&self, input: &Value) -> SafetyLevel {
            self.inner.safety(input)
        }
        fn capability(&self, input: &Value) -> Capability {
            self.inner.capability(input)
        }
        fn recursive_search(&self) -> bool {
            self.inner.recursive_search()
        }
        fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
            self.inner.resources(input)
        }
        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            self.state.executions.fetch_add(1, Ordering::SeqCst);
            let active = self.state.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.state.peak.fetch_max(active, Ordering::SeqCst);
            let _active = Active(Arc::clone(&self.state));
            tokio::time::sleep(Duration::from_millis(40)).await;
            self.inner.execute(input).await
        }
    }
    /// Any built-in tool with concurrency instrumentation, so the scheduler
    /// tests exercise the real resource declaration rather than a stand-in.
    struct CountingTool {
        inner: std::sync::Arc<dyn Tool>,
        state: Arc<State>,
    }
    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            self.inner.name()
        }
        fn description(&self) -> &str {
            self.inner.description()
        }
        fn input_schema(&self) -> Value {
            self.inner.input_schema()
        }
        fn safety(&self, input: &Value) -> SafetyLevel {
            self.inner.safety(input)
        }
        fn capability(&self, input: &Value) -> Capability {
            self.inner.capability(input)
        }
        fn recursive_search(&self) -> bool {
            self.inner.recursive_search()
        }
        fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
            self.inner.resources(input)
        }
        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            self.state.executions.fetch_add(1, Ordering::SeqCst);
            let active = self.state.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.state.peak.fetch_max(active, Ordering::SeqCst);
            let _active = Active(Arc::clone(&self.state));
            tokio::time::sleep(Duration::from_millis(40)).await;
            self.inner.execute(input).await
        }
    }

    fn search_fixture() -> (ToolRegistry, Arc<State>, std::path::PathBuf) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let state = Arc::new(State::default());
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(CountingSearch {
            inner: tool::SearchTool::default(),
            state: Arc::clone(&state),
        });
        let root = std::env::temp_dir().join(format!(
            "ax-sched-search-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("input.txt"), "needle\n").unwrap();
        (tools, state, root)
    }
    fn call(id: &str, input: Value) -> ToolCall {
        named(id, "probe", input)
    }
    fn named(id: &str, name: &str, input: Value) -> ToolCall {
        let arguments = input.to_string();
        drop(input);
        ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments,
            },
        }
    }
    fn fixture() -> (ToolRegistry, Arc<State>) {
        let state = Arc::new(State::default());
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(Probe(Arc::clone(&state)));
        (tools, state)
    }
    async fn schedule(
        calls: &[ToolCall],
        tools: &ToolRegistry,
        concurrency: usize,
    ) -> Vec<Message> {
        run(
            prepare(calls, tools).unwrap(),
            Arc::new(super::super::AllowAll),
            vec![],
            concurrency,
            0,
            &Mutex::new(|_| {}),
            None,
            |_, _, _| Ok(()),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn runtime_rules_stop_side_effects_even_with_allow_all_and_resolved_arguments() {
        let (tools, state) = fixture();
        for decision in [
            tool::PermissionDecision::Deny,
            tool::PermissionDecision::Ask,
        ] {
            let profile = tool::PermissionProfile {
                rules: vec![tool::PermissionRule {
                    decision,
                    matcher: tool::RuleMatcher::ToolParameter {
                        tool: "probe".into(),
                        pointer: "/label".into(),
                        pattern: "blocked".into(),
                    },
                }],
                ..Default::default()
            };
            let calls = vec![call(
                "blocked",
                json!({"label":"blocked","resource":"r","write":true}),
            )];
            let results = run(
                prepare(&calls, &tools).unwrap(),
                Arc::new(super::super::AllowAll),
                vec![profile],
                1,
                0,
                &Mutex::new(|_| {}),
                None,
                |_, _, _| Ok(()),
            )
            .await
            .unwrap();
            assert!(results[0].content.contains("permission denied"));
            assert!(state.trace.lock().unwrap().is_empty());
        }
        let profile = tool::PermissionProfile {
            rules: vec![tool::PermissionRule {
                decision: tool::PermissionDecision::Deny,
                matcher: tool::RuleMatcher::ToolParameter {
                    tool: "probe".into(),
                    pointer: "/label".into(),
                    pattern: "secret".into(),
                },
            }],
            ..Default::default()
        };
        let calls = vec![
            call(
                "source",
                json!({"label":"source","resource":"a","value":"secret"}),
            ),
            call(
                "dependent",
                json!({"label":{"$tool_result":"source","pointer":"/value"},"resource":"b"}),
            ),
        ];
        // A dependent label is resolved only after the first tool result; policy checks the resolved input.
        run(
            prepare(&calls, &tools).unwrap(),
            Arc::new(super::super::AllowAll),
            vec![profile],
            2,
            0,
            &Mutex::new(|_| {}),
            None,
            |_, _, _| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(
            *state.trace.lock().unwrap(),
            vec!["start:source", "end:source"]
        );
    }

    #[tokio::test]
    async fn independent_calls_are_bounded_and_keep_original_ids_despite_completion_order() {
        let (tools, state) = fixture();
        let calls: Vec<_> = (0..6).map(|i| call(&i.to_string(), json!({"label":i.to_string(),"resource":format!("r{i}"),"write":true,"delay":if i == 0 {100} else {20}}))).collect();
        let results = schedule(&calls, &tools, 3).await;
        assert_eq!(state.peak.load(Ordering::SeqCst), 3);
        assert_eq!(
            results
                .iter()
                .map(|message| message.tool_call_id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["0", "1", "2", "3", "4", "5"]
        );
        let trace = state.trace.lock().unwrap();
        assert!(
            trace.iter().position(|event| event == "end:1")
                < trace.iter().position(|event| event == "end:0")
        );
    }

    #[tokio::test]
    async fn conflicting_writes_and_reads_serialize_but_shared_reads_overlap() {
        let (tools, state) = fixture();
        schedule(
            &[
                call("a", json!({"label":"a","resource":"shared","write":true})),
                call("b", json!({"label":"b","resource":"shared"})),
                call("c", json!({"label":"c","resource":"shared","write":true})),
            ],
            &tools,
            4,
        )
        .await;
        assert_eq!(
            *state.trace.lock().unwrap(),
            ["start:a", "end:a", "start:b", "end:b", "start:c", "end:c"]
        );
        let (tools, state) = fixture();
        schedule(
            &[
                call("a", json!({"label":"a"})),
                call("b", json!({"label":"b"})),
            ],
            &tools,
            4,
        )
        .await;
        assert_eq!(state.peak.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn independent_searches_run_concurrently_in_one_round() {
        let (tools, state, root) = search_fixture();
        let calls = vec![
            named("first", "search", json!({"query":"needle","path":root})),
            named(
                "second",
                "search",
                json!({"query":"needle-elsewhere","path":root}),
            ),
            named("third", "search", json!({"query":"absent","path":root})),
        ];
        // Read-only discovery declares read access, so no call depends on
        // another and none of them takes the global write lock.
        let jobs = prepare(&calls, &tools).unwrap();
        assert!(jobs.iter().all(|job| job.dependencies.is_empty()));
        assert!(jobs.iter().all(|job| {
            job.tool.as_ref().is_some_and(|tool| {
                tool.resources(&job.input)
                    .iter()
                    .all(|access| !access.write && access.resource != Resource::All)
            })
        }));

        let results = schedule(&calls, &tools, 4).await;
        assert_eq!(state.peak.load(Ordering::SeqCst), 3);
        assert_eq!(state.executions.load(Ordering::SeqCst), 3);
        let found: Value = serde_json::from_str(
            &serde_json::from_str::<tool::ToolResult>(&results[0].content)
                .unwrap()
                .raw_output,
        )
        .unwrap();
        assert_eq!(found["matches"][0]["path"], "input.txt");
        let empty = serde_json::from_str::<tool::ToolResult>(&results[1].content).unwrap();
        assert!(empty.raw_output.contains("\"matches\":[]"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn an_identical_discovery_call_reuses_the_first_result_instead_of_rescanning() {
        let (tools, state, root) = search_fixture();
        let calls = vec![
            named("first", "search", json!({"query":"needle","path":root})),
            named("again", "search", json!({"query":"needle","path":root})),
        ];
        let results = schedule(&calls, &tools, 4).await;
        // One traversal, two protocol responses: the duplicate never re-walks
        // the workspace.
        assert_eq!(state.executions.load(Ordering::SeqCst), 1);
        assert_eq!(results.len(), 2);
        assert_eq!(results[1].tool_call_id.as_deref(), Some("again"));
        let first = serde_json::from_str::<tool::ToolResult>(&results[0].content).unwrap();
        let reused = serde_json::from_str::<tool::ToolResult>(&results[1].content).unwrap();
        assert_eq!(reused.status, "success");
        assert_eq!(reused.raw_output, first.raw_output);
        assert!(reused.summary.contains("reused"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn permissions_do_not_grant_concurrency_to_undeclared_effects() {
        let state = Arc::new(State::default());
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(Undeclared(Probe(state.clone())));
        let mut calls = [
            call("a", json!({"label":"a","resource":"one"})),
            call("b", json!({"label":"b","resource":"two"})),
        ];
        for call in &mut calls {
            call.function.name = "unknown-effects".into();
        }
        schedule(&calls, &tools, 4).await;
        assert_eq!(state.peak.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tool_result_references_wait_and_substitute_typed_values_even_for_forward_ids() {
        let (tools, state) = fixture();
        let results = schedule(&[
            call("consumer", json!({"label":"consumer","value":{"$tool_result":"producer","pointer":"/value"}})),
            call("producer", json!({"label":"producer","value":42})),
        ], &tools, 4).await;
        assert_eq!(
            serde_json::from_str::<Value>(
                &serde_json::from_str::<tool::ToolResult>(&results[0].content)
                    .unwrap()
                    .raw_output
            )
            .unwrap()["value"],
            42
        );
        assert_eq!(
            *state.trace.lock().unwrap(),
            [
                "start:producer",
                "end:producer",
                "start:consumer",
                "end:consumer"
            ]
        );
    }

    #[tokio::test]
    async fn failed_data_dependencies_block_consumers_but_failed_writes_do_not_block_unrelated_effects()
     {
        let (tools, state) = fixture();
        let results = schedule(
            &[
                call("failed", json!({"label":"failed","fail":true,"write":true})),
                call("next", json!({"label":"next","write":true})),
                call(
                    "dependent",
                    json!({"label":"dependent","_ax_depends_on":["failed"]}),
                ),
            ],
            &tools,
            4,
        )
        .await;
        assert!(results[2].content.contains("dependency failed"));
        assert_eq!(
            *state.trace.lock().unwrap(),
            ["start:failed", "end:failed", "start:next", "end:next"]
        );
    }

    #[tokio::test]
    async fn resource_locks_work_across_rounds_and_release_on_cancellation() {
        let (tools, state) = fixture();
        let first = [call(
            "a",
            json!({"label":"a","resource":"cross-round","write":true,"delay":40}),
        )];
        let second = [call(
            "b",
            json!({"label":"b","resource":"cross-round","write":true,"delay":40}),
        )];
        tokio::join!(schedule(&first, &tools, 4), schedule(&second, &tools, 4));
        assert_eq!(state.peak.load(Ordering::SeqCst), 1);
        let hanging = [call(
            "hang",
            json!({"label":"hang","resource":"cross-round","write":true,"delay":1000}),
        )];
        {
            let hang_future = schedule(&hanging, &tools, 4);
            tokio::pin!(hang_future);
            tokio::select! {
                _ = &mut hang_future => panic!("hanging call must not complete"),
                _started = HANG_STARTED.acquire() => {}
            }
        }
        assert_eq!(state.active.load(Ordering::SeqCst), 0);
        // The released lease must be acquirable again.
        schedule(&second, &tools, 4).await;
    }

    #[tokio::test]
    async fn three_independent_read_only_tools_run_together() {
        let root = std::env::temp_dir().join(format!(
            "ax-sched-mixed-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("input.txt"), "needle\n").unwrap();
        let state = Arc::new(State::default());
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        let counting = |inner: std::sync::Arc<dyn Tool>| CountingTool {
            inner,
            state: Arc::clone(&state),
        };
        tools.register_arc(std::sync::Arc::new(counting(std::sync::Arc::new(
            tool::SearchTool::default(),
        ))));
        tools.register_arc(std::sync::Arc::new(counting(std::sync::Arc::new(
            tool::FindFilesTool::new(root.clone()),
        ))));
        tools.register_arc(std::sync::Arc::new(counting(std::sync::Arc::new(
            tool::FilesystemTool,
        ))));
        let calls = vec![
            named("search", "search", json!({"query":"needle","path":root})),
            named("find", "find_files", json!({"pattern":"*.txt","path":root})),
            named(
                "read",
                "filesystem",
                json!({"operation":"read","path":root.join("input.txt")}),
            ),
        ];
        let results = schedule(&calls, &tools, 4).await;
        assert_eq!(state.peak.load(Ordering::SeqCst), 3);
        assert_eq!(state.executions.load(Ordering::SeqCst), 3);
        assert!(
            results.iter().all(
                |result| serde_json::from_str::<tool::ToolResult>(&result.content)
                    .is_ok_and(|result| result.status == "success")
            ),
            "{results:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_read_only_shell_command_overlaps_with_other_reads() {
        let state = Arc::new(State::default());
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register_arc(std::sync::Arc::new(CountingTool {
            inner: std::sync::Arc::new(tool::ShellTool),
            state: Arc::clone(&state),
        }));
        tools.register_arc(std::sync::Arc::new(CountingTool {
            inner: std::sync::Arc::new(tool::SearchTool::default()),
            state: Arc::clone(&state),
        }));
        let calls = vec![
            named("echo", "shell", json!({"command":"echo scheduler"})),
            named(
                "search",
                "search",
                json!({"query":"needle","path":std::env::temp_dir()}),
            ),
        ];
        // `echo` is a known read-only invocation, so it is not serialized behind
        // an opaque global write and overlaps with an independent read.
        let results = schedule(&calls, &tools, 4).await;
        assert_eq!(state.peak.load(Ordering::SeqCst), 2);
        assert_eq!(state.executions.load(Ordering::SeqCst), 2);
        assert!(results[0].content.contains("scheduler"));
    }

    #[test]
    fn malformed_dependency_graphs_are_rejected_before_execution() {
        let (tools, _) = fixture();
        for calls in [
            vec![call("a", json!({"_ax_depends_on":["missing"]}))],
            vec![
                call("a", json!({"_ax_depends_on":["b"]})),
                call("b", json!({"_ax_depends_on":["a"]})),
            ],
            vec![call("same", json!({})), call("same", json!({}))],
        ] {
            assert!(prepare(&calls, &tools).is_err());
        }
    }

    #[tokio::test]
    async fn kernel_batches_parallel_results_in_call_order_and_checkpoints_completion_order() {
        use model::{ModelError, ModelProvider, ModelRequest, ModelResponse};
        struct Provider(AtomicUsize);
        #[async_trait]
        impl ModelProvider for Provider {
            fn name(&self) -> &'static str {
                "scheduler-fixture"
            }
            fn model_id(&self) -> &'static str {
                "fixture"
            }
            fn context_window(&self) -> usize {
                100_000
            }
            async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
                if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                    assert!(
                        request.tools[0].function.parameters["properties"]["_ax_depends_on"]
                            .is_object()
                    );
                    Ok(ModelResponse {
                        provider_metadata: None,
                        usage: None,
                        content: String::new(),
                        tool_calls: vec![
                            call("slow", json!({"label":"slow","delay":100})),
                            call("fast", json!({"label":"fast","delay":20})),
                        ],
                        finish_reason: None,
                    })
                } else {
                    let results: Vec<_> = request
                        .messages
                        .iter()
                        .filter_map(|message| message.tool_call_id.as_deref())
                        .collect();
                    assert_eq!(results, ["slow", "fast"]);
                    Ok(ModelResponse {
                        provider_metadata: None,
                        usage: None,
                        content: "done".into(),
                        tool_calls: Vec::new(),
                        finish_reason: None,
                    })
                }
            }
        }
        let (tools, state) = fixture();
        let mut kernel = super::super::AgentKernel::new(
            Arc::new(Provider(AtomicUsize::new(0))),
            tools,
            Arc::new(super::super::AllowAll),
        )
        .with_tool_concurrency(2);
        let mut saved = Vec::new();
        let mut events = Vec::new();
        assert_eq!(
            kernel
                .run_turn_checkpointed(
                    "test",
                    |event| events.push(event),
                    |messages| {
                        saved = messages.to_vec();
                        Ok(())
                    }
                )
                .await
                .unwrap(),
            "done"
        );
        assert_eq!(state.peak.load(Ordering::SeqCst), 2);
        assert_eq!(
            saved
                .iter()
                .filter_map(|message| message.tool_call_id.as_deref())
                .collect::<Vec<_>>(),
            ["fast", "slow"]
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    AgentEvent::ToolFinished { id, .. } => Some(id.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["fast", "slow"]
        );
    }
}
