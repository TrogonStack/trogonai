use std::path::Path;

use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;

use super::key_version::KeyVersion;
use super::signing_key_handle::SigningKeyHandle;
use super::SigningKeySource;

const VERSION_CURRENT: &str = "current";
const VERSION_PREVIOUS: &str = "previous";

#[derive(Debug)]
pub struct FileSigningKeySource {
    current: SigningKeyHandle,
    previous: Option<SigningKeyHandle>,
}

impl FileSigningKeySource {
    pub fn new(
        current_path: impl AsRef<Path>,
        previous_path: Option<impl AsRef<Path>>,
    ) -> Result<Self, AuthCalloutError> {
        let current_bytes = std::fs::read(current_path.as_ref()).map_err(|e| {
            AuthCalloutError::Internal(format!(
                "failed to read signing key at {}: {e}",
                current_path.as_ref().display()
            ))
        })?;
        let current = SigningKeyHandle::new(
            KeyVersion::new(VERSION_CURRENT).expect("static version"),
            SigningKey::from_secret(&current_bytes),
        );

        let previous = match previous_path {
            None => None,
            Some(p) => {
                let bytes = std::fs::read(p.as_ref()).map_err(|e| {
                    AuthCalloutError::Internal(format!(
                        "failed to read previous signing key at {}: {e}",
                        p.as_ref().display()
                    ))
                })?;
                Some(SigningKeyHandle::new(
                    KeyVersion::new(VERSION_PREVIOUS).expect("static version"),
                    SigningKey::from_secret(&bytes),
                ))
            }
        };

        Ok(Self { current, previous })
    }
}

impl SigningKeySource for FileSigningKeySource {
    fn current(&self) -> SigningKeyHandle {
        self.current.clone()
    }

    fn accepted(&self) -> Vec<SigningKeyHandle> {
        let mut keys = vec![self.current.clone()];
        if let Some(prev) = &self.previous {
            keys.push(prev.clone());
        }
        keys
    }
}
