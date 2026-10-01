//! Presentation-only Git snapshots. Never applies, stages or reverts changes.
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub fn snapshot(cwd: &Path) -> BTreeMap<String, (Value, Vec<u8>)> {
    let mut files = BTreeMap::new();
    let Ok(output) = std::process::Command::new("git")
        .args(["diff", "--no-ext-diff", "--numstat", "HEAD"])
        .current_dir(cwd)
        .output()
    else {
        return files;
    };
    if !output.status.success() {
        return files;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<_> = line.splitn(3, '\t').collect();
        if parts.len() != 3 {
            continue;
        }
        let path = parts[2];
        let contents = std::fs::read(cwd.join(path)).unwrap_or_default();
        let diff = std::process::Command::new("git")
            .args(["diff", "--no-ext-diff", "--no-color", "HEAD", "--", path])
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
        files.insert(path.to_owned(),(json!({"path":path,"additions":parts[0].parse::<usize>().unwrap_or(0),"deletions":parts[1].parse::<usize>().unwrap_or(0),"diff":diff}),contents));
    }
    if let Ok(output) = std::process::Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(cwd)
        .output()
    {
        for path in String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|path| !path.starts_with(".ax/"))
        {
            if let Ok(contents) = std::fs::read(cwd.join(path)) {
                let additions = contents
                    .split(|byte| *byte == b'\n')
                    .count()
                    .saturating_sub(usize::from(contents.ends_with(b"\n")));
                files.insert(
                    path.to_owned(),
                    (
                        json!({"path":path,"additions":additions,"deletions":0,"diff":if contents.contains(&0) { None } else { Some(format!("--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{additions} @@\n{}", String::from_utf8_lossy(&contents).lines().map(|line| format!("+{line}\n")).collect::<String>())) }}),
                        contents,
                    ),
                );
            }
        }
    }
    files
}

pub fn changed(
    before: &BTreeMap<String, (Value, Vec<u8>)>,
    after: BTreeMap<String, (Value, Vec<u8>)>,
) -> Vec<Value> {
    after
        .into_iter()
        .filter(|(path, value)| before.get(path) != Some(value))
        .map(|(_, (value, _))| value)
        .collect()
}
