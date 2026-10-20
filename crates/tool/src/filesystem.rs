use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write as _;

use crate::{SafetyLevel, Tool, ToolError, ToolOutput};

pub struct FilesystemTool;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum FilesystemInput {
    Read {
        path: String,
        start_line: Option<usize>,
        end_line: Option<usize>,
    },
    List {
        path: String,
    },
    Write {
        path: String,
        content: String,
    },
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for FilesystemTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::WorkspaceWorker
    }
    fn fork_for_run(&self, context: &crate::RunContext) -> Option<std::sync::Arc<dyn Tool>> {
        Some(std::sync::Arc::new(crate::WorkspaceTool::new(
            std::sync::Arc::new(FilesystemTool),
            context.cwd.clone(),
        )))
    }
    fn name(&self) -> &str {
        "filesystem"
    }

    fn description(&self) -> &str {
        "Read, list or write a known path. `read` accepts a 1-based start_line/end_line range when sufficient; omit the range for full file context. All reads return verbatim text with `N: ` line-number prefixes; these prefixes are not file content and must be omitted from patch text/expected_lines. Truncated results report total_lines, shown_through_line and next_start_line; continue with that start_line. A line too long to display shows only its beginning and sets line_truncated; use search to locate content inside it. Files above 4 MiB must be read by line range. `list` shows exactly one directory level and is not a discovery tool: prefer find_files/glob for unknown locations and search for text or symbols. After discovery, prefer direct reads for context; targeted searches within known files remain useful. Batch independent reads when possible. Prefer patch for localized edits; write creates or replaces a whole file and is appropriate for requested full-file rewrites or generated artifacts. Inspect existing content before replacing it; writes require approval."
    }

    fn guidance(&self) -> Option<&'static str> {
        Some(
            "filesystem: read only the smallest sufficient 1-based line range; omit the range only \
             when the whole file is genuinely needed. Files above 4 MiB require a line range. If a \
             result reports line_truncated, that line is too long to display in full; use search to \
             locate content inside it. `list` returns one directory level and is not a discovery \
             tool. Batch independent reads of already-known paths into one call rather than \
             alternating read -> model -> read.",
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": { "type": "string", "enum": ["read", "list", "write"], "description": "`read` a file (optionally a 1-based line range), `list` one directory level, or `write` create or replace an entire file (requires approval)." },
                "path": { "type": "string", "description": "Path relative to the workspace root." },
                "content": { "type": "string", "description": "Complete new file content for write; replaces existing content rather than appending." },
                "start_line": { "type":"integer", "minimum":1 },
                "end_line": { "type":"integer", "minimum":1 }
            },
            "required": ["operation", "path"],
            "additionalProperties": false
        })
    }

    fn capability(&self, input: &Value) -> crate::Capability {
        if input.get("operation").and_then(Value::as_str) == Some("write") {
            crate::Capability::FilesystemWrite
        } else {
            crate::Capability::FilesystemRead
        }
    }

    fn safety(&self, input: &Value) -> SafetyLevel {
        if input.get("operation").and_then(Value::as_str) == Some("write") {
            SafetyLevel::RequiresApproval
        } else {
            SafetyLevel::Safe
        }
    }

    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        match self.execute_output(input).await? {
            ToolOutput::Text(text) | ToolOutput::Verbatim { text, .. } => Ok(text),
            ToolOutput::Image { description, .. } => Ok(description),
        }
    }

    async fn execute_output(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let parsed: FilesystemInput = serde_json::from_value(input)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        match parsed {
            FilesystemInput::Read {
                path,
                start_line,
                end_line,
            } => read_file(&path, start_line, end_line).await,
            FilesystemInput::List { path } => {
                let mut entries = tokio::fs::read_dir(&path)
                    .await
                    .map_err(|error| crate::path_error(std::path::Path::new(&path), &error))?;
                let mut names = Vec::new();
                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|error| ToolError::Execution(error.to_string()))?
                {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
                names.sort_unstable();
                Ok(ToolOutput::Text(names.join("\n")))
            }
            FilesystemInput::Write { path, content } => {
                tokio::fs::write(path, content)
                    .await
                    .map_err(|error| ToolError::Execution(error.to_string()))?;
                Ok(ToolOutput::Text("write completed".to_owned()))
            }
        }
    }

    fn resources(&self, input: &Value) -> Vec<crate::ResourceAccess> {
        let Some(path) = input["path"].as_str() else {
            return vec![crate::ResourceAccess::exclusive()];
        };
        let resource = crate::Resource::path(path);
        vec![if input["operation"] == "write" {
            crate::ResourceAccess::write(resource)
        } else {
            crate::ResourceAccess::read(resource)
        }]
    }
}

