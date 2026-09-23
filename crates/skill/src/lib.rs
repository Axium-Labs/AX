//! Metadata-first Agent Skills discovery, routing, and lazy body loading.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

const STANDARD: &str = "SKILL.md";
const LEGACY_MANIFEST: &str = "skill.toml";
const LEGACY_BODY: &str = "instructions.md";
const MAX_FRONTMATTER_BYTES: usize = 64_000;

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("failed to access skill path {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid legacy skill manifest {path}: {source}")]
    Manifest {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("duplicate skill '{name}': keeping {first}, ignoring {second}")]
    Duplicate {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },
    #[error("unknown skill: {0}")]
    Unknown(String),
    #[error("skill '{0}' has empty instructions")]
    EmptyInstructions(String),
    #[error("invalid SKILL.md in {path}: {message}")]
    Standard { path: PathBuf, message: String },
}

/// Standard frontmatter. Unknown extension fields are retained.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Option<String>,
    #[serde(default, flatten)]
    pub extensions: BTreeMap<String, serde_yaml::Value>,
    #[serde(skip)]
    pub required_tools: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyMetadata {
    name: String,
    description: String,
    #[serde(default)]
    required_tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkillStatus {
    pub metadata: SkillMetadata,
    pub missing_tools: Vec<String>,
}
impl SkillStatus {
    #[must_use]
    pub fn available(&self) -> bool {
        self.missing_tools.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillMatch {
    pub name: String,
    pub score: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoadedSkill {
    pub metadata: SkillMetadata,
    pub instructions: String,
    pub directory: PathBuf,
}

#[derive(Clone, Copy, Debug)]
pub enum ResourceKind {
    Scripts,
    References,
    Assets,
}
impl ResourceKind {
    const fn directory(self) -> &'static str {
        match self {
            Self::Scripts => "scripts",
            Self::References => "references",
            Self::Assets => "assets",
        }
    }
}

#[derive(Clone, Debug)]
struct IndexedSkill {
    metadata: SkillMetadata,
    directory: PathBuf,
    standard: bool,
}

#[derive(Debug, Default)]
pub struct SkillCatalog {
    skills: BTreeMap<String, IndexedSkill>,
    issues: Vec<SkillError>,
}
impl SkillCatalog {
    /// Index one root. A missing root is an empty catalog.
    ///
    /// # Errors
    /// Returns an error only when the root itself cannot be enumerated.
    pub fn index(root: impl AsRef<Path>) -> Result<Self, SkillError> {
        Self::index_sources([root.as_ref()])
    }

    /// Earlier roots win duplicate names; entries within a root are sorted.
    /// Bad packages are reported without preventing other skills from loading.
    ///
    /// # Errors
    /// Returns an error only when a root cannot be enumerated.
    pub fn index_sources<P: AsRef<Path>>(
        roots: impl IntoIterator<Item = P>,
    ) -> Result<Self, SkillError> {
        let mut catalog = Self::default();
        let mut seen_roots = HashSet::new();
        for root in roots {
            let root = root.as_ref();
            if !root.exists() {
                continue;
            }
            let canonical = fs::canonicalize(root).map_err(|source| SkillError::Io {
                path: root.to_owned(),
                source,
            })?;
            if !seen_roots.insert(canonical) {
                continue;
            }
            let entries = fs::read_dir(root).map_err(|source| SkillError::Io {
                path: root.to_owned(),
                source,
            })?;
            let mut directories = Vec::new();
            for entry in entries {
                match entry {
                    Ok(entry) => directories.push(entry.path()),
                    Err(source) => catalog.issues.push(SkillError::Io {
                        path: root.to_owned(),
                        source,
                    }),
                }
            }
            directories.sort();
            for directory in directories {
                if !directory.is_dir() {
                    continue;
                }
                match Self::index_directory(&directory) {
                    Ok(Some(skill)) => {
                        if let Some(first) = catalog.skills.get(&skill.metadata.name) {
                            catalog.issues.push(SkillError::Duplicate {
                                name: skill.metadata.name.clone(),
                                first: first.directory.clone(),
                                second: directory,
                            });
                        } else {
                            catalog.skills.insert(skill.metadata.name.clone(), skill);
                        }
                    }
                    Ok(None) => (),
                    Err(error) => catalog.issues.push(error),
                }
            }
        }
        Ok(catalog)
    }

    fn index_directory(directory: &Path) -> Result<Option<IndexedSkill>, SkillError> {
        let standard_path = directory.join(STANDARD);
        let legacy_path = directory.join(LEGACY_MANIFEST);
        let standard = standard_path.exists();
        let metadata = if standard {
            let frontmatter = read_frontmatter_only(&standard_path)?;
            let metadata = parse_standard_metadata(&frontmatter, &standard_path)?;
            validate_standard_metadata(&metadata, directory, &standard_path)?;
            metadata
        } else if legacy_path.exists() {
            let source = fs::read_to_string(&legacy_path).map_err(|source| SkillError::Io {
                path: legacy_path.clone(),
                source,
            })?;
            let legacy: LegacyMetadata =
                toml::from_str(&source).map_err(|source| SkillError::Manifest {
                    path: legacy_path.clone(),
                    source,
                })?;
            if legacy.name.trim().is_empty() || legacy.description.trim().is_empty() {
                return Err(SkillError::Standard {
                    path: legacy_path,
                    message: "legacy name and description must be nonempty".into(),
                });
            }
            SkillMetadata {
                name: legacy.name,
                description: legacy.description,
                license: None,
                compatibility: None,
                metadata: BTreeMap::new(),
                allowed_tools: None,
                extensions: BTreeMap::new(),
                required_tools: legacy.required_tools,
            }
        } else {
            return Ok(None);
        };
        Ok(Some(IndexedSkill {
            metadata,
            directory: directory.to_owned(),
            standard,
        }))
    }

    #[must_use]
    pub fn issues(&self) -> &[SkillError] {
        &self.issues
    }
    #[must_use]
    pub fn directory(&self, name: &str) -> Option<&Path> {
        self.skills.get(name).map(|skill| skill.directory.as_path())
    }
    #[must_use]
    pub fn instruction_path(&self, name: &str) -> Option<PathBuf> {
        self.skills.get(name).map(|skill| {
            skill.directory.join(if skill.standard {
                STANDARD
            } else {
                LEGACY_BODY
            })
        })
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }
    #[must_use]
    pub fn statuses<'a>(
        &self,
        available_tools: impl IntoIterator<Item = &'a str>,
    ) -> Vec<SkillStatus> {
        let available = available_tools.into_iter().collect::<HashSet<_>>();
        self.skills
            .values()
            .map(|skill| SkillStatus {
                metadata: skill.metadata.clone(),
                missing_tools: skill
                    .metadata
                    .required_tools
                    .iter()
                    .filter(|tool| !available.contains(tool.as_str()))
                    .cloned()
                    .collect(),
            })
            .collect()
    }

    /// Rank by name and description. Legacy triggers and allowed-tools do not affect routing.
    #[must_use]
    pub fn route_candidates<'a>(
        &self,
        input: &str,
        available_tools: impl IntoIterator<Item = &'a str>,
    ) -> Vec<SkillMatch> {
        let available = available_tools.into_iter().collect::<HashSet<_>>();
        let input_terms = terms(input);
        let lowered_input = input.to_lowercase();
        let mut matches =
            self.skills
                .values()
                .filter(|skill| {
                    skill
                        .metadata
                        .required_tools
                        .iter()
                        .all(|tool| available.contains(tool.as_str()))
                })
                .filter_map(|skill| {
                    let name = &skill.metadata.name;
                    let name_overlap = terms(name).intersection(&input_terms).count();
                    let description_overlap = terms(&skill.metadata.description)
                        .intersection(&input_terms)
                        .count();
                    let score = usize::from(lowered_input.contains(name)) * 12
                        + name_overlap * 5
                        + description_overlap * 2;
                    (lowered_input.contains(name) || name_overlap > 0 || description_overlap >= 2)
                        .then(|| SkillMatch {
                            name: name.clone(),
                            score,
                        })
                })
                .collect::<Vec<_>>();
        matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
        matches
    }
    #[must_use]
    pub fn route<'a>(
        &self,
        input: &str,
        available_tools: impl IntoIterator<Item = &'a str>,
    ) -> Option<SkillMatch> {
        self.route_candidates(input, available_tools)
            .into_iter()
            .next()
    }

    /// Read only the selected skill's body.
    ///
    /// # Errors
    /// Returns an error for a missing, changed, or empty body.
    pub fn load(&self, name: &str) -> Result<LoadedSkill, SkillError> {
        let skill = self
            .skills
            .get(name)
            .ok_or_else(|| SkillError::Unknown(name.to_owned()))?;
        let path = skill.directory.join(if skill.standard {
            STANDARD
        } else {
            LEGACY_BODY
        });
        let source = fs::read_to_string(&path).map_err(|source| SkillError::Io {
            path: path.clone(),
            source,
        })?;
        let instructions = if skill.standard {
            let (frontmatter, body) = split_frontmatter(&source, &path)?;
            let metadata = parse_standard_metadata(frontmatter, &path)?;
            validate_standard_metadata(&metadata, &skill.directory, &path)?;
            if metadata != skill.metadata {
                return Err(SkillError::Standard {
                    path,
                    message: "frontmatter changed since discovery; refresh the catalog".into(),
                });
            }
            body.trim().to_owned()
        } else {
            source.trim().to_owned()
        };
        if instructions.is_empty() {
            return Err(SkillError::EmptyInstructions(name.to_owned()));
        }
        Ok(LoadedSkill {
            metadata: skill.metadata.clone(),
            instructions,
            directory: skill.directory.clone(),
        })
    }

    /// List an optional resource directory only when needed.
    ///
    /// # Errors
    /// Returns an error when a present directory cannot be read.
    pub fn resources(&self, name: &str, kind: ResourceKind) -> Result<Vec<PathBuf>, SkillError> {
        let root = self
            .directory(name)
            .ok_or_else(|| SkillError::Unknown(name.to_owned()))?
            .join(kind.directory());
        if !root.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&root).map_err(|source| SkillError::Io {
            path: root.clone(),
            source,
        })?;
        let mut paths = Vec::new();
        for entry in entries {
            paths.push(
                entry
                    .map_err(|source| SkillError::Io {
                        path: root.clone(),
                        source,
                    })?
                    .path(),
            );
        }
        paths.sort();
        Ok(paths)
    }
}

