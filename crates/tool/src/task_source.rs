//! Projected table input and explicit record-to-work mapping, independent of dataset domain.
use crate::{
    Capability, EnvironmentContext, Resource, ResourceAccess, RunContext, SafetyLevel, Tool,
    ToolError,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

pub struct TaskSourceTool {
    root: PathBuf,
}
impl TaskSourceTool {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

const READER: &str = r"import json,sys
path,columns=sys.argv[1],json.loads(sys.argv[2])
if path.lower().endswith('.parquet'):
 import pyarrow.parquet as pq
 rows=pq.read_table(path,columns=columns).to_pylist()
elif path.lower().endswith('.csv'):
 import csv
 with open(path,encoding='utf-8',newline='') as f: rows=[{c:r[c] for c in columns} for r in csv.DictReader(f)]
else:
 with open(path,encoding='utf-8') as f: data=json.load(f)
 rows=[{c:r[c] for c in columns} for r in data]
print(json.dumps({'records':rows},ensure_ascii=True))";

#[async_trait::async_trait]
impl Tool for TaskSourceTool {
    fn name(&self) -> &'static str {
        "task_source"
    }
    fn description(&self) -> &'static str {
        "Read only explicitly selected columns from parquet/JSON/CSV. Never load full parquet rows or answer fields for schema discovery. Use before task_queue when work items come from a table. Optional work mapping turns each record into a complete executable task and automatically registers/dispatches the inventory; no separate queue call is needed. Repository URL templates use {column} placeholders from selected fields only. Reader/setup errors are recoverable: install an available reader or use a projected fallback."
    }
    fn guidance(&self) -> Option<&'static str> {
        Some(
            "task_source: select the exact columns you need before materializing rows; never read \
             answer fields for schema discovery. Only reach for this when the work genuinely comes \
             from a table — a request that is not table-driven needs no reader and no queue.",
        )
    }
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::WorkspaceWorker
    }
    fn fork_for_run(&self, context: &RunContext) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self::new(context.cwd.clone())))
    }
    fn capability(&self, _: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
        vec![ResourceAccess::read(Resource::path(
            self.root.join(input["path"].as_str().unwrap_or(".")),
        ))]
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{
        "path":{"type":"string"},"columns":{"type":"array","minItems":1,"items":{"type":"string"}},
        "work":{"type":"object","description":"Only use for concrete user-requested executable records, not instruction rows or setup checklists","properties":{
            "title_column":{"type":"string"},"instruction":{"type":"string","description":"Complete per-record task requirements, validation and output contract; no sibling/reference data"},
            "repo_url":{"type":"string","description":"Repository URL template, e.g. https://github.com/{repo}.git"},"revision_column":{"type":"string"},"revision":{"type":"string","description":"Explicit common revision, e.g. HEAD only if the user intends latest; otherwise use revision_column"},"subdir":{"type":"string"},
            "output_root":{"type":"string"},"sequential":{"type":"boolean","description":"Honor user ordering: true for sequential/one-at-a-time requests; false only when independent parallel execution is compatible with the request"},"workspace_mode":{"type":"string","enum":["inherit","git","empty"]}
        },"required":["title_column","instruction","sequential"],"additionalProperties":false}
    },"required":["path","columns"],"additionalProperties":false})
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let mut work = parse_work(&input)?;
        let path = self.root.join(
            input["path"]
                .as_str()
                .ok_or_else(|| ToolError::InvalidInput("path required".into()))?,
        );
        let columns = input["columns"]
            .as_array()
            .filter(|columns| !columns.is_empty())
            .ok_or_else(|| ToolError::InvalidInput("explicit selected columns required".into()))?;
        if columns
            .iter()
            .any(|column| column.as_str().is_none_or(str::is_empty))
        {
            return Err(ToolError::InvalidInput(
                "columns must be nonempty strings".into(),
            ));
        }
        let env = EnvironmentContext::detect(&self.root, &self.root);
        let python = env
            .executables
            .get("python")
            .or_else(|| env.executables.get("python3"))
            .ok_or_else(|| {
                ToolError::Execution(
                    "projected reader needs Python; use an available runtime or projected fallback"
                        .into(),
                )
            })?;
        let output = tokio::process::Command::new(&python.path)
            .arg("-X")
            .arg("utf8")
            .arg("-c")
            .arg(READER)
            .arg(&path)
            .arg(input["columns"].to_string())
            .current_dir(&self.root)
            .kill_on_drop(true)
            .output()
            .await?;
        if !output.status.success() {
            return Err(ToolError::Execution(format!(
                "projected reader setup/read failed (recoverable): {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        if let Some(work) = &mut work
            && let Some(root) = work["output_root"].as_str()
        {
            work["output_root"] = json!(self.root.join(root));
        }
        map_records(&value, work.as_ref(), &path)
    }
}

fn parse_work(input: &Value) -> Result<Option<Value>, ToolError> {
    input
        .get("work")
        .map(|value| {
            let parsed = if let Some(encoded) = value.as_str() {
                serde_json::from_str(encoded).map_err(|error| {
                    ToolError::InvalidInput(format!(
                        "work must be an object or JSON-encoded object: {error}"
                    ))
                })?
            } else {
                value.clone()
            };
            if !parsed.is_object() {
                return Err(ToolError::InvalidInput(
                    "work must be an object with title_column, instruction and sequential".into(),
                ));
            }
            Ok(parsed)
        })
        .transpose()
}

fn map_records(
    value: &Value,
    work: Option<&Value>,
    path: &std::path::Path,
) -> Result<String, ToolError> {
    let records = value["records"]
        .as_array()
        .ok_or_else(|| ToolError::InvalidInput("records required".into()))?;
    let Some(work) = work else {
        return Ok(value.to_string());
    };
    let title_column = work["title_column"]
        .as_str()
        .ok_or_else(|| ToolError::InvalidInput("title_column required".into()))?;
    let instruction = work["instruction"]
        .as_str()
        .filter(|instruction| !instruction.trim().is_empty())
        .ok_or_else(|| ToolError::InvalidInput("complete work instruction required".into()))?;
    if !work["sequential"].is_boolean() {
        return Err(ToolError::InvalidInput("work.sequential must explicitly select requested ordering (true for one at a time, false for independent parallel tasks)".into()));
    }
    if work.get("repo_url").is_some()
        && work.get("revision_column").is_none()
        && work.get("revision").is_none()
    {
        return Err(ToolError::InvalidInput("repository work requires explicit revision_column or revision; do not silently substitute HEAD for a user-requested commit".into()));
    }
    let mut tasks = Vec::with_capacity(records.len());
    let mut seen = std::collections::HashSet::new();
    for record in records {
        let title = record[title_column]
            .as_str()
            .filter(|title| !title.is_empty())
            .ok_or_else(|| {
                ToolError::InvalidInput("record title must be a nonempty string".into())
            })?;
        if !seen.insert(title) {
            return Err(ToolError::InvalidInput(
                "record work identifiers must be unique".into(),
            ));
        }
        let mut task = json!({"title":title,"input":format!("{instruction}\nTask data (selected fields only): {record}")});
        if let Some(url) = work["repo_url"].as_str() {
            let mut url = url.to_owned();
            for (column, value) in record
                .as_object()
                .ok_or_else(|| ToolError::InvalidInput("record must be an object".into()))?
            {
                if let Some(value) = value.as_str() {
                    url = url.replace(&format!("{{{column}}}"), value);
                }
            }
            let revision = work["revision_column"]
                .as_str()
                .and_then(|column| record[column].as_str())
                .or_else(|| work["revision"].as_str());
            if work.get("revision_column").is_some() && revision.is_none() {
                return Err(ToolError::InvalidInput(
                    "revision column missing from selected fields".into(),
                ));
            }
            task["workspace"] =
                json!({"mode":"git","repo_url":url,"revision":revision,"subdir":work["subdir"]});
        } else {
            task["workspace"] =
                json!({"mode":work["workspace_mode"].as_str().unwrap_or("inherit")});
        }
        if let Some(root) = work["output_root"].as_str() {
            let title_path = std::path::Path::new(title);
            if title_path.components().count() != 1
                || !matches!(
                    title_path.components().next(),
                    Some(std::path::Component::Normal(_))
                )
                || title.contains(['/', '\\', ':'])
            {
                return Err(ToolError::InvalidInput(
                    "output identifier must be one safe directory name".into(),
                ));
            }
            task["output_dir"] = json!(PathBuf::from(root).join(title));
        }
        if work["sequential"] == true {
            task["resources"] =
                json!([{"name":format!("task-source:{}",path.display()),"write":true}]);
        }
        tasks.push(task);
    }
    Ok(json!({"count":tasks.len(),"ax_work_items":tasks}).to_string())
}

#[cfg(test)]
#[path = "../../../test/harness/task_source.rs"]
mod tests;