async fn read_file(
    path: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<ToolOutput, ToolError> {
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or_else(|| {
        if start_line.is_none() {
            usize::MAX
        } else {
            start.saturating_add(79)
        }
    });
    if start == 0 || end < start {
        return Err(ToolError::InvalidInput("invalid line range".into()));
    }
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| crate::path_error(std::path::Path::new(path), &error))?;
    if metadata.len() > crate::discovery::MAX_FILE_BYTES {
        if start_line.is_none() && end_line.is_none() {
            return Err(ToolError::InvalidInput(format!(
                "{path} is {} bytes, above the {} byte read limit; read a line range with \
                 start_line/end_line or use search to locate content",
                metadata.len(),
                crate::discovery::MAX_FILE_BYTES
            )));
        }
        return read_ranged_stream(path, start, end).await;
    }
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|error| crate::path_error(std::path::Path::new(path), &error))?;
    let total_lines = text.split_inclusive('\n').count();
    let numbered = text
        .split_inclusive('\n')
        .enumerate()
        .skip(start - 1)
        .take(end.saturating_sub(start).saturating_add(1))
        .fold(String::new(), |mut numbered, (i, line)| {
            let _ = write!(numbered, "{}: {line}", i + 1);
            numbered
        });
    Ok(ToolOutput::Verbatim {
        text: numbered,
        total_lines,
        start_line: start,
        truncated: false,
        line_truncated: false,
    })
}

#[derive(Clone, PartialEq, Eq)]
struct LineCountKey {
    path: std::path::PathBuf,
    modified: std::time::SystemTime,
    len: u64,
}

impl LineCountKey {
    async fn read(path: &str) -> Option<Self> {
        let path = tokio::fs::canonicalize(path).await.ok()?;
        let metadata = tokio::fs::metadata(&path).await.ok()?;
        Some(Self {
            path,
            modified: metadata.modified().ok()?,
            len: metadata.len(),
        })
    }

    fn bytes(&self) -> usize {
        std::mem::size_of::<(Self, usize)>().saturating_add(self.path.as_os_str().len())
    }
}

/// Exact counts are cached lazily, within the existing shared file-byte limit.
/// The deque is LRU (hits move to its back); there is no startup scan.
#[derive(Default)]
struct LineCounts(std::collections::VecDeque<(LineCountKey, usize)>);

impl LineCounts {
    fn get(&mut self, key: &LineCountKey) -> Option<usize> {
        let index = self.0.iter().position(|(cached, _)| cached == key)?;
        let entry = self.0.remove(index)?;
        let count = entry.1;
        self.0.push_back(entry);
        Some(count)
    }

    fn insert(&mut self, key: LineCountKey, count: usize) {
        // A new version supersedes old metadata for this path.
        self.0.retain(|(old, _)| old.path != key.path);
        let limit = usize::try_from(crate::discovery::MAX_FILE_BYTES).unwrap_or(usize::MAX);
        let mut bytes: usize = self.0.iter().map(|(key, _)| key.bytes()).sum();
        if key.bytes() > limit {
            return;
        }
        while bytes.saturating_add(key.bytes()) > limit {
            let Some((old, _)) = self.0.pop_front() else {
                break;
            };
            bytes = bytes.saturating_sub(old.bytes());
        }
        self.0.push_back((key, count));
    }
}

