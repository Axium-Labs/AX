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
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)?;
    Ok(())
}
