use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trogon_std::{ReadFile, SecretString};

/// The AppRole identity the secrets service logs in to OpenBao with.
///
/// The secret id comes from a file the deployment platform provisions, never
/// from a config value or an environment variable, per ADR#0023 Decision 3 and
/// ADR#0007. It is held only as a [`SecretString`] and never shown by `Debug`.
#[derive(Clone)]
pub struct OpenBaoAppRole {
    role_id: Arc<str>,
    secret_id: SecretString,
}

impl OpenBaoAppRole {
    /// Reads the secret id from `secret_id_file`, ignoring surrounding
    /// whitespace such as the trailing newline most provisioners write.
    pub fn from_secret_id_file(
        role_id: impl AsRef<str>,
        secret_id_file: &Path,
        fs: &impl ReadFile,
    ) -> Result<Self, OpenBaoAppRoleError> {
        let role_id = role_id.as_ref();
        if role_id.is_empty() {
            return Err(OpenBaoAppRoleError::EmptyRoleId);
        }
        let contents = fs
            .read_to_string(secret_id_file)
            .map_err(|source| OpenBaoAppRoleError::SecretIdUnreadable {
                path: secret_id_file.to_path_buf(),
                source,
            })?;
        let secret_id = SecretString::new(contents.trim()).map_err(|_| OpenBaoAppRoleError::EmptySecretId {
            path: secret_id_file.to_path_buf(),
        })?;
        Ok(Self {
            role_id: Arc::from(role_id),
            secret_id,
        })
    }

    pub fn role_id(&self) -> &str {
        &self.role_id
    }

    pub(crate) fn secret_id(&self) -> &SecretString {
        &self.secret_id
    }
}

impl fmt::Debug for OpenBaoAppRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBaoAppRole")
            .field("role_id", &self.role_id)
            .field("secret_id", &"***")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenBaoAppRoleError {
    #[error("OpenBao AppRole role id must not be empty")]
    EmptyRoleId,
    #[error("failed to read the OpenBao AppRole secret id from {}: {source}", path.display())]
    SecretIdUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the OpenBao AppRole secret id file {} is empty", path.display())]
    EmptySecretId { path: PathBuf },
}

#[cfg(test)]
mod tests;
