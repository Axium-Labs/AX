//! Persist explicit skill choices separately from dependency availability.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};

use crate::{ReplState, active_skill_name};

impl ReplState {
    pub(crate) fn disabled_skills(&self) -> Result<BTreeSet<String>> {
        let path = self.data_dir.join("disabled-skills.json");
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid disabled-skills.json"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(error) => Err(error).context("Cannot read skill settings"),
        }
    }

    pub(crate) fn toggle_skill(&mut self, name: &str) -> Result<()> {
        if self.skills()?.directory(name).is_none() {
            bail!("Unknown skill: {name}");
        }
        let mut disabled = self.disabled_skills()?;
        if !disabled.remove(name) {
            disabled.insert(name.to_owned());
        }
        std::fs::create_dir_all(&self.data_dir)?;
        let temporary = self.data_dir.join("disabled-skills.json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&disabled)?)?;
        std::fs::rename(temporary, self.data_dir.join("disabled-skills.json"))?;
        self.invalidate_runtime();
        self.loaded_messages
            .retain(|message| active_skill_name(message).as_deref() != Some(name));
        self.active_skills.remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[test]
    fn toggle_persists_affects_routing_and_resume_without_deleting_history() {
        let root = std::env::temp_dir().join(format!(
            "ax-skill-toggle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let skills = root.join("skills");
        let source = skills.join("different-directory-name");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("skill.toml"),
            r#"name = "review"
description = "Review code"
trigger_keywords = ["review"]
required_tools = ["mcp"]
"#,
        )
        .unwrap();
        std::fs::write(source.join("instructions.md"), "Review carefully.").unwrap();
        let data = root.join("data");
        let mut state =
            ReplState::new_in_project(data.clone(), skills.clone(), None, &root).unwrap();
        assert_eq!(
            state.skills().unwrap().directory("review"),
            Some(source.as_path())
        );
        let (catalog_context, used) = state.skill_catalog_context(10_000).unwrap();
        let catalog_context = catalog_context.unwrap();
        assert!(used > 0);
        assert!(catalog_context.content.contains("Review code"));
        assert!(catalog_context.content.contains("instructions.md"));
        assert!(!catalog_context.content.contains("Review carefully."));
        state.create_session("test").unwrap();
        let session = state.current_session_id().unwrap().to_owned();
        let routed = state.route_skills("review", 1000).unwrap();
        assert_eq!(routed.len(), 1);
        state.persist_messages(&routed).unwrap();
        state.loaded_messages.extend(routed);
        state
            .loaded_messages
            .push(Message::user("Keep this message"));
        state.toggle_skill("review").unwrap();
        assert!(
            state
                .skill_catalog_context(10_000)
                .unwrap()
                .0
                .as_ref()
                .is_none_or(|message| !message.content.contains("Review code"))
        );
        assert!(
            state
                .loaded_messages
                .iter()
                .all(|m| active_skill_name(m).is_none())
        );
        assert!(state.route_skills("review", 1000).unwrap().is_empty());
        assert_eq!(
            state
                .store()
                .unwrap()
                .load_context_messages(&session)
                .unwrap()
                .len(),
            1
        );
        drop(state);
        let mut restored = ReplState::new_in_project(data, skills, None, &root).unwrap();
        assert!(restored.disabled_skills().unwrap().contains("review"));
        let budget = runtime_core::ContextBudget::new(32000, Some(1000), 0);
        assert!(restored.open_session(&session, &budget).unwrap());
        assert!(
            restored
                .loaded_messages
                .iter()
                .all(|m| active_skill_name(m).is_none())
        );
        restored.toggle_skill("review").unwrap();
        assert_eq!(restored.route_skills("review", 1000).unwrap().len(), 1);
        drop(restored);
        std::fs::remove_dir_all(root).unwrap();
    }
}