/// Validate an entire standard package before installation, without executing resources.
///
/// # Errors
/// Returns an explicit error for missing or malformed content.
pub fn validate_skill_directory(directory: &Path) -> Result<LoadedSkill, SkillError> {
    let path = directory.join(STANDARD);
    let source = fs::read_to_string(&path).map_err(|source| SkillError::Io {
        path: path.clone(),
        source,
    })?;
    let (frontmatter, body) = split_frontmatter(&source, &path)?;
    let metadata = parse_standard_metadata(frontmatter, &path)?;
    validate_standard_metadata(&metadata, directory, &path)?;
    if body.trim().is_empty() {
        return Err(SkillError::EmptyInstructions(metadata.name));
    }
    Ok(LoadedSkill {
        metadata,
        instructions: body.trim().to_owned(),
        directory: directory.to_owned(),
    })
}

/// Create a minimal standard Agent Skill package without overwriting files.
///
/// # Errors
/// Returns an error for invalid metadata, empty instructions, or failed I/O.
pub fn create_skill_directory(
    root: &Path,
    name: &str,
    description: &str,
    instructions: &str,
) -> Result<PathBuf, SkillError> {
    let directory = root.join(name);
    let path = directory.join(STANDARD);
    let metadata = SkillMetadata {
        name: name.to_owned(),
        description: description.to_owned(),
        license: None,
        compatibility: None,
        metadata: BTreeMap::new(),
        allowed_tools: None,
        extensions: BTreeMap::new(),
        required_tools: Vec::new(),
    };
    validate_standard_metadata(&metadata, &directory, &path)?;
    if instructions.trim().is_empty() {
        return Err(SkillError::EmptyInstructions(name.to_owned()));
    }
    if directory.exists() {
        return Err(SkillError::Standard {
            path: directory,
            message: "destination already exists".into(),
        });
    }
    let frontmatter = serde_yaml::to_string(&BTreeMap::from([
        ("name", name),
        ("description", description),
    ]))
    .map_err(|error| SkillError::Standard {
        path: path.clone(),
        message: error.to_string(),
    })?;
    fs::create_dir_all(root).map_err(|source| SkillError::Io {
        path: root.to_owned(),
        source,
    })?;
    fs::create_dir(&directory).map_err(|source| SkillError::Io {
        path: directory.clone(),
        source,
    })?;
    let content = format!("---\n{frontmatter}---\n{}\n", instructions.trim());
    if let Err(source) = fs::write(&path, content) {
        let _ = fs::remove_dir(&directory);
        return Err(SkillError::Io { path, source });
    }
    Ok(directory)
}

