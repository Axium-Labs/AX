//! Provider credential storage, adapted from pi's MIT-licensed `AuthStorage`.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::ModelError;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Credential {
    ApiKey {
        key: String,
    },
    OAuth {
        access: String,
        refresh: String,
        expires: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<String>,
    },
}

/// Provider-neutral OAuth credential persisted in AX's own auth store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthCredential {
    pub access: String,
    pub refresh: String,
    pub expires: u64,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AuthStorage {
    path: PathBuf,
}

impl AuthStorage {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Lists providers with credentials persisted by AX without exposing any
    /// secret material.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential file cannot be read or parsed.
    pub fn provider_ids(&self) -> Result<Vec<String>, ModelError> {
        Ok(self.load()?.into_keys().collect())
    }

    /// Resolve stored credentials before falling back to the provider's
    /// conventional environment variable, matching pi's precedence model.
    ///
    /// # Errors
    ///
    /// Returns an error when the auth file cannot be read or parsed.
    pub fn resolve_api_key(
        &self,
        provider: &str,
        environment: &str,
    ) -> Result<Option<String>, ModelError> {
        if let Some(Credential::ApiKey { key }) = self.load()?.remove(provider) {
            return Ok(Some(resolve_key(&key)));
        }
        Ok(std::env::var(environment)
            .ok()
            .filter(|value| !value.is_empty()))
    }

    /// Stores a provider API key in the AX auth file.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential cannot be encoded or persisted.
    pub fn store_api_key(&self, provider: &str, key: impl Into<String>) -> Result<(), ModelError> {
        let mut credentials = self.load()?;
        credentials.insert(provider.to_owned(), Credential::ApiKey { key: key.into() });
        self.save(&credentials)
    }

    /// Stores an OAuth credential under a provider id in AX's auth file.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential cannot be persisted.
    pub fn store_oauth(
        &self,
        provider: &str,
        credential: OAuthCredential,
    ) -> Result<(), ModelError> {
        let mut credentials = self.load()?;
        credentials.insert(
            provider.to_owned(),
            Credential::OAuth {
                access: credential.access,
                refresh: credential.refresh,
                expires: credential.expires,
                account_id: credential.account_id,
            },
        );
        self.save(&credentials)
    }

    /// Resolves a stored OAuth credential without consulting another
    /// application's credential files.
    ///
    /// # Errors
    ///
    /// Returns an error when the AX auth file cannot be read or parsed.
    pub fn resolve_oauth(&self, provider: &str) -> Result<Option<OAuthCredential>, ModelError> {
        Ok(match self.load()?.remove(provider) {
            Some(Credential::OAuth {
                access,
                refresh,
                expires,
                account_id,
            }) => Some(OAuthCredential {
                access,
                refresh,
                expires,
                account_id,
            }),
            _ => None,
        })
    }

    /// Removes a stored provider credential.
    ///
    /// # Errors
    ///
    /// Returns an error when the auth file cannot be read or rewritten.
    pub fn remove(&self, provider: &str) -> Result<bool, ModelError> {
        let mut credentials = self.load()?;
        if credentials.remove(provider).is_none() {
            return Ok(false);
        }
        self.save(&credentials)?;
        Ok(true)
    }

    fn load(&self) -> Result<BTreeMap<String, Credential>, ModelError> {
        match fs::read(&self.path) {
            Ok(content) => serde_json::from_slice(&content).map_err(|error| {
                ModelError::InvalidResponse(format!(
                    "invalid auth storage {}: {error}",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error.into()),
        }
    }

    fn save(&self, credentials: &BTreeMap<String, Credential>) -> Result<(), ModelError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_vec_pretty(credentials).map_err(|error| {
                ModelError::InvalidResponse(format!("failed to encode auth storage: {error}"))
            })?,
        )?;
        set_private_permissions(&temporary)?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

fn resolve_key(value: &str) -> String {
    std::env::var(value).unwrap_or_else(|_| value.to_owned())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), ModelError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Restricts the credential file to the current user, mirroring Unix's
/// `0600`. Windows has no direct equivalent of a mode bit, so this shells out
/// to `icacls` (present on every supported Windows version) to strip
/// inherited ACEs and grant full control solely to the current user.
/// Best-effort: failing to tighten the ACL never fails credential storage,
/// since the file already lives under the user's own profile directory.
#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)]
fn set_private_permissions(path: &Path) -> Result<(), ModelError> {
    let Some(user) = std::env::var("USERNAME")
        .ok()
        .filter(|name| !name.is_empty())
    else {
        return Ok(());
    };
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        .output();
    Ok(())
}

#[cfg(not(any(unix, windows)))]
#[allow(clippy::unnecessary_wraps)]
fn set_private_permissions(_path: &Path) -> Result<(), ModelError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_key_has_priority() {
        let path = std::env::temp_dir().join(format!("ax-auth-{}.json", std::process::id()));
        let storage = AuthStorage::new(&path);
        storage.store_api_key("deepseek", "stored-secret").unwrap();
        assert_eq!(
            storage
                .resolve_api_key("deepseek", "AX_MISSING_KEY")
                .unwrap()
                .as_deref(),
            Some("stored-secret")
        );
        fs::remove_file(path).ok();
    }

    #[cfg(windows)]
    #[test]
    fn windows_credential_file_denies_broad_default_groups() {
        let path = std::env::temp_dir().join(format!("ax-auth-acl-{}.json", std::process::id()));
        let storage = AuthStorage::new(&path);
        storage.store_api_key("deepseek", "stored-secret").unwrap();

        let output = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .expect("icacls should be available on Windows");
        let listing = String::from_utf8_lossy(&output.stdout).to_lowercase();
        assert!(
            !listing.contains("everyone") && !listing.contains("builtin\\users"),
            "expected inherited broad groups to be stripped, got: {listing}"
        );
        let user = std::env::var("USERNAME").unwrap().to_lowercase();
        assert!(
            listing.contains(&user),
            "expected the current user to retain access, got: {listing}"
        );

        fs::remove_file(path).ok();
    }
}
