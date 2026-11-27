use std::path::{Path, PathBuf};

use crate::{ACTOR_ID_PATH, ActorId, ActorIdentityError};

/// Reads the actor identity the runtime publishes into the sandbox.
///
/// The file is rewritten on every resume, so it is read fresh on every request and never cached: a value captured
/// before a suspend names the actor that no longer exists.
#[derive(Debug, Clone)]
pub struct ActorIdentitySource {
    path: PathBuf,
}

impl ActorIdentitySource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn read(&self) -> Result<ActorId, ActorIdentityError> {
        let path = self.path.display().to_string();
        let raw = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|source| ActorIdentityError::Unreadable {
                path: path.clone(),
                source,
            })?;
        ActorId::new(raw).map_err(|source| ActorIdentityError::Invalid { path, source })
    }
}

impl Default for ActorIdentitySource {
    fn default() -> Self {
        Self::new(ACTOR_ID_PATH)
    }
}
