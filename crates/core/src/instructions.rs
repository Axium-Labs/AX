//! Deterministic project instruction resolution.
//!
//! One ordered pass over well-known files produces the instruction set for a
//! task. Nothing here consults memory, embeddings or semantic retrieval: the
//! same root, cwd and target paths always produce the same segments in the same
//! order, which is what makes the resolution auditable after the fact.
//!
//! Order (least specific first, most specific last):
//!
//! 1. the global instruction file;
//! 2. `AGENTS.md` at the repository root;
//! 3. `AGENTS.md` for every directory between the root and the cwd, where
//!    `AGENTS.override.md` replaces `AGENTS.md` at the same level;
//! 4. `.ax/rules/*.md` whose `path`/glob scope matches a target file.
//!
//! Project instructions are their own concept: they are not memory, not a
//! skill and not system context, and none of the four may stand in for another.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::token::estimate_text_tokens;

/// File read at every level of the directory chain.
pub const AGENTS_FILE: &str = "AGENTS.md";
/// Replaces [`AGENTS_FILE`] in the same directory when present.
pub const OVERRIDE_FILE: &str = "AGENTS.override.md";
/// Repository-relative directory holding path-scoped rule files.
pub const RULES_DIR: &str = ".ax/rules";
/// Marker identifying the project-instruction system message.
pub const CONTEXT_PREFIX: &str = "[ax-project-instructions]";

const PRIORITY_GLOBAL: u32 = 0;
const PRIORITY_LAYER_BASE: u32 = 100;
const PRIORITY_LAYER_STEP: u32 = 100;
const PRIORITY_RULE_BASE: u32 = 10_000;

