//! One declarative inheritance contract; unsupported host policies fail closed.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextInheritance {
    #[default]
    None,
    Summary,
    LastN,
    Full,
    /// Every completed turn of the parent conversation, excluding the current
    /// in-flight turn: the `subagent_fork` seed.
    CompletedTurns,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryInheritance {
    None,
    ParentReadonly,
    #[default]
    Isolated,
    SharedProject,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    #[default]
    None,
    Selected,
    Inherit,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelInheritance {
    #[default]
    Inherit,
    Override,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceInheritance {
    Shared,
    Snapshot,
    #[default]
    Isolated,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionInheritance {
    #[default]
    InheritRestricted,
    Custom,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChildPolicy {
    pub context: ContextInheritance,
    pub last_n: usize,
    pub memory: MemoryInheritance,
    pub skills: Selection,
    pub selected_skills: Vec<String>,
    pub tools: Selection,
    pub selected_tools: Vec<String>,
    pub mcp: Selection,
    pub selected_mcp: Vec<String>,
    pub model: ModelInheritance,
    pub model_override: Option<String>,
    pub workspace: WorkspaceInheritance,
    pub permissions: PermissionInheritance,
    pub custom_permissions: tool::PermissionProfile,
}
impl Default for ChildPolicy {
    fn default() -> Self {
        Self {
            context: ContextInheritance::None,
            last_n: 0,
            memory: MemoryInheritance::Isolated,
            skills: Selection::None,
            selected_skills: vec![],
            tools: Selection::Inherit,
            selected_tools: vec![],
            mcp: Selection::None,
            selected_mcp: vec![],
            model: ModelInheritance::Inherit,
            model_override: None,
            workspace: WorkspaceInheritance::Isolated,
            permissions: PermissionInheritance::InheritRestricted,
            custom_permissions: tool::PermissionProfile::default(),
        }
    }
}
impl ChildPolicy {
    /// # Errors
    /// Rejects `last_n` inheritance without a positive complete-turn count.
    pub fn inherited_context(
        &self,
        messages: &[model::Message],
    ) -> Result<Vec<model::Message>, &'static str> {
        use model::Role;
        let mut selected = match self.context {
            ContextInheritance::None => vec![],
            ContextInheritance::Summary => messages
                .iter()
                .filter(|m| m.role == Role::System && m.content.starts_with("[memory-summary]"))
                .cloned()
                .collect(),
            ContextInheritance::Full => messages.to_vec(),
            // Completed turns end where the current in-flight turn starts, so
            // the fork child never sees the turn that is delegating it.
            ContextInheritance::CompletedTurns => {
                let start = messages
                    .iter()
                    .rposition(|m| m.role == Role::User)
                    .unwrap_or(messages.len());
                messages[..start].to_vec()
            }
            ContextInheritance::LastN => {
                if self.last_n == 0 {
                    return Err("last_n requires a positive count");
                }
                // Count complete turns, never split tool call/result groups.
                let starts: Vec<_> = messages
                    .iter()
                    .enumerate()
                    .filter_map(|(i, m)| (m.role == Role::User).then_some(i))
                    .collect();
                starts
                    .get(starts.len().saturating_sub(self.last_n))
                    .map_or_else(Vec::new, |i| messages[*i..].to_vec())
            }
        };
        selected.retain(|m| {
            !m.content.starts_with(crate::task_queue::STATE_PREFIX)
                && !m.content.starts_with(crate::task_queue::ARCHIVE_PREFIX)
                && !m.content.starts_with(crate::execution::STATE_PREFIX)
                && !m.content.starts_with("[ax-progress]")
                && (self.skills != Selection::None || !m.content.contains("[ax-skill:"))
                && (self.memory != MemoryInheritance::None
                    || !m.content.contains("[retrieved-memory]"))
                && (self.memory != MemoryInheritance::None
                    || !m.content.starts_with("[retrieved-memory]"))
                && match self.skills {
                    Selection::Inherit => true,
                    Selection::None => {
                        !m.content.contains("[ax-skill:") && !m.content.contains("[skill-catalog]")
                    }
                    Selection::Selected => {
                        !m.content.contains("[skill-catalog]")
                            && m.content.split("[ax-skill:").skip(1).all(|body| {
                                body.split_once(']').is_some_and(|(name, _)| {
                                    self.selected_skills.iter().any(|selected| selected == name)
                                })
                            })
                    }
                }
        });
        Ok(selected)
    }
}

impl ChildPolicy {
    #[must_use]
    pub fn schema() -> serde_json::Value {
        use serde_json::json;
        let names = json!({"type":"array","items":{"type":"string"}});
        json!({"type":"object","additionalProperties":false,"properties":{
            "context":{"type":"string","enum":["none","summary","last_n","full"]},
            "last_n":{"type":"integer","minimum":1,"description":"Complete user turns for context=last_n"},
            "memory":{"type":"string","enum":["none","parent_readonly","isolated","shared_project"]},
            "skills":{"type":"string","enum":["none","selected","inherit"]},"selected_skills":names,
            "tools":{"type":"string","enum":["none","selected","inherit"]},"selected_tools":names,
            "mcp":{"type":"string","enum":["none","selected","inherit"]},"selected_mcp":names,
            "model":{"type":"string","enum":["inherit","override"]},"model_override":{"type":"string","description":"Parent-registered child model key"},
            "workspace":{"type":"string","enum":["shared","snapshot","isolated"]},
            "permissions":{"type":"string","enum":["inherit_restricted","custom"]},
            "custom_permissions":{"type":"object","description":"Additional restrictions; cannot relax parent profile/capabilities or sandbox"}
        }})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use model::Message;
    #[test]
    fn defaults_and_complete_turn_inheritance() {
        let policy: ChildPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(policy.context, ContextInheritance::None);
        assert_eq!(policy.memory, MemoryInheritance::Isolated);
        assert_eq!(policy.workspace, WorkspaceInheritance::Isolated);
        assert!(
            policy
                .inherited_context(&[Message::user("secret")])
                .unwrap()
                .is_empty()
        );
        let history = vec![
            Message::system("[ax-task-queue]\n{}"),
            Message::user("old"),
            Message::assistant("old answer", vec![]),
            Message::user("last"),
            Message::assistant("last answer", vec![]),
        ];
        let selected = ChildPolicy {
            context: ContextInheritance::LastN,
            last_n: 1,
            ..Default::default()
        }
        .inherited_context(&history)
        .unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].content, "last");
        assert!(
            ChildPolicy {
                context: ContextInheritance::LastN,
                ..Default::default()
            }
            .inherited_context(&history)
            .is_err()
        );
        assert_eq!(
            ChildPolicy {
                context: ContextInheritance::Full,
                ..Default::default()
            }
            .inherited_context(&history)
            .unwrap()
            .len(),
            4
        );
    }
    #[test]
    fn selected_skills_filter_wrapped_tool_results_and_parent_catalog() {
        let policy = ChildPolicy {
            context: ContextInheritance::Full,
            skills: Selection::Selected,
            selected_skills: vec!["chosen".into()],
            ..Default::default()
        };
        let history = vec![
            Message::user("task"),
            Message::system("[skill-catalog] all skills"),
            Message::tool("1", r#"{"raw_output":"[ax-skill:chosen] instructions"}"#),
            Message::tool(
                "2",
                r#"{"raw_output":"[ax-skill:other] private instructions"}"#,
            ),
        ];
        let selected = policy.inherited_context(&history).unwrap();
        assert_eq!(selected.len(), 2);
        assert!(selected[1].content.contains("[ax-skill:chosen]"));
    }
}
