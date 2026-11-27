use crate::{SpawnedProcess, SubprocessError};

/// Starts the agent binary with a given environment.
#[async_trait::async_trait]
pub trait ProcessSpawner: Send + Sync {
    async fn spawn(&self, binary: &str, env: Vec<(String, String)>) -> Result<SpawnedProcess, SubprocessError>;
}
