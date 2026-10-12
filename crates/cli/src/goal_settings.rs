//! The same Goal templates used by explicit Goal creation, exposed to host settings.
use anyhow::{Context, Result, ensure};
use runtime_core::GoalConfig;
use std::{io::Read, path::Path};

pub(crate) fn manage(
    root: &Path,
    scope: &str,
    json_stdin: bool,
    reset: bool,
) -> Result<GoalConfig> {
    ensure!(
        ["global", "project"].contains(&scope),
        "invalid Goal settings scope"
    );
    ensure!(!(json_stdin && reset), "reset cannot include settings");
    let mut global = crate::config::AxConfig::load()?;
    let next = if json_stdin {
        let mut bytes = Vec::new();
        std::io::stdin().take(1_048_577).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 1_048_576, "Goal template exceeds 1 MiB");
        let config: GoalConfig =
            serde_json::from_slice(&bytes).context("invalid GoalConfig JSON")?;
        config.validate().map_err(anyhow::Error::msg)?;
        Some(config)
    } else if reset {
        Some(GoalConfig::default())
    } else {
        None
    };
    if let Some(next) = next {
        if scope == "global" {
            global.goal = next;
            global.save_goal_template()?;
        } else {
            let path = root.join(".ax/goal.json");
            if reset {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                // An inherited template must not be silently replaced by legacy migration.
                let legacy = root.join(".ax/verification.json");
                if legacy.exists() {
                    std::fs::rename(
                        &legacy,
                        legacy.with_file_name(format!(
                            "verification.settings-backup-{}.json",
                            uuid::Uuid::new_v4()
                        )),
                    )?;
                }
            } else {
                std::fs::create_dir_all(path.parent().expect("project settings directory"))?;
                let temporary = path.with_extension("settings.tmp");
                std::fs::write(&temporary, serde_json::to_vec_pretty(&next)?)?;
                std::fs::rename(temporary, &path)?;
            }
        }
    }
    let mut result = if scope == "project" {
        crate::config::project_goal_template(root, global.goal)?
    } else {
        global.goal
    };
    // Return portable references for editing instead of saving a machine-specific root.
    for path in &mut result.deliverables {
        if let Ok(relative) = path.strip_prefix(root) {
            *path = relative.to_path_buf();
        }
    }
    for validator in &mut result.validators {
        if let runtime_core::goal::ValidatorKind::Command { reference, .. } = &mut validator.kind
            && let Ok(relative) = reference.strip_prefix(root)
        {
            *reference = relative.to_path_buf();
        }
    }
    result.validate().map_err(anyhow::Error::msg)?;
    Ok(result)
}
