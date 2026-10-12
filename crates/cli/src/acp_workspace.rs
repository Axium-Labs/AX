//! Read-only workspace discovery for ACP clients, on the AX host itself.
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;

pub(crate) fn listing(params: &Value, boundary: Option<&std::path::Path>) -> Result<Value> {
    let requested = params["cwd"].as_str().filter(|path| !path.is_empty());
    let path = requested
        .map(PathBuf::from)
        .or_else(|| boundary.map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map_or_else(|| PathBuf::from("."), PathBuf::from)
        });
    let path = path.canonicalize()?;
    let boundary = boundary.map(std::path::Path::canonicalize).transpose()?;
    if boundary
        .as_ref()
        .is_some_and(|root| !path.starts_with(root))
    {
        bail!("workspace is outside the ACP process workspace");
    }
    if !path.is_dir() {
        bail!("workspace is not a directory");
    }
    let mut directories = Vec::new();
    for entry in std::fs::read_dir(&path)? {
        let Ok(entry) = entry else { continue };
        if entry.path().is_dir()
            && boundary.as_ref().is_none_or(|root| {
                entry
                    .path()
                    .canonicalize()
                    .is_ok_and(|p| p.starts_with(root))
            })
        {
            directories.push(json!({"name":entry.file_name().to_string_lossy(),"path":entry.path().to_string_lossy()}));
        }
    }
    directories.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(
        json!({"cwd":path.to_string_lossy(),"parent":path.parent().filter(|p|boundary.as_ref().is_none_or(|root|p.starts_with(root))).map(|p|p.to_string_lossy()),"directories":directories}),
    )
}

#[cfg(test)]
#[path = "../../../test/acp_workspace.rs"]
mod tests;
