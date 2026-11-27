use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::process::Child;
use tokio::sync::Mutex;

use crate::{SandboxError, SandboxHandle, SandboxStatus};

/// A launched agent process.
pub struct SubprocessSandboxHandle {
    pub(crate) id: String,
    pub(crate) child: Arc<Mutex<Option<Child>>>,
    pub(crate) cancelled: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl SandboxHandle for SubprocessSandboxHandle {
    fn id(&self) -> &str {
        &self.id
    }

    async fn status(&self) -> SandboxStatus {
        if self.cancelled.load(Ordering::Acquire) {
            return SandboxStatus::Cancelled;
        }

        let mut guard = self.child.lock().await;
        match guard.as_mut() {
            Some(child) => status_of(child.try_wait()),
            None => SandboxStatus::Cancelled,
        }
    }

    async fn cancel(&self) -> Result<(), SandboxError> {
        self.cancelled.store(true, Ordering::Release);
        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut() {
            // tokio's kill() sends SIGKILL on Unix rather than SIGTERM; acceptable for local dev use
            let _ = child.kill().await;
        }
        Ok(())
    }
}

fn status_of(polled: io::Result<Option<ExitStatus>>) -> SandboxStatus {
    match polled {
        Ok(None) => SandboxStatus::Running,
        Ok(Some(status)) if status.success() => SandboxStatus::Completed,
        Ok(Some(status)) => {
            let exit_code = status.code().unwrap_or(-1);
            SandboxStatus::Failed {
                error: format!("process exited with code {exit_code}"),
            }
        }
        Err(error) => SandboxStatus::Failed {
            error: format!("failed to poll process status: {error}"),
        },
    }
}

#[cfg(test)]
mod tests;
