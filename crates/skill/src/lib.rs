//! Metadata-first skill discovery, routing, and lazy instruction loading.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use thiserror::Error;

const MANIFEST_NAME: &str = "skill.toml";
const INSTRUCTIONS_NAME: &str = "instructions.md";

#[derive(Debug, Error)]
pub enum SkillError {
    #[error("failed to access skill path {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid skill manifest {path}: {source}")]
    Manifest {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("duplicate skill name '{name}' in {first} and {second}")]
    Duplicate {
        name: String,
        first: PathBuf,
        second: PathBuf,
    },
    #[error("unknown skill: {0}")]
    Unknown(String),
    #[error("skill '{0}' has empty instructions")]
    EmptyInstructions(String),
    #[error("invalid skill metadata in {path}: {message}")]
    InvalidMetadata { path: PathBuf, message: String },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub trigger_keywords: Vec<String>,
    #[serde(default)]
    pub required_tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedSkill {
    pub metadata: SkillMetadata,
    pub instructions: String,
}

#[derive(Clone, Debug)]
struct IndexedSkill {
    metadata: SkillMetadata,
    directory: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct SkillCatalog {
    skills: BTreeMap<String, IndexedSkill>,
}

impl SkillCatalog {
    /// Indexes immediate child directories by reading only their `skill.toml` files.
    /// A missing root is treated as an empty catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when a directory or manifest cannot be read, a manifest
    /// is invalid, or two manifests declare the same skill name.
    pub fn index(root: impl AsRef<Path>) -> Result<Self, SkillError> {
        let root = root.as_ref();
        if !root.exists() {
            return Ok(Self::default());
        }
        let entries = fs::read_dir(root).map_err(|source| SkillError::Io {
            path: root.to_owned(),
            source,
        })?;
        let mut catalog = Self::default();
        for entry in entries {
            let entry = entry.map_err(|source| SkillError::Io {
                path: root.to_owned(),
                source,
            })?;
            let directory = entry.path();
            let file_type = entry.file_type().map_err(|source| SkillError::Io {
                path: directory.clone(),
                source,
            })?;
            if !file_type.is_dir() {
                continue;
            }
            let manifest_path = directory.join(MANIFEST_NAME);
            if !manifest_path.is_file() {
                continue;
            }
            let manifest = fs::read_to_string(&manifest_path).map_err(|source| SkillError::Io {
                path: manifest_path.clone(),
                source,
            })?;
            let metadata = toml::from_str::<SkillMetadata>(&manifest).map_err(|source| {
                SkillError::Manifest {
                    path: manifest_path.clone(),
                    source,
                }
            })?;
            validate_metadata(&metadata, &manifest_path)?;
            if let Some(existing) = catalog.skills.get(&metadata.name) {
                return Err(SkillError::Duplicate {
                    name: metadata.name,
                    first: existing.directory.join(MANIFEST_NAME),
                    second: manifest_path,
                });
            }
            catalog.skills.insert(
                metadata.name.clone(),
                IndexedSkill {
                    metadata,
                    directory,
                },
            );
        }
        Ok(catalog)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Lists indexed metadata and reports unavailable tool dependencies.
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

    /// Selects the highest-scoring available skill without loading instructions.
    #[must_use]
    pub fn route<'a>(
        &self,
        input: &str,
        available_tools: impl IntoIterator<Item = &'a str>,
    ) -> Option<SkillMatch> {
        let input = input.to_lowercase();
        let available = available_tools.into_iter().collect::<HashSet<_>>();
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
                let score = skill
                    .metadata
                    .trigger_keywords
                    .iter()
                    .filter(|keyword| {
                        !keyword.trim().is_empty() && input.contains(&keyword.to_lowercase())
                    })
                    .count();
                (score > 0).then(|| SkillMatch {
                    name: skill.metadata.name.clone(),
                    score,
                })
            })
            .max_by(|left, right| {
                left.score
                    .cmp(&right.score)
                    .then_with(|| right.name.cmp(&left.name))
            })
    }

    /// Loads `instructions.md` for one already-indexed skill.
    ///
    /// # Errors
    ///
    /// Returns an error when the skill is unknown, instructions cannot be read,
    /// or the instruction file is empty.
    pub fn load(&self, name: &str) -> Result<LoadedSkill, SkillError> {
        let skill = self
            .skills
            .get(name)
            .ok_or_else(|| SkillError::Unknown(name.to_owned()))?;
        let path = skill.directory.join(INSTRUCTIONS_NAME);
        let instructions = fs::read_to_string(&path).map_err(|source| SkillError::Io {
            path: path.clone(),
            source,
        })?;
        let instructions = instructions.trim().to_owned();
        if instructions.is_empty() {
            return Err(SkillError::EmptyInstructions(name.to_owned()));
        }
        Ok(LoadedSkill {
            metadata: skill.metadata.clone(),
            instructions,
        })
    }
}

fn validate_metadata(metadata: &SkillMetadata, path: &Path) -> Result<(), SkillError> {
    if metadata.name.trim().is_empty() {
        return Err(SkillError::InvalidMetadata {
            path: path.to_owned(),
            message: "name must not be empty".to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempSkills {
        root: PathBuf,
    }

    impl TempSkills {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("ax-skills-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).expect("temporary skill root should be created");
            Self { root }
        }

        fn add(&self, directory: &str, manifest: &str, instructions: Option<&str>) {
            let path = self.root.join(directory);
            fs::create_dir(&path).expect("skill directory should be created");
            fs::write(path.join(MANIFEST_NAME), manifest).expect("manifest should be written");
            if let Some(instructions) = instructions {
                fs::write(path.join(INSTRUCTIONS_NAME), instructions)
                    .expect("instructions should be written");
            }
        }
    }

    impl Drop for TempSkills {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("temporary skills should be removed");
        }
    }

    #[test]
    fn indexes_metadata_without_reading_instructions() {
        let temp = TempSkills::new();
        temp.add(
            "coding",
            r#"
                name = "coding"
                description = "Code tasks"
                trigger_keywords = ["rust", "code"]
                required_tools = ["shell"]
            "#,
            None,
        );

        let catalog = SkillCatalog::index(&temp.root).expect("metadata index should succeed");
        assert_eq!(catalog.len(), 1);
        assert!(matches!(catalog.load("coding"), Err(SkillError::Io { .. })));
    }

    #[test]
    fn routes_only_skills_with_available_tools_then_loads_instructions() {
        let temp = TempSkills::new();
        temp.add(
            "coding",
            r#"
                name = "coding"
                description = "Code tasks"
                trigger_keywords = ["rust", "compile"]
                required_tools = ["shell", "filesystem"]
            "#,
            Some("Keep changes small."),
        );
        let catalog = SkillCatalog::index(&temp.root).expect("metadata index should succeed");

        assert!(catalog.route("compile rust", ["shell"]).is_none());
        let matched = catalog
            .route("compile this Rust project", ["shell", "filesystem"])
            .expect("coding skill should match");
        assert_eq!(matched.name, "coding");
        assert_eq!(matched.score, 2);
        assert_eq!(
            catalog
                .load(&matched.name)
                .expect("instructions should load")
                .instructions,
            "Keep changes small."
        );
    }
}
