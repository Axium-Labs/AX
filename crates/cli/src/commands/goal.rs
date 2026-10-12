//! Native /goal command. It only prepares the existing prompt/queue path.
use crate::repl::ReplState;
use anyhow::{Result, bail};
use runtime_core::{GoalConfig, GoalMode, GoalState, GoalTurn, QueueState, task_queue::TaskQueue};

pub(crate) enum GoalCommand {
    Run(String),
    Output(String),
}
// Control admission and its Session checkpoint form one ordered operation.
#[allow(clippy::too_many_lines)]
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
    if args == "rotate" {
        let _lease = state.goal_session_lease(queue.as_ref())?;
        let q = queue
            .as_ref()
            .filter(|q| q.goal.is_some())
            .ok_or_else(|| anyhow::anyhow!("No Goal to rotate"))?;
        let goal = q.goal.as_ref().unwrap();
        if matches!(
            goal.state,
            GoalState::Completed | GoalState::Cancelled | GoalState::Failed
        ) {
            bail!("Goal is terminal");
        }
        if state.pending_question().is_some() {
            bail!("Answer pending user input before rotation");
        }
        if !goal.config.long_project.as_ref().is_some_and(|p| p.enabled)
            && goal.config.session_rotation.is_none()
        {
            bail!("Configure Long Project or session_rotation before manual rotation");
        }
        state.next_rotation = true;
        state.next_goal_turn = GoalTurn::Resume {
            goal_id: q.goal_id.clone(),
        };
        return Ok(GoalCommand::Run("Rotate this Goal at the next safe boundary and continue only unfinished work; check authoritative task state and external effects.".into()));
    }
    if let Some(args) = args
        .strip_prefix("project")
        .filter(|v| v.is_empty() || v.starts_with(' '))
        .map(str::trim)
    {
        let _lease = state.goal_session_lease(queue.as_ref())?;
        let q = queue
            .as_mut()
            .filter(|q| q.goal.is_some())
            .ok_or_else(|| anyhow::anyhow!("No Goal for Long Project"))?;
        if args.starts_with('{') {
            let goal = q.goal.as_mut().unwrap();
            if !matches!(
                goal.state,
                GoalState::Paused | GoalState::Blocked | GoalState::Created
            ) {
                bail!("Pause the Goal before changing Long Project configuration");
            }
            let patch: serde_json::Value = serde_json::from_str(args)?;
            let mut value =
                serde_json::to_value(goal.config.long_project.clone().unwrap_or_default())?;
            crate::long_project::merge(&mut value, &patch)?;
            goal.config.long_project = Some(serde_json::from_value(value)?);
            goal.config.validate().map_err(anyhow::Error::msg)?;
            let snapshot = q.snapshot();
            state.persist_messages(std::slice::from_ref(&snapshot))?;
            if let Some(runtime) = state.runtime.take() {
                let mut m = runtime.messages().to_vec();
                m.push(snapshot.clone());
                state.runtime = Some(runtime.with_messages(m));
            }
            state.loaded_messages.push(snapshot);
            crate::long_project::refresh_native_state(state, q)?;
            return Ok(GoalCommand::Output("Long Project configuration saved; document/history retained. Resume explicitly to execute.".into()));
        }
        return Ok(GoalCommand::Output(crate::long_project::manage(
            state, q, args,
        )?));
    }
    if matches!(args, "pause" | "cancel" | "off" | "resume")
        || args.starts_with("limits ")
        || args.starts_with("acknowledge ")
    {
        let _lease = state.goal_session_lease(queue.as_ref())?;
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
        if let Some(id) = args.strip_prefix("acknowledge ") {
            if !matches!(goal.state, GoalState::Paused | GoalState::Blocked) {
                bail!("Pause the Goal before acknowledging external effects");
            }
            let pending = goal
                .pending_validation
                .as_ref()
                .filter(|p| p.validator_id == id.trim())
                .ok_or_else(|| anyhow::anyhow!("No interrupted validator with that ID"))?;
            let marker = model::Message::system(format!(
                "[ax-goal-effects-acknowledged]\n{}",
                serde_json::json!({"goal_id":q.goal_id,"validator_id":pending.validator_id,"call_id":pending.call_id,"reason":"User explicitly reviewed external effects and authorized a future validation"})
            ));
            let mut saved = 0;
            state.persist_turn_messages(&[marker], &mut saved)?;
            goal.pending_validation = None;
        } else if let Some(json) = args.strip_prefix("limits ") {
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
        crate::long_project::refresh_native_state(state, q)?;
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
    config.validate().map_err(anyhow::Error::msg)?;
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
        crate::long_project::merge(&mut template, &value)?;
        *config = serde_json::from_value::<GoalConfig>(template)?;
        target
    } else {
        args.to_owned()
    };
    Ok(target)
}

fn status(queue: &TaskQueue) -> Option<String> {
    let goal = queue.goal.as_ref()?;
    let project = goal
        .config
        .long_project
        .as_ref()
        .map_or("off".to_owned(), |p| {
            format!(
                "{}; automatic rotation: {}; strategy: {:?}; threshold: {}%",
                p.enabled, p.rotation.automatic, p.rotation.strategy, p.rotation.context_percent
            )
        });
    Some(format!(
        "Goal {}: {:?}\n{}\nAcceptance: {}\nChecks: {} (stagnant {}) | rounds: {} | requests: {} | tokens: {}{} | cost: {} | active time: {}ms\nDecision: {:?}\n{}\nActivity: {:?} | Validators: {}\nLong Project: {}",
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
        goal.stop_reason.as_deref().unwrap_or_default(),
        goal.activity,
        goal.config
            .validators
            .iter()
            .map(|v| format!("{}@{}", v.id, v.version))
            .collect::<Vec<_>>()
            .join(", "),
        project
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
                | "stagnant_seconds"
                | "max_unobserved_checks"
                | "stagnation_action"
        ) {
            bail!("Not a Goal limit: {key}");
        }
        value[key] = val.clone();
    }
    *config = serde_json::from_value(value)?;
    Ok(())
}
