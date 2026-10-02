//! Explicit main-model skill selection. Only eligible metadata is exposed.
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};
use tool::{Capability, SafetyLevel, Tool, ToolError};
#[derive(Clone)]
pub(crate) struct SkillInvocation {
    pub catalog: Arc<skill::SkillCatalog>,
    pub allowed: HashSet<String>,
}
#[async_trait]
impl Tool for SkillInvocation {
    fn name(&self) -> &'static str {
        "invoke_skill"
    }
    fn inheritance_class(&self) -> tool::InheritanceClass {
        tool::InheritanceClass::Skills
    }
    fn fork_skills(&self, selected: Option<&[String]>) -> Option<Arc<dyn Tool>> {
        let mut child = self.clone();
        if let Some(selected) = selected {
            child.allowed.retain(|name| selected.contains(name));
        }
        Some(Arc::new(child))
    }
    fn execution_boundary(&self) -> tool::ExecutionBoundary {
        tool::ExecutionBoundary::RuntimeOwned
    }
    fn description(&self) -> &'static str {
        "Explicitly select a relevant eligible skill by name using the compact catalog. Loads its instructions lazily; allowed-tools never grants permissions."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"],"additionalProperties":false})
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn capability(&self, _: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn resources(&self, input: &Value) -> Vec<tool::ResourceAccess> {
        input["name"]
            .as_str()
            .filter(|n| self.allowed.contains(*n))
            .and_then(|n| self.catalog.instruction_path(n))
            .map_or_else(Vec::new, |path| {
                vec![tool::ResourceAccess::read(tool::Resource::path(path))]
            })
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let name = input["name"]
            .as_str()
            .filter(|n| self.allowed.contains(*n))
            .ok_or_else(|| {
                ToolError::PermissionDenied("skill unavailable under current policy".into())
            })?;
        let loaded = self
            .catalog
            .load(name)
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        Ok(format!(
            "[ax-skill:{}]\nSkill root: {}\n{}",
            name,
            loaded.directory.display(),
            loaded.instructions
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_explicit_eligible_invocation_loads_body() {
        let root = std::env::temp_dir().join(format!("ax-skill-invoke-{}", uuid::Uuid::new_v4()));
        skill::create_skill_directory(&root, "review", "Review code", "unique body").unwrap();
        let catalog = Arc::new(skill::SkillCatalog::index(&root).unwrap());
        let tool = SkillInvocation {
            catalog,
            allowed: HashSet::from(["review".into()]),
        };
        assert!(tool.execute(json!({"name":"unknown"})).await.is_err());
        assert!(
            tool.execute(json!({"name":"review"}))
                .await
                .unwrap()
                .contains("unique body")
        );
        let narrowed = tool.fork_skills(Some(&[])).unwrap();
        assert!(narrowed.execute(json!({"name":"review"})).await.is_err());
        std::fs::remove_file(root.join("review/SKILL.md")).unwrap();
        assert!(tool.execute(json!({"name":"review"})).await.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