/// Install a validated standard package unchanged, or migrate a legacy AX
/// package to `SKILL.md`. Existing destinations are never overwritten.
///
/// # Errors
/// Returns an error for invalid content, a conflicting destination, or failed I/O.
pub fn install_skill_directory(
    source: &Path,
    destination_root: &Path,
) -> Result<PathBuf, SkillError> {
    let standard = source.join(STANDARD).exists();
    let (name, converted) = if standard {
        (validate_skill_directory(source)?.metadata.name, None)
    } else {
        let (name, converted) = convert_legacy(source)?;
        (name, Some(converted))
    };
    let destination = destination_root.join(&name);
    if destination.exists() {
        return Err(SkillError::Standard {
            path: destination,
            message: "destination already exists".into(),
        });
    }
    // Validate the name and body before creating destination files.
    let validation_path = destination.join(STANDARD);
    let content = if let Some(content) = converted.as_deref() {
        let (frontmatter, body) = split_frontmatter(content, &validation_path)?;
        let metadata = parse_standard_metadata(frontmatter, &validation_path)?;
        validate_standard_metadata(&metadata, &destination, &validation_path)?;
        if body.trim().is_empty() {
            return Err(SkillError::EmptyInstructions(name));
        }
        Some(content)
    } else {
        None
    };
    fs::create_dir_all(destination_root).map_err(|source| SkillError::Io {
        path: destination_root.to_owned(),
        source,
    })?;
    let source_canonical = fs::canonicalize(source).map_err(|source_error| SkillError::Io {
        path: source.to_owned(),
        source: source_error,
    })?;
    let destination_root_canonical =
        fs::canonicalize(destination_root).map_err(|source| SkillError::Io {
            path: destination_root.to_owned(),
            source,
        })?;
    if destination_root_canonical.starts_with(source_canonical) {
        return Err(SkillError::Standard {
            path: destination,
            message: "destination root cannot be inside source package".into(),
        });
    }
    fs::create_dir(&destination).map_err(|source| SkillError::Io {
        path: destination.clone(),
        source,
    })?;
    let result = copy_package(source, &destination, !standard).and_then(|()| {
        if let Some(content) = content {
            fs::write(destination.join(STANDARD), content).map_err(|source| SkillError::Io {
                path: destination.join(STANDARD),
                source,
            })?;
        }
        validate_skill_directory(&destination).map(|_| ())
    });
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&destination);
        return Err(error);
    }
    Ok(destination)
}

