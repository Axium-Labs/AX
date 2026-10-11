//! Native /goal command. It only prepares the existing prompt/queue path.
use crate::repl::ReplState;
use anyhow::{Result, bail};
use runtime_core::{GoalConfig, GoalMode, GoalState, GoalTurn, QueueState, task_queue::TaskQueue};

pub(crate) enum GoalCommand {
    Run(String),
    Output(String),
}
pub(crate) fn prepare(state: &mut ReplState, command: &str) -> Result<GoalCommand> {
    let args = command.strip_prefix("/goal").unwrap_or_default().trim();
    let mut messages = state.loaded_messages.clone();
    let mut queue = state
        .runtime
        .as_ref()
        .and_then(|k| k.task_queue().cloned())
        .or_else(|| TaskQueue::restore(&mut messages));
    if args.is_empty() || args == "status" {
        return Ok(GoalCommand::Output(queue.as_ref().and_then(status).unwrap_or_else(||
            "No optional Goal. /goal <target> starts one; /goal status|pause|resume|cancel|limits <JSON>.".into())));
    }
    if matches!(args, "pause" | "cancel" | "off" | "resume") || args.starts_with("limits ") {
        let Some(q) = queue.as_mut().filter(|q| q.goal.is_some()) else {
            bail!("No optional Goal in this session");
        };
        let goal = q.goal.as_mut().unwrap();
        if matches!(
            goal.state,
            GoalState::Completed | GoalState::Cancelled | GoalState::Failed
        ) {
            bail!("Goal is terminal: {:?}", goal.state);
        }
        if args == "resume" {
            if state.pending_question().is_some() {
                bail!("Answer the pending question before resuming");
            }
            state.next_goal_turn = GoalTurn::Resume {
                goal_id: q.goal_id.clone(),
            };
            return Ok(GoalCommand::Run(format!(
                "Resume Goal {}. Inspect recorded tool results, completed child receipts and actual external state before retrying interrupted operations. Continue only unfinished work.",
                q.goal_id
            )));
        }
        if let Some(json) = args.strip_prefix("limits ") {
            if goal.state != GoalState::Paused && goal.state != GoalState::Blocked {
                bail!("Pause the Goal before changing limits");
            }
            patch_limits(&mut goal.config, json)?;
        } else {
            goal.state = if args == "pause" {
                GoalState::Paused
            } else {
                GoalState::Cancelled
            };
            goal.stop_reason = Some(format!("User requested {args}"));
            q.state = if args == "pause" {
                QueueState::Suspended
            } else {
                QueueState::Cancelled
            };
            q.stop_reason = goal.stop_reason.clone();
        }
        let snapshot = q.snapshot();
        let mut saved = 0;
        state.persist_turn_messages(std::slice::from_ref(&snapshot), &mut saved)?;
        if let Some(runtime) = state.runtime.take() {
            let mut context = runtime.messages().to_vec();
            context.push(snapshot.clone());
            state.runtime = Some(runtime.with_messages(context));
        }
        state.loaded_messages.push(snapshot);
        return Ok(GoalCommand::Output(format!(
            "Goal {}: {:?}",
            q.goal_id,
            q.goal.as_ref().unwrap().state
        )));
    }
    let mut config = crate::config::goal_template(&state.project_root)?;
    if config.mode == GoalMode::Off {
        config.mode = GoalMode::Model;
    }
    let target = parse_target(args, &mut config)?;
    if target.trim().is_empty() || config.mode == GoalMode::Off {
        bail!("Goal needs a target and an enabled mode");
    }
    crate::config::resolve_goal_paths(&state.project_root, &mut config);
    let goal_id = match &state.next_goal_turn {
        GoalTurn::Start { goal_id } => Some(goal_id.clone()),
        GoalTurn::Pursue { goal_id, .. } => goal_id.clone(),
        _ => None,
    };
    state.next_goal_turn = GoalTurn::Pursue { config, goal_id };
    Ok(GoalCommand::Run(target))
}
fn parse_target(args: &str, config: &mut GoalConfig) -> Result<String> {
    let target = if let Some(json) = args
        .strip_prefix("create ")
        .filter(|s| s.trim_start().starts_with('{'))
    {
        let mut value: serde_json::Value = serde_json::from_str(json)?;
        let target = value
            .as_object_mut()
            .and_then(|v| v.remove("target"))
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| anyhow::anyhow!("create requires target"))?;
        let mut template = serde_json::to_value(&*config)?;
        for (key, val) in value.as_object().unwrap() {
            template[key] = val.clone();
        }
        *config = serde_json::from_value::<GoalConfig>(template)?;
        target
    } else {
        args.to_owned()
    };
    Ok(target)
}

fn status(queue: &TaskQueue) -> Option<String> {
    let goal = queue.goal.as_ref()?;
    Some(format!(
        "Goal {}: {:?}\n{}\nAcceptance: {}\nChecks: {} (stagnant {}) | rounds: {} | requests: {} | tokens: {}{} | cost: {} | active time: {}ms\nDecision: {:?}\n{}",
        queue.goal_id,
        goal.state,
        goal.target,
        goal.config.acceptance.join("; "),
        goal.checks,
        goal.stagnant_checks,
        goal.usage.rounds,
        goal.usage.requests,
        goal.usage
            .input_tokens
            .saturating_add(goal.usage.output_tokens),
        if goal.usage.estimated {
            " (estimated)"
        } else {
            ""
        },
        goal.usage.cost_display(),
        goal.usage.elapsed_ms,
        goal.last_decision,
        goal.stop_reason.as_deref().unwrap_or_default()
    ))
}

fn patch_limits(config: &mut GoalConfig, json: &str) -> Result<()> {
    let patch: serde_json::Value = serde_json::from_str(json)?;
    let mut value = serde_json::to_value(&*config)?;
    let Some(object) = patch.as_object() else {
        bail!("limits expects a JSON object");
    };
    for (key, val) in object {
        if !matches!(
            key.as_str(),
            "max_rounds"
                | "max_seconds"
                | "max_tokens"
                | "max_cost_microusd"
                | "input_microusd_per_million"
                | "output_microusd_per_million"
                | "max_stagnant_checks"
        ) {
            bail!("Not a Goal limit: {key}");
        }
        value[key] = val.clone();
    }
    *config = serde_json::from_value(value)?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/goal_commands.rs"]
mod tests;
