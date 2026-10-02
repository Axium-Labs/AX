//! `ax agents`: independent tasks with bounded concurrency.
//!
//! One supervisor clones a template kernel per task, so the prompts share a
//! provider, tool registry and approval policy without sharing history.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use runtime_core::{AgentSupervisor, AgentTask, ApprovalPolicy, ExecutionBudget};

use crate::{model_selection::ModelSelection, runtime};

/// Runs each prompt in its own agent context and prints the answers in task
/// order. Individual failures are reported per task and do not fail the run.
///
/// # Errors
///
/// Returns an error when the template kernel or a supervisor worker cannot be
/// constructed.
pub(crate) async fn run(
    prompts: &[String],
    concurrency: usize,
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    budget: ExecutionBudget,
    auth_path: &Path,
) -> Result<()> {
    let template = runtime::kernel(selection, approval, Vec::new(), &[], auth_path)?
        .with_execution_budget(budget);
    let tasks = prompts
        .iter()
        .enumerate()
        .map(|(index, prompt)| AgentTask {
            id: format!("agent-{:04}", index + 1),
            prompt: prompt.clone(),
            context: Vec::new(),
        })
        .collect();
    let results = AgentSupervisor::new(template, concurrency)
        .run_tasks(tasks, None)
        .await?;
    for result in results {
        match result.result {
            Ok(output) => println!("[{}]\n{output}\n", result.id),
            Err(error) => eprintln!("[{}] error: {error}", result.id),
        }
    }
    Ok(())
}
