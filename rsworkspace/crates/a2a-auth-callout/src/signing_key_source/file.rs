use std::path::Path;

use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;

use super::SigningKeySource;
use super::key_version::KeyVersion;
use super::signing_key_handle::SigningKeyHandle;

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
        let current_bytes = std::fs::read(current_path.as_ref()).map_err(|e| AuthCalloutError::KeyLoadIo {
            path: current_path.as_ref().to_path_buf(),
            source: e,
        })?;
        // VERSION_CURRENT / VERSION_PREVIOUS are validated string constants;
        // their KeyVersion::new can't fail at runtime.
        #[allow(clippy::expect_used)]
        let current = SigningKeyHandle::new(
            KeyVersion::new(VERSION_CURRENT).expect("static version"),
            signing_key_from_file_bytes(&current_bytes)?,
        );

        let previous = match previous_path {
            None => None,
            Some(p) => {
                let bytes = std::fs::read(p.as_ref()).map_err(|e| AuthCalloutError::KeyLoadIo {
                    path: p.as_ref().to_path_buf(),
                    source: e,
                })?;
                #[allow(clippy::expect_used)]
                let version = KeyVersion::new(VERSION_PREVIOUS).expect("static version");
                Some(SigningKeyHandle::new(version, signing_key_from_file_bytes(&bytes)?))
            }
        };

        Ok(Self { current, previous })
    }
}

fn signing_key_from_file_bytes(bytes: &[u8]) -> Result<SigningKey, AuthCalloutError> {
    let seed = std::str::from_utf8(bytes)
        .map_err(AuthCalloutError::KeyLoadUtf8)?
        .trim();
    SigningKey::from_seed(seed).map_err(AuthCalloutError::Jwt)
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
