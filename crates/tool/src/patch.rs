//! Exact-text atomic patching; does not invoke a shell.
use crate::{Capability, SafetyLevel, Tool, ToolError};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: PathBuf,
    edits: Vec<Edit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    old_text: String,
    new_text: String,
}
pub struct PatchTool;
#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &'static str {
        "patch"
    }
    fn description(&self) -> &'static str {
        "Atomically patch an existing UTF-8 file with exact unique old_text/new_text replacements. All edits are validated before writing; ambiguous or missing matches reject the entire patch."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"edits":{"type":"array","minItems":1,"maxItems":128,"items":{"type":"object","properties":{"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["old_text","new_text"],"additionalProperties":false}}},"required":["path","edits"],"additionalProperties":false})
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::FilesystemWrite
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::RequiresApproval
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        // Canonicalize first so replacing a symlink edits its target, not the link.
        let path = tokio::fs::canonicalize(&input.path).await?;
        let metadata = tokio::fs::metadata(&path).await?;
        if metadata.len() > 2_000_000 || input.edits.is_empty() || input.edits.len() > 128 {
            return Err(ToolError::InvalidInput(
                "file or edit count exceeds limit".into(),
            ));
        }
        let original = tokio::fs::read_to_string(&path).await?;
        let mut updated = original.clone();
        for edit in &input.edits {
            if edit.old_text.is_empty() || updated.matches(&edit.old_text).count() != 1 {
                return Err(ToolError::InvalidInput(
                    "old_text must match exactly once; no changes written".into(),
                ));
            }
            updated = updated.replacen(&edit.old_text, &edit.new_text, 1);
            if updated.len() > 2_000_000 {
                return Err(ToolError::InvalidInput(
                    "patched file exceeds size limit".into(),
                ));
            }
        }
        let temporary = path.with_file_name(format!(".ax-patch-{}", uuid::Uuid::new_v4()));
        let cleanup = Temporary(temporary.clone());
        tokio::fs::write(&temporary, updated).await?;
        tokio::fs::set_permissions(&temporary, metadata.permissions()).await?;
        if tokio::fs::read_to_string(&path).await? != original {
            return Err(ToolError::Execution(
                "file changed while patching; retry with latest contents".into(),
            ));
        }
        tokio::fs::rename(&temporary, &path).await?;
        drop(cleanup);
        Ok(json!({"path":path,"edits_applied":input.edits.len()}).to_string())
    }
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invalid_later_edit_does_not_write_partial_changes() {
        let path = std::env::temp_dir().join(format!("ax-patch-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, "alpha\nbeta\n").await.unwrap();
        let cleanup = Temporary(path.clone());
        assert!(PatchTool.execute(json!({"path":path,"edits":[{"old_text":"alpha","new_text":"new"},{"old_text":"missing","new_text":"bad"}]})).await.is_err());
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "alpha\nbeta\n"
        );
        PatchTool
            .execute(json!({"path":path,"edits":[{"old_text":"alpha","new_text":"new"}]}))
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "new\nbeta\n"
        );
        drop(cleanup);
    }
}
