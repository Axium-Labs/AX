//! Atomic, line-addressed multi-hunk patching; does not invoke a shell.
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
    start_line: usize,
    delete_count: usize,
    #[serde(default)]
    expected_lines: Option<Vec<String>>,
    new_text: String,
}
pub struct PatchTool;
#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &'static str {
        "patch"
    }
    fn description(&self) -> &'static str {
        "Apply an atomic structured multi-hunk patch. Each edit addresses original 1-based start_line and delete_count (0 inserts), with optional expected_lines to detect stale context. Read only the affected lines first. All hunks validate before writing."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"edits":{"type":"array","minItems":1,"maxItems":128,"items":{"type":"object","properties":{"start_line":{"type":"integer","minimum":1},"delete_count":{"type":"integer","minimum":0},"expected_lines":{"type":"array","items":{"type":"string"}},"new_text":{"type":"string"}},"required":["start_line","delete_count","new_text"],"additionalProperties":false}}},"required":["path","edits"],"additionalProperties":false})
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::FilesystemWrite
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::RequiresApproval
    }
    fn resources(&self, input: &Value) -> Vec<crate::ResourceAccess> {
        input["path"].as_str().map_or_else(
            || vec![crate::ResourceAccess::exclusive()],
            |path| vec![crate::ResourceAccess::write(crate::Resource::path(path))],
        )
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        // Canonicalize first so replacing a symlink edits its target, not the link.
        let path = tokio::fs::canonicalize(&input.path)
            .await
            .map_err(|error| crate::path_error(&input.path, &error))?;
        let metadata = tokio::fs::metadata(&path).await?;
        if metadata.len() > 2_000_000 || input.edits.is_empty() || input.edits.len() > 128 {
            return Err(ToolError::InvalidInput(
                "file or edit count exceeds limit".into(),
            ));
        }
        let original = tokio::fs::read_to_string(&path).await?;
        let lines: Vec<_> = original.split_inclusive('\n').collect();
        let mut edits: Vec<_> = input.edits.iter().enumerate().collect();
        edits.sort_by_key(|(_, edit)| edit.start_line);
        let mut updated = String::new();
        let mut cursor = 0;
        let mut additions = 0;
        let mut deletions = 0;
        for (index, edit) in edits {
            let start = edit.start_line.saturating_sub(1);
            let end = start.saturating_add(edit.delete_count);
            let actual: Vec<String> = lines
                .get(start..end)
                .unwrap_or_default()
                .iter()
                .map(|line| line.trim_end_matches(['\r', '\n']).to_owned())
                .collect();
            if edit.start_line == 0
                || start < cursor
                || end > lines.len()
                || start > lines.len()
                || edit
                    .expected_lines
                    .as_ref()
                    .is_some_and(|expected| expected != &actual)
            {
                let context: Vec<_> = lines
                    .iter()
                    .enumerate()
                    .skip(start.saturating_sub(2))
                    .take(edit.delete_count.saturating_add(4).min(20))
                    .map(|(i, line)| json!({"line":i+1,"text":line.trim_end_matches(['\r','\n'])}))
                    .collect();
                return Err(ToolError::InvalidInput(json!({"diagnostics":[{"kind":"patch_conflict","path":path,"hunk":index,"start_line":edit.start_line,"actual_lines":actual,"context":context,"message":"Overlapping, out-of-range or stale hunk; no changes written. Read this range and repair this hunk."}]}).to_string()));
            }
            updated.push_str(&lines[cursor..start].concat());
            let replacement = if original.contains("\r\n") {
                edit.new_text.replace("\r\n", "\n").replace('\n', "\r\n")
            } else {
                edit.new_text.clone()
            };
            updated.push_str(&replacement);
            additions += edit.new_text.lines().count();
            deletions += edit.delete_count;
            cursor = end;
        }
        updated.push_str(&lines[cursor..].concat());
        if updated.len() > 2_000_000 {
            return Err(ToolError::InvalidInput(
                "patched file exceeds size limit".into(),
            ));
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
        Ok(json!({"path":path,"edits_applied":input.edits.len(),"changed_files":[{"path":path,"additions":additions,"deletions":deletions}]}).to_string())
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
    async fn line_hunks_can_edit_repeated_text_and_preserve_crlf() {
        let path = std::env::temp_dir().join(format!("ax-patch-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, "same\r\nkeep\r\nsame\r\n")
            .await
            .unwrap();
        let _cleanup = Temporary(path.clone());
        PatchTool
            .execute(json!({"path":path,"edits":[
                {"start_line":3,"delete_count":1,"expected_lines":["same"],"new_text":"last\n"},
                {"start_line":1,"delete_count":1,"expected_lines":["same"],"new_text":"first\n"}
            ]}))
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "first\r\nkeep\r\nlast\r\n"
        );
    }
    #[tokio::test]
    async fn invalid_later_edit_does_not_write_partial_changes() {
        let path = std::env::temp_dir().join(format!("ax-patch-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, "alpha\nbeta\n").await.unwrap();
        let cleanup = Temporary(path.clone());
        let error = PatchTool.execute(json!({"path":path,"edits":[{"start_line":1,"delete_count":1,"new_text":"new\n"},{"start_line":2,"delete_count":1,"expected_lines":["missing"],"new_text":"bad"}]})).await.unwrap_err();
        assert!(error.to_string().contains("patch_conflict"));
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "alpha\nbeta\n"
        );
        PatchTool
            .execute(json!({"path":path,"edits":[{"start_line":1,"delete_count":1,"expected_lines":["alpha"],"new_text":"new\n"}]}))
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "new\nbeta\n"
        );
        drop(cleanup);
    }
}
