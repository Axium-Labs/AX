use crate::{Capability, SafetyLevel, Tool, ToolError, ToolOutput};
use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const MAX_IMAGE_BYTES: u64 = 10_000_000;

pub struct ViewImageTool {
    workspace: PathBuf,
}

impl ViewImageTool {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
    async fn load(&self, path: &Path) -> Result<ToolOutput, ToolError> {
        let root = tokio::fs::canonicalize(&self.workspace).await?;
        let path = tokio::fs::canonicalize(path).await?;
        if !path.starts_with(root) {
            return Err(ToolError::InvalidInput(
                "image path is outside workspace".into(),
            ));
        }
        let metadata = tokio::fs::metadata(&path).await?;
        if !metadata.is_file() || metadata.len() > MAX_IMAGE_BYTES {
            return Err(ToolError::InvalidInput(
                "image must be a file under 10 MB".into(),
            ));
        }
        let data = tokio::fs::read(&path).await?;
        let media_type = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if data.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            "image/gif"
        } else {
            return Err(ToolError::InvalidInput("unsupported image format".into()));
        };
        Ok(ToolOutput::Image {
            description: format!("Image: {}", path.display()),
            media_type: media_type.into(),
            data: base64::engine::general_purpose::STANDARD.encode(data),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: PathBuf,
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &'static str {
        "view_image"
    }
    fn description(&self) -> &'static str {
        "Read a workspace PNG, JPEG, WebP or GIF as a native image for a vision-capable model."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let _: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        Err(ToolError::Execution(
            "view_image requires multimodal tool execution".into(),
        ))
    }
    async fn execute_output(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        self.load(&input.path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn loads_image_as_native_part_and_rejects_outside_path() {
        let root = std::env::temp_dir().join(format!("ax-image-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let path = root.join("image.png");
        tokio::fs::write(&path, b"\x89PNG\r\n\x1a\nsmall")
            .await
            .unwrap();
        let tool = ViewImageTool::new(root.clone());
        let result = tool.execute_output(json!({"path":path})).await.unwrap();
        assert!(
            matches!(result, ToolOutput::Image { media_type, .. } if media_type == "image/png")
        );
        assert!(
            tool.execute_output(json!({"path":std::env::current_exe().unwrap()}))
                .await
                .is_err()
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
