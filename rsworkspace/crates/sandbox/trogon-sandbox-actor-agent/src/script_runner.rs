use std::future::Future;
use std::process::Stdio;

use tokio::process::Command;
use trogon_sandbox_contract::LeaseError;

use crate::script_outcome::ScriptOutcome;

/// Runs a shell script and reports how it ended.
///
/// A seam so provisioning logic is exercised without needing root or the real tools in the image.
pub(crate) trait ScriptRunner: Send + Sync {
    fn run(&self, shell: &str, script: &str) -> impl Future<Output = Result<ScriptOutcome, LeaseError>> + Send;
}

/// Runs the script through a real shell child process with no stdin.
pub(crate) struct ProcessScriptRunner;

impl ScriptRunner for ProcessScriptRunner {
    async fn run(&self, shell: &str, script: &str) -> Result<ScriptOutcome, LeaseError> {
        let output = Command::new(shell)
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| LeaseError::Backend {
                message: format!("failed to run {shell}: {error}"),
            })?;

        if output.status.success() {
            return Ok(ScriptOutcome::Succeeded);
        }
        Ok(ScriptOutcome::Failed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(512)
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests;
