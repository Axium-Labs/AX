//! Persistence primitives: atomic writes, digests, territory guards and the
//! credential screen every stored artifact passes.
//!
//! `plain` refuses symlinks and Windows reparse points, so evolution can never
//! write through a link out of its own root.

use std::{fs, path::Path};

use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};

#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(crate) fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
/// Reuse credential screening without imposing Memory's 1000-character fact limit.
pub(crate) fn screen(text: &str) -> Result<()> {
    ensure!(!text.trim().is_empty(), "empty learning content");
    let chars: Vec<_> = text.chars().collect();
    for start in (0..chars.len()).step_by(800) {
        let chunk: String = chars[start..(start + 1000).min(chars.len())]
            .iter()
            .collect();
        memory::validate_fact("evolution.content", &chunk)?;
    }
    Ok(())
}
pub(crate) fn plain(path: &Path) -> Result<()> {
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        let metadata = fs::symlink_metadata(path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "symlinks are not evolution territory"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                metadata.file_attributes() & 0x400 == 0,
                "reparse points are not evolution territory"
            );
        }
    }
    Ok(())
}
pub(crate) fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    plain(path)?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temp, path)?;
    Ok(())
}

/// Stream records from an exact JSONL byte boundary. A torn final line is an
/// error, never a checkpoint or an invitation to overwrite user data.
pub(crate) struct ExperienceReader {
    reader: std::io::BufReader<fs::File>,
    offset: u64,
    end: u64,
}
impl Iterator for ExperienceReader {
    type Item = Result<(crate::Experience, u64, u64)>;
    fn next(&mut self) -> Option<Self::Item> {
        use std::io::BufRead;
        if self.offset >= self.end {
            return None;
        }
        let start = self.offset;
        let mut line = String::new();
        let result = (|| {
            let length = self.reader.read_line(&mut line)?;
            ensure!(
                length > 0 && line.ends_with('\n'),
                "incomplete experience JSONL record at {start}"
            );
            self.offset += u64::try_from(length)?;
            ensure!(self.offset <= self.end, "invalid experience boundary");
            Ok((serde_json::from_str(&line)?, start, self.offset))
        })();
        if result.is_err() {
            self.offset = self.end;
        }
        Some(result)
    }
}
pub(crate) fn read_experiences(path: &Path, start: u64) -> Result<ExperienceReader> {
    plain(path)?;
    if !path.exists() {
        ensure!(start == 0, "missing experience stream");
        // Creating an empty authoritative stream is harmless, and keeps the
        // reader implementation independent of optional file handles.
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
    }
    let file = fs::File::open(path)?;
    let end = file.metadata()?.len();
    read_experiences_file(file, start, end)
}
fn read_experiences_file(mut file: fs::File, start: u64, end: u64) -> Result<ExperienceReader> {
    use std::io::{Read, Seek, SeekFrom};
    ensure!(
        start <= end && end <= file.metadata()?.len(),
        "missing experience bytes"
    );
    if start > 0 {
        file.seek(SeekFrom::Start(start - 1))?;
        let mut previous = [0];
        file.read_exact(&mut previous)?;
        ensure!(previous[0] == b'\n', "cursor is not an experience boundary");
    }
    file.seek(SeekFrom::Start(start))?;
    Ok(ExperienceReader {
        reader: std::io::BufReader::new(file),
        offset: start,
        end,
    })
}
pub(crate) fn read_experiences_range(
    path: &Path,
    start: u64,
    end: u64,
) -> Result<ExperienceReader> {
    plain(path)?;
    read_experiences_file(fs::File::open(path)?, start, end)
}
pub(crate) fn append_experience(path: &Path, experience: &crate::Experience) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    plain(path)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0];
        file.read_exact(&mut last)?;
        ensure!(
            last[0] == b'\n',
            "incomplete experience tail; restore before appending"
        );
    }
    let mut bytes = serde_json::to_vec(experience)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Distinguish a failed Memory operation from a rejected semantic proposal.
#[derive(Debug)]
pub(crate) struct PersistenceFailure;
impl std::fmt::Display for PersistenceFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("evolution persistence failed")
    }
}
impl std::error::Error for PersistenceFailure {}