fn convert_legacy(source: &Path) -> Result<(String, String), SkillError> {
    let path = source.join(LEGACY_MANIFEST);
    let manifest = fs::read_to_string(&path).map_err(|source| SkillError::Io {
        path: path.clone(),
        source,
    })?;
    let legacy: LegacyMetadata =
        toml::from_str(&manifest).map_err(|source| SkillError::Manifest {
            path: path.clone(),
            source,
        })?;
    let body_path = source.join(LEGACY_BODY);
    let body = fs::read_to_string(&body_path).map_err(|source| SkillError::Io {
        path: body_path,
        source,
    })?;
    let mut metadata = BTreeMap::new();
    if !legacy.required_tools.is_empty() {
        metadata.insert("ax.required-tools", legacy.required_tools.join(" "));
    }
    let frontmatter = serde_yaml::to_string(&serde_yaml::Mapping::from_iter([
        (
            serde_yaml::Value::from("name"),
            serde_yaml::Value::from(legacy.name.as_str()),
        ),
        (
            serde_yaml::Value::from("description"),
            serde_yaml::Value::from(legacy.description.as_str()),
        ),
        (
            serde_yaml::Value::from("metadata"),
            serde_yaml::to_value(metadata).map_err(|error| SkillError::Standard {
                path: path.clone(),
                message: error.to_string(),
            })?,
        ),
    ]))
    .map_err(|error| SkillError::Standard {
        path,
        message: error.to_string(),
    })?;
    Ok((
        legacy.name,
        format!("---\n{frontmatter}---\n{}\n", body.trim()),
    ))
}

