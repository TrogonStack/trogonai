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
            signing_key_from_file_bytes(&current_bytes)?,
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
                    signing_key_from_file_bytes(&bytes)?,
                ))
            }
        };

        Ok(Self { current, previous })
    }
}

fn signing_key_from_file_bytes(bytes: &[u8]) -> Result<SigningKey, AuthCalloutError> {
    let seed = std::str::from_utf8(bytes)
        .map_err(|e| AuthCalloutError::Internal(format!("signing key file must be UTF-8 NKey seed: {e}")))?
        .trim();
    SigningKey::from_seed(seed).map_err(|e| AuthCalloutError::Internal(e.to_string()))
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