fn line_counts() -> &'static std::sync::Mutex<LineCounts> {
    static COUNTS: std::sync::OnceLock<std::sync::Mutex<LineCounts>> = std::sync::OnceLock::new();
    COUNTS.get_or_init(Default::default)
}

/// Range retention (including number prefixes) is bounded by `MAX_FILE_BYTES`.
/// Even a requested single huge line keeps only a UTF-8 prefix and marks it
/// partial. On a count-cache miss the rest is scanned without retaining it;
/// on a hit reading stops at the range end or the retention limit.
async fn read_ranged_stream(path: &str, start: usize, end: usize) -> Result<ToolOutput, ToolError> {
    use tokio::io::AsyncBufReadExt;
    let key = LineCountKey::read(path).await;
    let cached = key
        .as_ref()
        .and_then(|key| line_counts().lock().ok()?.get(key));
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| crate::path_error(std::path::Path::new(path), &error))?;
    let mut reader = tokio::io::BufReader::new(file);
    let mut number = 0usize;
    let mut numbered = Vec::new();
    let limit = usize::try_from(crate::discovery::MAX_FILE_BYTES).unwrap_or(usize::MAX);
    let mut started = false;
    let mut wanted = false;
    let mut truncated = false;
    let mut line_truncated = false;
    loop {
        let (consumed, complete) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                break;
            }
            let length = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if !started {
                number = number.saturating_add(1);
                wanted = number >= start && number <= end;
                started = true;
                if wanted && !truncated {
                    let prefix = format!("{number}: ");
                    if prefix.len() >= limit.saturating_sub(numbered.len()) {
                        truncated = true;
                    } else {
                        numbered.extend_from_slice(prefix.as_bytes());
                    }
                }
            }
            if wanted && !truncated {
                let take = length.min(limit.saturating_sub(numbered.len()));
                numbered.extend_from_slice(&available[..take]);
                if take < length {
                    truncated = true;
                    line_truncated = true;
                }
            }
            (length, available[length - 1] == b'\n')
        };
        reader.consume(consumed);
        if complete {
            started = false;
        }
        if cached.is_some() && ((complete && number >= end) || truncated) {
            break;
        }
    }
    if cached.is_none()
        && let Some(key) = key
        && LineCountKey::read(path).await.as_ref() == Some(&key)
        && let Ok(mut counts) = line_counts().lock()
    {
        counts.insert(key, number);
    }
    // Cutting a byte slice may split one UTF-8 character. Only an incomplete
    // tail caused by our limit is recoverable; invalid source bytes still fail.
    if let Err(error) = std::str::from_utf8(&numbered) {
        if line_truncated && error.error_len().is_none() {
            numbered.truncate(error.valid_up_to());
        } else {
            return Err(ToolError::Execution(format!(
                "{path} is not valid UTF-8 text"
            )));
        }
    }
    let text = String::from_utf8(numbered)
        .map_err(|_| ToolError::Execution(format!("{path} is not valid UTF-8 text")))?;
    Ok(ToolOutput::Verbatim {
        text,
        total_lines: cached.unwrap_or(number),
        start_line: start,
        truncated,
        line_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ranged_read_and_missing_path_include_recovery_information() {
        let root = std::env::temp_dir().join(format!("ax-read-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let path = root.join("actual.rs");
        tokio::fs::write(&path, "one\ntwo\nthree\n").await.unwrap();
        let result = FilesystemTool
            .execute(json!({"operation":"read","path":path,"start_line":2,"end_line":2}))
            .await
            .unwrap();
        assert_eq!(result, "2: two\n");
        let error = FilesystemTool
            .execute(json!({"operation":"read","path":root.join("missing.rs")}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("candidate_paths"));
        assert!(error.to_string().contains("actual.rs"));
        tokio::fs::remove_file(path).await.unwrap();
        tokio::fs::remove_dir(root).await.unwrap();
    }

    /// Above `MAX_FILE_BYTES` a whole-file read must fail with the size and the
    /// limit, while a ranged read still works without loading the file.
    #[tokio::test]
    async fn oversized_file_requires_a_line_range() {
        let root = std::env::temp_dir().join(format!("ax-read-big-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let path = root.join("big.txt");
        let mut content = String::new();
        for line in 1..=200 {
            let _ = writeln!(content, "line-{line:03} {}", "x".repeat(22_000));
        }
        tokio::fs::write(&path, &content).await.unwrap();
        let size = tokio::fs::metadata(&path).await.unwrap().len();
        assert!(size > crate::discovery::MAX_FILE_BYTES);

        let error = FilesystemTool
            .execute(json!({"operation":"read","path":path}))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("invalid tool input"), "{message}");
        assert!(message.contains(&size.to_string()), "{message}");
        assert!(
            message.contains(&crate::discovery::MAX_FILE_BYTES.to_string()),
            "{message}"
        );
        assert!(message.contains("start_line"), "{message}");
        assert!(message.contains("search"), "{message}");

        let output = FilesystemTool
            .execute_output(json!({"operation":"read","path":path,"start_line":199,"end_line":200}))
            .await
            .unwrap();
        let ToolOutput::Verbatim {
            text,
            total_lines,
            start_line,
            ..
        } = output
        else {
            panic!("file reads declare verbatim text");
        };
        assert_eq!(total_lines, 200);
        assert_eq!(start_line, 199);
        assert_eq!(text.lines().count(), 2);
        assert!(text.starts_with("199: line-199 "));
        assert!(text.ends_with('\n'));

        // A range before the end, a CRLF file without a final newline and a
        // range past EOF all keep the same coordinate semantics as a normal read.
        let crlf = root.join("crlf.txt");
        let mut body = String::new();
        for line in 1..=120 {
            let _ = write!(body, "row {line} {}\r\n", "y".repeat(40_000));
        }
        body.push_str("tail without newline");
        tokio::fs::write(&crlf, &body).await.unwrap();
        assert!(tokio::fs::metadata(&crlf).await.unwrap().len() > crate::discovery::MAX_FILE_BYTES);
        let output = FilesystemTool
            .execute_output(json!({"operation":"read","path":crlf,"start_line":2,"end_line":2}))
            .await
            .unwrap();
        let ToolOutput::Verbatim {
            text,
            total_lines,
            start_line,
            ..
        } = output
        else {
            panic!("file reads declare verbatim text");
        };
        assert_eq!(start_line, 2);
        assert_eq!(text.lines().count(), 1);
        assert!(text.starts_with("2: row 2 "));
        assert!(text.ends_with("\r\n"));
        assert_eq!(total_lines, 121);

        let output = FilesystemTool
            .execute_output(json!({"operation":"read","path":crlf,"start_line":121,"end_line":121}))
            .await
            .unwrap();
        let ToolOutput::Verbatim { text, .. } = output else {
            panic!("file reads declare verbatim text");
        };
        assert_eq!(text, "121: tail without newline");

        let output = FilesystemTool
            .execute_output(json!({"operation":"read","path":crlf,"start_line":500,"end_line":500}))
            .await
            .unwrap();
        let ToolOutput::Verbatim {
            text, total_lines, ..
        } = output
        else {
            panic!("file reads declare verbatim text");
        };
        assert_eq!(text, "");
        assert_eq!(total_lines, 121);

        tokio::fs::remove_file(crlf).await.unwrap();
        tokio::fs::remove_file(path).await.unwrap();
        tokio::fs::remove_dir(root).await.unwrap();
    }
}
