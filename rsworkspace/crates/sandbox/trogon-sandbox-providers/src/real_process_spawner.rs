use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{ProcessSpawner, SpawnedProcess, SubprocessError};

/// Spawns the agent binary as a child of this process.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealProcessSpawner;

#[async_trait::async_trait]
impl ProcessSpawner for RealProcessSpawner {
    async fn spawn(&self, binary: &str, env: Vec<(String, String)>) -> Result<SpawnedProcess, SubprocessError> {
        let mut command = tokio::process::Command::new(binary);
        for (key, value) in &env {
            command.env(key, value);
        }
        let child = command.spawn().map_err(SubprocessError::Spawn)?;
        Ok(SpawnedProcess {
            id: process_label(child.id()),
            child: Arc::new(Mutex::new(Some(child))),
        })
    }
}

fn process_label(pid: Option<u32>) -> String {
    pid.map_or_else(|| "unknown".to_string(), |pid| pid.to_string())
}

#[cfg(test)]
mod tests;
