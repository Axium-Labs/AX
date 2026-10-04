//! Parallel child dispatch.
//!
//! The task queue already decides *what* may run: `pending` becomes `ready`
//! once every dependency is `Completed`. This module is the missing half — it
//! takes the whole ready frontier and hands it to the existing
//! [`AgentSupervisor`], instead of promoting a single task per model round.
//!
//! Boundaries stay where they were:
//!
//! - the kernel decides permission, sandbox, resource conflicts, dependency,
//!   timeout/budget and cancellation;
//! - the model decides what to search, read, delegate and change.
//!
//! Concurrency is therefore never requested through a prompt. The runtime reads
//! the dependency graph and each task's declared resources and admits whatever
//! is provably independent.
//!
//! Nothing here spawns a detached task: children are futures owned by the turn,
//! so dropping the turn (cancel, timeout, process exit) drops every child with
//! it.

use std::collections::VecDeque;
use std::sync::Mutex;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use model::Message;

use crate::child::{ChildRun, ChildSaveSink, PreparedChild};
use crate::child_result::{self, ChildObserver, ChildResult, ChildStatus};
use crate::task_queue::{TaskQueue, TaskStatus};
use crate::{AgentError, AgentEvent, AgentKernel, AgentSupervisor, ExecutionState, QueueState};

/// One finished child, as the controller needs it.
pub(crate) type ChildSettlement = (usize, Option<ChildRun>, ChildResult, ExecutionState);

impl AgentKernel {
    /// Dispatch every currently-ready child, then keep admitting newly ready
    /// ones as others finish. Returns how many children were started.
    ///
    /// One failing or timing-out child never stops its siblings: its receipt is
    /// recorded and the frontier moves on.
    pub(crate) async fn execute_ready_children<F, H>(
        &mut self,
        emit: &Mutex<F>,
        checkpoint: &mut H,
    ) -> Result<usize, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let Some(host) = self.child_host.clone() else {
            return Ok(0);
        };
        let total = self
            .task_queue
            .as_ref()
            .map_or(0, |queue| queue.tasks.len());
        if total == 0 {
            return Ok(0);
        }
        // The supervisor owns bounded concurrency and child lifecycle.
        let supervisor = AgentSupervisor::bounded(self.child_concurrency);
        let limit = supervisor.max_concurrency();

        let mut started = vec![false; total];
        let mut reserved: Vec<(usize, Vec<tool::ResourceAccess>)> = Vec::new();
        let mut settled: VecDeque<ChildSettlement> = VecDeque::new();
        let mut running: FuturesUnordered<BoxFuture<'_, ChildSettlement>> = FuturesUnordered::new();
        let mut executed = 0usize;
        self.receipts_dirty = false;

