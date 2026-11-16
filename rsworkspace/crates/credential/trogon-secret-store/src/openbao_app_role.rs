use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trogon_std::{ReadFile, SecretString};

/// The AppRole identity the secrets service logs in to OpenBao with.
///
/// The secret id comes from a file the deployment platform provisions, never
/// from a config value or an environment variable, per ADR#0023 Decision 3 and
/// ADR#0007. The file is checked when the identity is built and read again for
/// every login, so the platform can rotate the secret id in place without a
/// restart. The secret id is never kept between logins, and `Debug` shows only
/// the file path.
#[derive(Clone)]
pub struct OpenBaoAppRole {
    role_id: Arc<str>,
    secret_id_file: Arc<Path>,
    fs: Arc<dyn ReadFile + Send + Sync>,
}

impl OpenBaoAppRole {
    /// Checks that `secret_id_file` holds a secret id, so a missing or blank
    /// file refuses to start instead of failing the first login.
    pub fn from_secret_id_file(
        role_id: impl AsRef<str>,
        secret_id_file: &Path,
        fs: Arc<dyn ReadFile + Send + Sync>,
    ) -> Result<Self, OpenBaoAppRoleError> {
        let role_id = role_id.as_ref();
        if role_id.is_empty() {
            return Err(OpenBaoAppRoleError::EmptyRoleId);
        }
        let approle = Self {
            role_id: Arc::from(role_id),
            secret_id_file: Arc::from(secret_id_file),
            fs,
        };
        approle.read_secret_id()?;
        Ok(approle)
    }

    pub fn role_id(&self) -> &str {
        &self.role_id
    }

    /// Reads the secret id the file holds now, ignoring surrounding whitespace
    /// such as the trailing newline most provisioners write.
    pub(crate) fn read_secret_id(&self) -> Result<SecretString, OpenBaoAppRoleError> {
        let contents =
            self.fs
                .read_to_string(&self.secret_id_file)
                .map_err(|source| OpenBaoAppRoleError::SecretIdUnreadable {
                    path: self.secret_id_file.to_path_buf(),
                    source,
                })?;
        SecretString::new(contents.trim()).map_err(|_| OpenBaoAppRoleError::EmptySecretId {
            path: self.secret_id_file.to_path_buf(),
        })
    }
}

impl fmt::Debug for OpenBaoAppRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBaoAppRole")
            .field("role_id", &self.role_id)
            .field("secret_id_file", &self.secret_id_file)
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