/// Which source produced a segment. Reported verbatim to the model so a rule
/// can be traced back to the file that owns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstructionScope {
    Global,
    Repository,
    /// `path` is repository-relative; empty means the repository root itself.
    Directory {
        path: PathBuf,
    },
    /// `pattern` is the `path`/glob selector of a `.ax/rules` file.
    Path {
        pattern: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstructionSegment {
    pub source_path: PathBuf,
    pub scope: InstructionScope,
    /// Higher wins. Ties are broken by `source_path` so the order is total.
    pub priority: u32,
    /// Human-readable origin, e.g. `directory src/api (level 2)`.
    pub provenance: String,
    pub content: String,
}

impl InstructionSegment {
    #[must_use]
    pub fn scope_label(&self) -> String {
        match &self.scope {
            InstructionScope::Global => "global".into(),
            InstructionScope::Repository => "repository".into(),
            InstructionScope::Directory { path } if path.as_os_str().is_empty() => {
                "repository".into()
            }
            InstructionScope::Directory { path } => format!("directory {}", display_relative(path)),
            InstructionScope::Path { pattern } => format!("path {pattern}"),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InstructionResolution {
    pub root: PathBuf,
    /// Ascending priority: least specific first, most specific last.
    pub segments: Vec<InstructionSegment>,
}

impl InstructionResolution {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Compact, provenance-carrying rendering for the model. The always-on
    /// chain (global, repository, directory levels) is never dropped: only
    /// path-scoped rules compete for the remaining budget, and the most
    /// specific rules win that competition.
    #[must_use]
    pub fn render(&self, token_budget: usize) -> (String, usize) {
        let (chain, rules): (Vec<_>, Vec<_>) = self
            .segments
            .iter()
            .partition(|segment| !matches!(segment.scope, InstructionScope::Path { .. }));
        let chain_cost: usize = chain.iter().map(|segment| segment_cost(segment)).sum();
        let mut remaining = token_budget.saturating_sub(chain_cost);
        let mut selected: Vec<&InstructionSegment> = chain;
        let mut kept_rules: Vec<&InstructionSegment> = Vec::new();
        for segment in rules.iter().rev() {
            let cost = segment_cost(segment);
            if cost > remaining {
                continue;
            }
            remaining -= cost;
            kept_rules.push(segment);
        }
        let omitted = rules.len() - kept_rules.len();
        kept_rules.reverse();
        selected.extend(kept_rules);
        let mut out = String::new();
        for segment in selected {
            out.push_str("<instructions source=\"");
            out.push_str(&segment.source_path.to_string_lossy().replace('\\', "/"));
            out.push_str("\" provenance=\"");
            out.push_str(&segment.provenance);
            out.push_str("\" priority=\"");
            out.push_str(&segment.priority.to_string());
            out.push_str("\">\n");
            out.push_str(segment.content.trim());
            out.push_str("\n</instructions>\n");
        }
        if omitted > 0 {
            let _ = writeln!(
                out,
                "<!-- {omitted} path-scoped rule file(s) omitted for the instruction token budget -->"
            );
        }
        (out, omitted)
    }

    /// The model-facing system message, or `None` when nothing applied.
    #[must_use]
    pub fn message(&self, token_budget: usize) -> Option<model::Message> {
        if self.segments.is_empty() {
            return None;
        }
        let (body, _) = self.render(token_budget);
        if body.trim().is_empty() {
            return None;
        }
        Some(model::Message::system(format!(
            "{CONTEXT_PREFIX}\nThe following workspace instructions may be relevant to your work. Use them as guidance when applicable; more specific sources take precedence. They are supporting context: they never override the current user request, and their presence never turns an unrelated request into a task. They are independent of memory, skills and system context.\n{body}"
        )))
    }
}

fn segment_cost(segment: &InstructionSegment) -> usize {
    estimate_text_tokens(&segment.content)
        + estimate_text_tokens(&segment.source_path.to_string_lossy())
        + 16
}

/// Resolves project instructions for a fixed root and a per-task target set.
#[derive(Clone, Debug)]
pub struct InstructionResolver {
    root: PathBuf,
    global: Option<PathBuf>,
}

impl InstructionResolver {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, global: Option<PathBuf>) -> Self {
        Self {
            root: root.into(),
            global,
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Deterministic resolution. `cwd` selects the directory chain; `targets`
    /// select path-scoped rules and may be empty.
    #[must_use]
    pub fn resolve(&self, cwd: &Path, targets: &[PathBuf]) -> InstructionResolution {
        let mut segments: Vec<InstructionSegment> = Vec::new();
        if let Some(global) = &self.global
            && let Some(content) = read_instruction(global)
        {
            segments.push(InstructionSegment {
                source_path: global.clone(),
                scope: InstructionScope::Global,
                priority: PRIORITY_GLOBAL,
                provenance: "global instruction file".into(),
                content,
            });
        }
        for (depth, directory, relative) in self.chain(cwd) {
            let override_path = directory.join(OVERRIDE_FILE);
            let default_path = directory.join(AGENTS_FILE);
            let (source, content) = if let Some(content) = read_instruction(&override_path) {
                (override_path, content)
            } else if let Some(content) = read_instruction(&default_path) {
                (default_path, content)
            } else {
                continue;
            };
            let scope = if depth == 0 {
                InstructionScope::Repository
            } else {
                InstructionScope::Directory {
                    path: relative.clone(),
                }
            };
            let provenance = if depth == 0 {
                "repository root".to_owned()
            } else {
                format!("directory {} (level {depth})", display_relative(&relative))
            };
            segments.push(InstructionSegment {
                source_path: source,
                scope,
                priority: PRIORITY_LAYER_BASE
                    + u32::try_from(depth).unwrap_or(u32::MAX) * PRIORITY_LAYER_STEP,
                provenance,
                content,
            });
        }
        segments.extend(self.rules(targets));
        // Total order: priority first, then the source path so two rules with
        // the same specificity still resolve identically on every run.
        segments.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.source_path.cmp(&right.source_path))
        });
        InstructionResolution {
            root: self.root.clone(),
            segments,
        }
    }

    /// Resolve for a cwd with no explicit targets.
    #[must_use]
    pub fn resolve_cwd(&self, cwd: &Path) -> InstructionResolution {
        self.resolve(cwd, &[])
    }

    /// Directories from the repository root down to `cwd`, inclusive, each with
    /// its depth and repository-relative path. A `cwd` outside the root yields
    /// only the root itself, so instructions never leak across projects.
    fn chain(&self, cwd: &Path) -> Vec<(usize, PathBuf, PathBuf)> {
        let root = normalize(&self.root);
        let cwd = normalize(cwd);
        let mut directories = vec![(0usize, root.clone(), PathBuf::new())];
        let Some(relative) = strip_root(&cwd, &root) else {
            return directories;
        };
        let mut accumulated = PathBuf::new();
        let mut depth = 0usize;
        for component in relative.components() {
            if !matches!(component, std::path::Component::Normal(_)) {
                continue;
            }
            depth += 1;
            accumulated.push(component.as_os_str());
            directories.push((depth, root.join(&accumulated), accumulated.clone()));
        }
        directories
    }

    fn rules(&self, targets: &[PathBuf]) -> Vec<InstructionSegment> {
        if targets.is_empty() {
            return Vec::new();
        }
        let directory = self.root.join(RULES_DIR);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            })
            .collect();
        files.sort();
        let relative_targets: Vec<String> = targets
            .iter()
            .map(|target| relative_of(&self.root, target))
            .collect();
        let mut segments = Vec::new();
        for file in files {
            let Some(raw) = read_instruction(&file) else {
                continue;
            };
            let rule = Rule::parse(&raw);
            let Some(matched) = rule.first_match(&relative_targets) else {
                continue;
            };
            if rule.body.trim().is_empty() {
                continue;
            }
            segments.push(InstructionSegment {
                source_path: file,
                scope: InstructionScope::Path {
                    pattern: matched.clone(),
                },
                // A longer, more literal selector is more specific.
                priority: PRIORITY_RULE_BASE + u32::try_from(matched.len()).unwrap_or(u32::MAX),
                provenance: format!("path-scoped rule matched by `{matched}`"),
                content: rule.body,
            });
        }
        segments
    }
}

