//! Lazy project-file lookup for composer `@` references.

use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
};

pub fn files(root: &Path) -> Vec<String> {
    let mut pending = VecDeque::from([root.to_path_buf()]);
    let mut found = Vec::new();
    while let Some(dir) = pending.pop_front() {
        if found.len() >= 10_000 || pending.len() >= 10_000 {
            break;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !matches!(
                    name.as_ref(),
                    ".git" | ".ax" | "target" | "node_modules" | ".venv"
                ) {
                    pending.push_back(path);
                }
            } else if path.is_file()
                && let Ok(relative) = path.strip_prefix(root)
            {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

fn score(query: &str, path: &str) -> Option<usize> {
    let query = query.to_lowercase();
    let path_lower = path.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let file = path_lower.rsplit('/').next().unwrap_or(&path_lower);
    if file == query {
        return Some(0);
    }
    if file.starts_with(&query) {
        return Some(1);
    }
    if file.contains(&query) {
        return Some(2);
    }
    if path_lower.contains(&query) {
        return Some(3);
    }
    let mut chars = path_lower.chars();
    query
        .chars()
        .all(|wanted| chars.by_ref().any(|c| c == wanted))
        .then_some(4)
}

pub fn matches<'a>(query: &str, paths: &'a [String]) -> Vec<&'a str> {
    let mut ranked = paths
        .iter()
        .filter_map(|path| score(query, path).map(|rank| (rank, path.as_str())))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(rank, path)| (*rank, path.len(), *path));
    ranked.into_iter().take(8).map(|(_, path)| path).collect()
}

pub fn current_query(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = text.get(..cursor)?;
    let start = before.rfind('@')?;
    if start > 0 && !before[..start].chars().next_back()?.is_whitespace() {
        return None;
    }
    let query = &before[start + 1..];
    (!query.chars().any(char::is_whitespace)).then_some((start, query))
}

pub fn references(prompt: &str, paths: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = prompt;
    while let Some(index) = rest.find('@') {
        let boundary = index == 0
            || rest[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        rest = &rest[index + 1..];
        if !boundary {
            continue;
        }
        let (query, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let Some(end) = quoted.find('"') else {
                continue;
            };
            (&quoted[..end], &quoted[end + 1..])
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (
                rest[..end].trim_end_matches(&['.', ',', ';', ':', '!', '?'][..]),
                &rest[end..],
            )
        };
        rest = after;
        if query.is_empty() {
            continue;
        }
        if let Some(path) = paths
            .iter()
            .find(|path| path.as_str() == query)
            .map(String::as_str)
            .or_else(|| matches(query, paths).first().copied())
            && !found.iter().any(|existing| existing == path)
        {
            found.push(path.to_owned());
        }
    }
    found
}

pub fn read(root: &Path, relative: &str) -> Option<String> {
    let root = fs::canonicalize(root).ok()?;
    let path = fs::canonicalize(root.join(PathBuf::from(relative))).ok()?;
    if !path.starts_with(&root) || fs::metadata(&path).ok()?.len() > 256_000 {
        return None;
    }
    fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_reference_finds_readme() {
        let paths = vec!["docs/README.md".to_owned(), "README.md".to_owned()];
        assert_eq!(matches("README", &paths)[0], "README.md");
        assert_eq!(references("check @README", &paths), vec!["README.md"]);
        assert_eq!(current_query("check @README", 13), Some((6, "README")));
        let spaced = vec!["docs/My Guide.md".to_owned()];
        assert_eq!(references("read @\"docs/My Guide.md\"", &spaced), spaced);
    }

    #[test]
    fn reads_only_files_inside_project() {
        let root = std::env::temp_dir().join(format!("ax-file-ref-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("README.md"), "project content").unwrap();
        assert_eq!(read(&root, "README.md").as_deref(), Some("project content"));
        assert!(read(&root, "../outside.txt").is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
