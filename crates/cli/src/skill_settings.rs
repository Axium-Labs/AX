//! Persist explicit skill choices separately from dependency availability.
use std::collections::BTreeSet;

use anyhow::Result;

use crate::ReplState;

impl ReplState {
    pub(crate) fn disabled_skills(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .capability_registry(crate::capabilities::Kind::Skills)?
            .entries()
            .filter(|entry| !entry.enabled)
            .map(|entry| entry.name.clone())
            .collect())
    }

    #[cfg(test)]
    pub(crate) fn toggle_skill(&mut self, name: &str) -> Result<()> {
        let disabled = self.disabled_skills()?.contains(name);
        self.manage_capability(
            crate::capabilities::Kind::Skills,
            scoped::Scope::Project,
            if disabled { "enable" } else { "disable" },
            name,
            None,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, active_skill_name};

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
        // Isolate this fixture from any Skills installed in the developer's AX home.
        state.allowed_skills = Some(std::collections::HashSet::from(["review".to_owned()]));
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
        assert!(routed.is_empty());
        // Seed a historical explicit invocation to verify disabling/resume cleanup.
        let loaded = state.skills().unwrap().load("review").unwrap();
        let routed = vec![Message::system(format!(
            "[ax-skill:review]\n{}",
            loaded.instructions
        ))];
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
        restored.allowed_skills = Some(std::collections::HashSet::from(["review".to_owned()]));
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
        assert!(restored.route_skills("review", 1000).unwrap().is_empty());
        assert!(
            restored
                .skill_catalog_context(10_000)
                .unwrap()
                .0
                .unwrap()
                .content
                .contains("Review code")
        );
        drop(restored);
        std::fs::remove_dir_all(root).unwrap();
    }
}