fn copy_package(source: &Path, destination: &Path, legacy: bool) -> Result<(), SkillError> {
    for entry in fs::read_dir(source).map_err(|source_error| SkillError::Io {
        path: source.to_owned(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| SkillError::Io {
            path: source.to_owned(),
            source: source_error,
        })?;
        let source_path = entry.path();
        let target_path = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(|source_error| SkillError::Io {
            path: source_path.clone(),
            source: source_error,
        })?;
        if kind.is_symlink() {
            return Err(SkillError::Standard {
                path: source_path,
                message: "symlinked package resources are unsupported".into(),
            });
        }
        if legacy
            && matches!(
                entry.file_name().to_str(),
                Some(LEGACY_MANIFEST | LEGACY_BODY)
            )
        {
            continue;
        }
        if kind.is_dir() {
            fs::create_dir(&target_path).map_err(|source| SkillError::Io {
                path: target_path.clone(),
                source,
            })?;
            copy_package(&source_path, &target_path, false)?;
        } else {
            fs::copy(&source_path, &target_path).map_err(|source| SkillError::Io {
                path: target_path,
                source,
            })?;
        }
    }
    Ok(())
}

fn read_frontmatter_only(path: &Path) -> Result<String, SkillError> {
    let file = fs::File::open(path).map_err(|source| SkillError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut frontmatter = String::new();
    let mut first = true;
    loop {
        let mut line = String::new();
        let count = reader
            .read_line(&mut line)
            .map_err(|source| SkillError::Io {
                path: path.to_owned(),
                source,
            })?;
        if count == 0 || frontmatter.len() + count > MAX_FRONTMATTER_BYTES {
            return Err(SkillError::Standard {
                path: path.to_owned(),
                message: "frontmatter is unclosed or exceeds 64 KB".into(),
            });
        }
        if first {
            first = false;
            if line.trim_start_matches('\u{feff}').trim_end() != "---" {
                return Err(SkillError::Standard {
                    path: path.to_owned(),
                    message: "missing opening YAML frontmatter delimiter".into(),
                });
            }
        } else if line.trim_end() == "---" {
            return Ok(frontmatter);
        } else {
            frontmatter.push_str(&line);
        }
    }
}

fn split_frontmatter<'a>(source: &'a str, path: &Path) -> Result<(&'a str, &'a str), SkillError> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut lines = source.split_inclusive('\n');
    let opening = lines.next().unwrap_or_default();
    if opening.trim_end() != "---" {
        return Err(SkillError::Standard {
            path: path.to_owned(),
            message: "missing opening YAML frontmatter delimiter".into(),
        });
    }
    let mut offset = opening.len();
    for line in lines {
        if line.trim_end() == "---" {
            return Ok((
                &source[opening.len()..offset],
                &source[offset + line.len()..],
            ));
        }
        offset += line.len();
    }
    Err(SkillError::Standard {
        path: path.to_owned(),
        message: "unclosed YAML frontmatter".into(),
    })
}

fn parse_standard_metadata(source: &str, path: &Path) -> Result<SkillMetadata, SkillError> {
    let mut metadata: SkillMetadata =
        serde_yaml::from_str(source).map_err(|error| SkillError::Standard {
            path: path.to_owned(),
            message: error.to_string(),
        })?;
    if let Some(required) = metadata.metadata.get("ax.required-tools") {
        metadata.required_tools = required.split_whitespace().map(str::to_owned).collect();
    }
    Ok(metadata)
}

fn validate_standard_metadata(
    metadata: &SkillMetadata,
    directory: &Path,
    path: &Path,
) -> Result<(), SkillError> {
    let name = &metadata.name;
    let valid_name = (1..=64).contains(&name.chars().count())
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid_name {
        return Err(SkillError::Standard { path: path.to_owned(), message: "name must be 1-64 lowercase ASCII letters, digits, or single hyphens, without leading/trailing hyphens".into() });
    }
    if directory
        .file_name()
        .is_none_or(|folder| folder != name.as_str())
    {
        return Err(SkillError::Standard {
            path: path.to_owned(),
            message: format!("name '{name}' must match parent directory"),
        });
    }
    if metadata.description.trim().is_empty() || metadata.description.chars().count() > 1024 {
        return Err(SkillError::Standard {
            path: path.to_owned(),
            message: "description must be 1-1024 characters".into(),
        });
    }
    if metadata
        .compatibility
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 500)
    {
        return Err(SkillError::Standard {
            path: path.to_owned(),
            message: "compatibility must be 1-500 characters".into(),
        });
    }
    if metadata
        .allowed_tools
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(SkillError::Standard {
            path: path.to_owned(),
            message: "allowed-tools must be a nonempty space-separated string".into(),
        });
    }
    Ok(())
}

fn terms(input: &str) -> HashSet<String> {
    const STOP: &[&str] = &[
        "a", "an", "and", "the", "to", "for", "of", "on", "in", "or", "with", "when", "use",
        "using", "this", "that", "from", "into", "can", "is", "are", "do", "does", "user", "users",
        "skill",
    ];
    input
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter_map(|word| {
            let stem = if word.len() > 5 && word.ends_with("ing") {
                word.trim_end_matches("ing")
            } else {
                word.trim_end_matches('s')
            };
            (stem.chars().count() >= 2 && !STOP.contains(&stem)).then(|| stem.to_owned())
        })
        .collect()
}