        loop {
            let capacity = limit.saturating_sub(running.len() + settled.len());
            if capacity > 0 {
                let ready = self.select_ready_children(&started, &reserved, capacity);
                if !ready.is_empty() {
                    for &index in &ready {
                        started[index] = true;
                        let task = &mut self.task_queue.as_mut().unwrap().tasks[index];
                        // Persisted before dispatch: an interrupted task is
                        // never silently replaced on resume.
                        task.execution_started = true;
                        reserved.push((index, task.resources.clone()));
                    }
                    self.checkpoint_queue(checkpoint)?;
                    for outcome in self.prepare_children(&host, &ready).await {
                        match outcome {
                            Prepared::Ready(index, child) => {
                                executed += 1;
                                self.attach_child(
                                    index,
                                    *child,
                                    emit,
                                    checkpoint,
                                    &mut running,
                                    &mut settled,
                                )?;
                            }
                            Prepared::Failed(index, error) => {
                                reserved.retain(|(held, _)| *held != index);
                                let task_id = task_id_of(index);
                                let mut result = ChildResult::failed(
                                    format!(
                                        "{}:{task_id}:setup",
                                        self.goal_id.as_deref().unwrap_or_default()
                                    ),
                                    task_id,
                                    ChildStatus::Failed,
                                    error.to_string(),
                                );
                                if let Err(persist_error) = host.persist_preparation_failure(
                                    &self.task_queue.as_ref().unwrap().tasks[index],
                                    &mut result,
                                ) {
                                    result.diagnostics.push(format!(
                                        "setup receipt persistence failed: {persist_error}"
                                    ));
                                }
                                settled.push_back((index, None, result, ExecutionState::fresh()));
                            }
                        }
                    }
                }
            }

            self.continuation.running_children = running.len();
            let outcome = if let Some(item) = settled.pop_front() {
                Some(item)
            } else if running.is_empty() {
                None
            } else {
                running.next().await
            };
            let Some((index, run, result, child_state)) = outcome else {
                break;
            };
            self.continuation.running_children = running.len();
            reserved.retain(|(held, _)| *held != index);
            self.record_child(index, run.as_ref(), result, &child_state, checkpoint)?;
        }
        // The receipt index is written once per dispatch, not once per child:
        // the full records are already durable in each child's own session, and
        // a per-child rewrite would persist O(n²) bytes for n children.
        if self.receipts_dirty {
            self.receipts_dirty = false;
            if let Some(snapshot) = child_result::snapshot(&self.child_results) {
                self.raw_turn_messages.push(snapshot);
                checkpoint(&self.raw_turn_messages)?;
            }
        }
        Ok(executed)
    }

    /// Indices of ready children, in queue order. `started` marks tasks this
    /// call already claimed; `reserved` holds the resources of in-flight work.
    fn select_ready_children(
        &self,
        started: &[bool],
        reserved: &[(usize, Vec<tool::ResourceAccess>)],
        capacity: usize,
    ) -> Vec<usize> {
        let Some(queue) = self.task_queue.as_ref() else {
            return Vec::new();
        };
        if queue.state != QueueState::Active {
            return Vec::new();
        }
        let mut ready = Vec::new();
        // A task may reserve itself during this same selection round.
        let mut claimed: Vec<Vec<tool::ResourceAccess>> = Vec::new();
        for (index, task) in queue.tasks.iter().enumerate() {
            if ready.len() >= capacity {
                break;
            }
            if started[index]
                || !matches!(task.status, TaskStatus::Pending | TaskStatus::Running)
                || task.depends_on.iter().any(|&dep| {
                    queue
                        .tasks
                        .get(dep)
                        .is_none_or(|d| d.status != TaskStatus::Completed)
                })
            {
                continue;
            }
            // Write/write and read/write overlap on the same resource is never
            // run concurrently, whether the conflict comes from a declared path
            // or from an opaque `all` capability.
            let conflicts = |held: &Vec<tool::ResourceAccess>| {
                task.resources
                    .iter()
                    .any(|resource| held.iter().any(|other| resource.conflicts(other)))
            };
            if reserved.iter().any(|(_, held)| conflicts(held)) || claimed.iter().any(conflicts) {
                continue;
            }
            claimed.push(task.resources.clone());
            ready.push(index);
        }
        ready
    }

    /// Provision every selected child. Preparation happens concurrently so the
    /// controller is not serialized behind workspace provisioning.
    async fn prepare_children(
        &self,
        host: &std::sync::Arc<dyn crate::ChildHost>,
        ready: &[usize],
    ) -> Vec<Prepared> {
        let kernel: &AgentKernel = self;
        let inputs = ready
            .iter()
            .map(|&index| {
                (
                    index,
                    self.task_queue.as_ref().unwrap().tasks[index].clone(),
                )
            })
            .collect::<Vec<_>>();
        let mut futures = Vec::with_capacity(inputs.len());
        for (index, task) in inputs {
            let host = host.clone();
            futures.push(async move { (index, host.prepare_task(kernel, &task).await) });
        }
        futures_util::future::join_all(futures)
            .await
            .into_iter()
            .map(|(index, outcome)| match outcome {
                Ok(child) => Prepared::Ready(index, Box::new(child)),
                Err(error) => Prepared::Failed(index, error),
            })
            .collect()
    }

    /// Persist the child identity, then either accept its recovered receipt or
    /// start it running.
    fn attach_child<'a, F, H>(
        &mut self,
        index: usize,
        mut child: PreparedChild,
        emit: &'a Mutex<F>,
        checkpoint: &mut H,
        running: &mut FuturesUnordered<BoxFuture<'a, ChildSettlement>>,
        settled: &mut VecDeque<ChildSettlement>,
    ) -> Result<(), AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        self.touch_progress();
        let child_id = child.run.session_id.clone();
        let task_id = task_id_of(index);
        let input = self.task_queue.as_ref().unwrap().tasks[index]
            .task_input()
            .to_owned();
        {
            let task = &mut self.task_queue.as_mut().unwrap().tasks[index];
            task.child = Some(child.run.clone());
        }
        self.execution
            .lock()
            .unwrap()
            .current_step
            .clone_from(&input);
        // The child's identity is durable before its first model or tool call,
        // so a disconnect can resume the same child instead of provisioning a
        // second one and redoing its side effects.
        self.checkpoint_queue(checkpoint)?;
        if let Some(mut result) = child.terminal.take() {
            // Already finished in an earlier process; never re-run it.
            result.child_id = child_id;
            result.task_id = task_id;
            settled.push_back((index, Some(child.run), result, ExecutionState::fresh()));
            return Ok(());
        }
        running.push(run_child_future(
            index, task_id, child_id, input, child, emit,
        ));
        Ok(())
    }

    /// Fold one finished child back into the controller: shared execution
    /// progress, the durable receipt index, and the queue frontier.
    fn record_child<H>(
        &mut self,
        index: usize,
        run: Option<&ChildRun>,
        result: ChildResult,
        child_state: &ExecutionState,
        checkpoint: &mut H,
    ) -> Result<(), AgentError>
    where
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        if let Some(run) = run
            && self
                .execution
                .lock()
                .unwrap()
                .absorb_child(&run.session_id, child_state)
        {
            self.raw_turn_messages
                .push(self.execution_state().snapshot());
        }
        self.touch_progress();
        let summary = result.model_summary();
        let status = result.status;
        child_result::store(&mut self.child_results, result);
        self.receipts_dirty = true;
        self.continuation.unconsumed_child_results += 1;
        let queue = self.task_queue.as_mut().unwrap();
        let task = &mut queue.tasks[index];
        // A receipt is the only thing that decides the task's fate; the compact
        // summary is what the model reads, the full receipt stays durable.
        task.status = if status.success() {
            TaskStatus::Completed
        } else {
            TaskStatus::Failed
        };
        task.outcome = Some(summary.clone());
        task.failure_reason = (!status.success()).then_some(summary);
        queue.advance();
        self.checkpoint_queue(checkpoint)
    }
}

