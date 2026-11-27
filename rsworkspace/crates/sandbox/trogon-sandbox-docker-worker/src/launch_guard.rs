//! Keeps a lease from being reported `Running` when its container has already
//! exited.
//!
//! A lease that honors the image's own `ENTRYPOINT`/`CMD` has no control over
//! how long that process lives. A bare image such as `alpine` runs `/bin/sh`,
//! which exits at once without a TTY or stdin, so the container is gone before
//! the first exec. The guard checks the container right after start. When the
//! image command finished cleanly (exit code 0), it relaunches the lease as
//! [`BootMode::Bare`] (the keep-alive `sleep infinity` that warm-pool
//! containers use), because a sandbox must outlive any single command. A
//! command that failed is a broken image, not a finished one, so that and a
//! container still down after the relaunch fail the lease with the exit reason
//! instead of reporting it running.

use std::future::Future;
use std::time::Duration;

use tracing::warn;
use trogon_sandbox_contract::{BootMode, LifecycleEvent, SandboxId};
use trogon_sandbox_protocol::LifecycleEmitter;
use trogon_sandbox_providers::{SandboxHandle, SandboxStatus};

mod constants;
mod launch_guard_error;

pub(crate) use constants::WATCH_POLL;
use constants::{SETTLE_POLL, SETTLE_WINDOW};
pub(crate) use launch_guard_error::LaunchGuardError;

/// How a container that did not stay up ended.
pub(super) enum Exit {
    /// The main process finished with code 0.
    Clean,
    /// The main process failed or the container was stopped.
    Failed(String),
}

impl Exit {
    pub(super) fn reason(&self) -> &str {
        match self {
            Exit::Clean => "container exited with code 0",
            Exit::Failed(reason) => reason,
        }
    }
}

/// Returns how the container ended when it is no longer alive after the
/// settle window, or `None` when it stayed up.
async fn exit_reason(handle: &dyn SandboxHandle) -> Option<Exit> {
    let mut waited = Duration::ZERO;
    loop {
        match handle.status().await {
            SandboxStatus::Running | SandboxStatus::Pending => {}
            SandboxStatus::Completed => return Some(Exit::Clean),
            SandboxStatus::Failed { error } => return Some(Exit::Failed(error)),
            SandboxStatus::Cancelled => {
                return Some(Exit::Failed("container was cancelled".to_string()));
            }
        }
        if waited >= SETTLE_WINDOW {
            return None;
        }
        tokio::time::sleep(SETTLE_POLL).await;
        waited += SETTLE_POLL;
    }
}

/// Watches a launched container until it stops being alive and reports that
/// on the lease's lifecycle stream as [`LifecycleEvent::Failed`], the same
/// event a failed launch uses. Returns without reporting once `tracked`
/// says the lease was released, so a teardown the worker asked for is not
/// reported as a crash. A clean exit is reported too: a sandbox whose main
/// process is gone cannot serve execs whatever its exit code was.
pub(crate) async fn report_exit<T, Fut>(
    handle: &dyn SandboxHandle,
    sandbox_id: &SandboxId,
    poll: Duration,
    tracked: T,
    lifecycle: &dyn LifecycleEmitter,
) where
    T: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    // One not-alive reading can be a transient inspect error, so the exit
    // is reported only once two consecutive polls agree.
    let mut seen_down = false;
    let exit = loop {
        tokio::time::sleep(poll).await;
        if !tracked().await {
            return;
        }
        let exit = match handle.status().await {
            SandboxStatus::Running | SandboxStatus::Pending => {
                seen_down = false;
                continue;
            }
            SandboxStatus::Completed => Exit::Clean,
            SandboxStatus::Failed { error } => Exit::Failed(error),
            SandboxStatus::Cancelled => Exit::Failed("container was cancelled".to_string()),
        };
        if seen_down {
            break exit;
        }
        seen_down = true;
    };
    // The lease may have been released while the last inspect was in flight.
    if !tracked().await {
        return;
    }
    warn!(
        %sandbox_id,
        reason = exit.reason(),
        "sandbox container exited after launch"
    );
    lifecycle
        .emit(LifecycleEvent::Failed {
            sandbox_id: sandbox_id.clone(),
            error: exit.reason().to_string(),
        })
        .await;
}

async fn discard(handle: &dyn SandboxHandle) {
    if let Err(e) = handle.cancel().await {
        warn!(container_id = %handle.id(), error = %e, "failed to remove exited container");
    }
}

/// Launches a sandbox in `requested` boot mode through `launch` and returns a
/// handle whose container is still alive after it settles.
pub(crate) async fn launch_alive<F, Fut, E>(
    requested: BootMode,
    launch: F,
) -> Result<Box<dyn SandboxHandle>, LaunchGuardError>
where
    F: Fn(BootMode) -> Fut,
    Fut: Future<Output = Result<Box<dyn SandboxHandle>, E>>,
    E: std::error::Error + Send + Sync + 'static,
{
    let handle = launch(requested).await.map_err(LaunchGuardError::launch)?;
    let Some(exit) = exit_reason(handle.as_ref()).await else {
        return Ok(handle);
    };
    discard(handle.as_ref()).await;

    if requested == BootMode::Bare || matches!(exit, Exit::Failed(_)) {
        return Err(LaunchGuardError::not_running(&exit));
    }

    warn!(
        reason = exit.reason(),
        "image command exited right after start, relaunching with a keep-alive command"
    );
    let handle = launch(BootMode::Bare).await.map_err(LaunchGuardError::launch)?;
    if let Some(exit) = exit_reason(handle.as_ref()).await {
        discard(handle.as_ref()).await;
        return Err(LaunchGuardError::not_running(&exit));
    }
    Ok(handle)
}

#[cfg(test)]
mod tests;
