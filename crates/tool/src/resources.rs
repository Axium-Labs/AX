//! Tools declare effects; the runtime owns scheduling and process-wide locks.
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    /// Unknown effects conflict with every resource, including other tools.
    All,
    Path(PathBuf),
    Named(String),
}

impl Resource {
    /// Resolve existing ancestors (including symlinks), then normalize missing
    /// components, so aliases for a file share the same lock before creation.
    #[must_use]
    pub fn path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        };
        let normalized = std::fs::canonicalize(&absolute).unwrap_or_else(|_| {
            let mut resolved = PathBuf::new();
            for component in absolute.components() {
                match component {
                    Component::CurDir => {}
                    Component::ParentDir => {
                        resolved.pop();
                    }
                    Component::Normal(name) => {
                        resolved.push(name);
                        // Resolve symlinks before a later `..`, including when
                        // the final file has not yet been created.
                        if let Ok(existing) = std::fs::canonicalize(&resolved) {
                            resolved = existing;
                        }
                    }
                    other => resolved.push(other.as_os_str()),
                }
            }
            resolved
        });
        #[cfg(windows)]
        let normalized = PathBuf::from(normalized.to_string_lossy().to_lowercase());
        Self::Path(normalized)
    }

    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::All, _) | (_, Self::All) => true,
            (Self::Path(left), Self::Path(right)) => {
                left.starts_with(right) || right.starts_with(left)
            }
            (Self::Named(left), Self::Named(right)) => left == right,
            _ => false,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ResourceAccess {
    pub resource: Resource,
    pub write: bool,
}

impl ResourceAccess {
    #[must_use]
    pub const fn read(resource: Resource) -> Self {
        Self {
            resource,
            write: false,
        }
    }
    #[must_use]
    pub const fn write(resource: Resource) -> Self {
        Self {
            resource,
            write: true,
        }
    }
    #[must_use]
    pub const fn exclusive() -> Self {
        Self::write(Resource::All)
    }
    #[must_use]
    pub fn conflicts(&self, other: &Self) -> bool {
        (self.write || other.write) && self.resource.overlaps(&other.resource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_aliases_and_parent_reads_conflict_with_child_writes() {
        let root = std::env::temp_dir();
        let child = root.join("ax-resource-not-created/file.txt");
        let alias = root.join("ax-resource-not-created/./file.txt");
        assert_eq!(Resource::path(&child), Resource::path(&alias));
        assert_eq!(
            Resource::path(root.join("ax-resource-not-created/../ax-resource-file.txt")),
            Resource::path(root.join("ax-resource-file.txt"))
        );
        assert!(
            ResourceAccess::read(Resource::path(&root))
                .conflicts(&ResourceAccess::write(Resource::path(&child)))
        );
        assert!(
            !ResourceAccess::read(Resource::path(&root))
                .conflicts(&ResourceAccess::read(Resource::path(&child)))
        );
    }
}