enum Prepared {
    /// Boxed: a prepared child owns a kernel and a checkpoint, and the failure
    /// variant must not inflate every stack slot of the dispatch loop.
    Ready(usize, Box<PreparedChild>),
    Failed(usize, AgentError),
}

fn task_id_of(index: usize) -> String {
    format!("task-{}", index + 1)
}

#[allow(clippy::too_many_arguments)]
fn run_child_future<F>(
    index: usize,
    task_id: String,
    child_id: String,
    input: String,
    child: PreparedChild,
    emit: &Mutex<F>,
) -> BoxFuture<'_, ChildSettlement>
where
    F: FnMut(AgentEvent) + Send,
{
    Box::pin(async move {
        let run = child.run.clone();
        let session = run.session_id.clone();
        let mut kernel = child.kernel;
        let mut checkpoint = child.checkpoint;
        let observer = Mutex::new(ChildObserver::new());
        let shared = &observer;
        let outcome = {
            let sink: ChildSaveSink<'_> = Box::new(|messages| checkpoint.save(messages));
            let event_session = session.clone();
            let events: Box<dyn FnMut(AgentEvent) + Send + '_> = Box::new(move |mut event| {
                shared.lock().unwrap().observe(&event);
                match &mut event {
                    AgentEvent::ToolStarted { id, .. } | AgentEvent::ToolFinished { id, .. } => {
                        *id = format!("{event_session}:{id}");
                    }
                    _ => {}
                }
                // Child text and turn boundaries are not controller output.
                if !matches!(
                    event,
                    AgentEvent::ContentDelta { .. }
                        | AgentEvent::TurnStarted
                        | AgentEvent::TurnFinished
                ) {
                    (emit.lock().unwrap())(event);
                }
            });
            AgentSupervisor::run_child(&mut kernel, &input, events, sink).await
        };
        let mut observer = observer.into_inner().unwrap();
        observer.absorb_usage(kernel.messages());
        let child_state = kernel.execution_state();
        let (status, failure) = match &outcome {
            Ok(output) => {
                observer.set_output(output);
                (ChildStatus::Completed, None)
            }
            Err(error) => (classify_child_error(error), Some(error.to_string())),
        };
        let mut result = observer.into_result(child_id, task_id, status, failure);
        if let Err(error) = checkpoint.finish(&mut result) {
            result.status = ChildStatus::Failed;
            result.diagnostics.push(format!("receipt failed: {error}"));
            result
                .failure_reason
                .get_or_insert_with(|| error.to_string());
        }
        (index, Some(run), result, child_state)
    })
}

fn classify_child_error(error: &AgentError) -> ChildStatus {
    match error {
        AgentError::Timeout(_) => ChildStatus::TimedOut,
        _ => ChildStatus::Failed,
    }
}

/// Kept so the queue's own view of "nothing left to run" is still the queue's.
#[must_use]
pub(crate) fn has_open_work(queue: &TaskQueue) -> bool {
    queue
        .tasks
        .iter()
        .any(|task| matches!(task.status, TaskStatus::Pending | TaskStatus::Running))
}

#[cfg(test)]
#[path = "child_dispatch_tests.rs"]
mod tests;
