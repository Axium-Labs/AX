//! Project-scoped, JSON-only inspection and explicit evolved-Skill management.

use anyhow::Result;
use evolution::Engine;
use serde_json::{Value, json};

use crate::args::{Cli, EvolutionCommand};

/// Errors are command output too: keep stdout machine-readable and fail the process.
pub(crate) fn fail(error: impl std::fmt::Display, code: i32) -> ! {
    println!("{}", json!({"error": error.to_string()}));
    std::process::exit(code);
}

pub(crate) fn run(cli: &Cli, command: &EvolutionCommand) {
    match manage(cli, command) {
        Ok(value) => println!("{value:#}"),
        Err(error) => fail(format!("{error:#}"), 1),
    }
}

fn manage(cli: &Cli, command: &EvolutionCommand) -> Result<Value> {
    let cwd = std::env::current_dir()?;
    let project_root = crate::bootstrap::discover_project_root(&cwd);
    let (data_dir, _) =
        crate::bootstrap::resolve_directories(&cwd, cli.data_dir.clone(), cli.skills_dir.clone());
    let project = crate::project_identity::load_or_create(&project_root)?;
    let root = data_dir.join("evolution").join(&project);
    // Engine::open may recover JSONL telemetry or migrate its ledger; lock first.
    let _lock = Engine::writer_lock(&root)?;
    let mut engine = Engine::open(root, crate::bootstrap::database_path(&data_dir), project)?;
    match command {
        EvolutionCommand::Status => {
            let personalization = crate::config::AxConfig::load()?.personalization;
            Ok(json!({
                "enabled": personalization.evolution_active() && engine.config.enabled,
                "pending": engine.ledger.pending,
                "last_analysis": engine.ledger.last_analysis,
                "consecutive_failures": engine.ledger.consecutive_failures,
                "backoff_secs": engine.analysis_cooldown(),
                "pending_reviews": engine.ledger.trial_requests.len(),
            }))
        }
        EvolutionCommand::Skills => Ok(Value::Array(
            engine
                .ledger
                .skills
                .iter()
                .filter(|(_, meta)| meta.source == "evolved" && meta.project == engine.project)
                .map(|(name, meta)| {
                    json!({
                        "name": name,
                        "state": meta.state,
                        "use_count": meta.use_count,
                        "success_count": meta.success_count,
                        "failure_count": meta.failure_count,
                        "created_at": meta.created_at,
                        "last_used_at": meta.last_used_at,
                        "evidence_count": meta.evidence.len(),
                        "digest": engine.ledger.trial_requests.get(name),
                    })
                })
                .collect(),
        )),
        EvolutionCommand::Show { name } => {
            let text = engine.skill_text(name)?;
            let meta = &engine.ledger.skills[name];
            let evidence = meta.evidence.iter().map(|id| {
                let experience = engine.experience(id)?;
                Ok(json!({"id": id, "task": experience.task.chars().take(200).collect::<String>()}))
            }).collect::<Result<Vec<_>>>()?;
            Ok(json!({"name": name, "state": meta.state, "skill_md": text, "evidence": evidence}))
        }
        EvolutionCommand::Review {
            name,
            digest,
            accept,
            reject,
        } => {
            anyhow::ensure!(
                *accept != *reject,
                "choose exactly one of --accept or --reject"
            );
            engine.review_candidate(name, digest, *accept)?;
            Ok(json!({"name": name, "state": engine.ledger.skills[name].state}))
        }
        EvolutionCommand::Archive { name } => {
            engine.archive_skill(name)?;
            Ok(json!({"name": name, "state": engine.ledger.skills[name].state}))
        }
    }
}