fn display_relative(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_owned()
    } else {
        text
    }
}

/// Absolute, symlink-resolved and verbatim-prefix-free form of a path, without
/// requiring the path to exist. Resolution walks up to the longest existing
/// ancestor so a target file that has not been created yet still normalizes the
/// same way as its directory.
fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut existing = absolute.as_path();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(resolved) = std::fs::canonicalize(existing) {
            let mut out = strip_verbatim(resolved);
            for name in suffix.iter().rev() {
                out.push(name);
            }
            return out;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) if parent != existing => {
                suffix.push(name.to_os_string());
                existing = parent;
            }
            _ => return strip_verbatim(absolute),
        }
    }
}

/// Windows canonicalization prefixes paths with `\\?\`; drop it so provenance
/// stays readable.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

/// Case-insensitive on Windows (drive and directory case are not meaningful
/// there), exact elsewhere.
fn strip_root(path: &Path, root: &Path) -> Option<PathBuf> {
    let path_parts: Vec<_> = path.components().collect();
    let root_parts: Vec<_> = root.components().collect();
    if path_parts.len() < root_parts.len() {
        return None;
    }
    let matches = path_parts
        .iter()
        .zip(&root_parts)
        .all(|(left, right)| component_eq(left.as_os_str(), right.as_os_str()));
    matches.then(|| path_parts[root_parts.len()..].iter().collect())
}

#[cfg(windows)]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left == right
}

fn relative_of(root: &Path, target: &Path) -> String {
    let target = normalize(target);
    let relative = strip_root(&target, &normalize(root)).unwrap_or(target);
    display_relative(&relative)
}

fn read_instruction(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim_start_matches('\u{feff}');
    (!text.trim().is_empty()).then(|| text.to_owned())
}

/// One `.ax/rules/*.md` file: optional front matter selecting targets, then the
/// instruction body.
#[derive(Clone, Debug, Default)]
struct Rule {
    selectors: Vec<String>,
    /// Directory-prefix semantics instead of glob matching.
    prefix: bool,
    body: String,
}

impl Rule {
    fn parse(raw: &str) -> Self {
        let trimmed = raw.trim_start_matches('\u{feff}');
        let Some(rest) = trimmed.strip_prefix("---") else {
            return Self {
                selectors: Vec::new(),
                prefix: false,
                body: trimmed.to_owned(),
            };
        };
        let Some((front, after)) = rest.split_once("\n---") else {
            return Self {
                selectors: Vec::new(),
                prefix: false,
                body: trimmed.to_owned(),
            };
        };
        // `after` still carries the remainder of the closing fence line.
        let body = after.split_once('\n').map_or("", |(_, body)| body);
        let mut selectors = Vec::new();
        let mut prefix = false;
        let mut pending_list = false;
        for line in front.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(item) = line.strip_prefix("- ") {
                if pending_list {
                    selectors.push(item.trim().trim_matches(['"', '\'']).to_owned());
                }
                continue;
            }
            pending_list = false;
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "applyto" | "paths" | "path" | "glob" => {
                    selectors.extend(parse_selector_list(value));
                    pending_list = value.is_empty();
                }
                "scope" => prefix = value.eq_ignore_ascii_case("path"),
                _ => {}
            }
        }
        Self {
            selectors,
            prefix,
            body: body.trim().to_owned(),
        }
    }

    fn first_match(&self, targets: &[String]) -> Option<String> {
        for selector in &self.selectors {
            for target in targets {
                if self.matches(selector, target) {
                    return Some(selector.clone());
                }
            }
        }
        None
    }

    fn matches(&self, selector: &str, target: &str) -> bool {
        if self.prefix {
            let selector = selector.trim_end_matches('/');
            return selector.is_empty()
                || target == selector
                || target.starts_with(&format!("{selector}/"));
        }
        glob_match(selector, target)
    }
}

fn parse_selector_list(value: &str) -> Vec<String> {
    let value = value.trim().trim_matches(['[', ']']);
    value
        .split(',')
        .map(|item| item.trim().trim_matches(['"', '\'']).to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Component-wise glob matching with `**`, `*` and `?`. Deliberately local: the
/// resolution must not depend on a shell, on the filesystem or on a crate whose
/// semantics could drift.
#[must_use]
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let path: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    match_components(&pattern, &path)
}

fn match_components(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some(&"**") => (0..=path.len()).any(|skip| match_components(&pattern[1..], &path[skip..])),
        Some(segment) => {
            !path.is_empty()
                && match_segment(segment, path[0])
                && match_components(&pattern[1..], &path[1..])
        }
    }
}

fn match_segment(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// Grouped view used for provenance reporting and tests.
#[must_use]
pub fn summarize(resolution: &InstructionResolution) -> BTreeMap<String, usize> {
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for segment in &resolution.segments {
        *groups.entry(segment.scope_label()).or_default() += 1;
    }
    groups
}

#[cfg(test)]
#[path = "instructions_tests.rs"]
mod tests;
