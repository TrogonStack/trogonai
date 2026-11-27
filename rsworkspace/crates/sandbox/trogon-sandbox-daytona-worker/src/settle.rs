use tokio::time::Instant;

use crate::{DaytonaApi, DaytonaApiError, DaytonaProviderError, SandboxState, Wait};

/// Whether asking Daytona to move a sandbox in `state` to `target` is the next step.
fn needs_request(state: SandboxState, target: SandboxState) -> bool {
    match target {
        SandboxState::Started => matches!(state, SandboxState::Stopped | SandboxState::Archived),
        _ => state == SandboxState::Started,
    }
}

/// Moves a sandbox to `target` (started or stopped) and waits until Daytona reports it there. A sandbox already in
/// the state is left alone, so the call is idempotent.
pub(crate) async fn settle(
    api: &DaytonaApi,
    id: &str,
    target: SandboxState,
    wait: Wait,
) -> Result<(), DaytonaProviderError> {
    let deadline = Instant::now() + wait.timeout;
    let mut requested = false;
    loop {
        let record = api.get_sandbox(id).await?;
        let state = record.state.unwrap_or(SandboxState::Unknown);
        if state == target {
            return Ok(());
        }
        if state.is_failed() {
            return Err(DaytonaProviderError::SandboxFailed {
                state,
                reason: record.error_reason,
            });
        }
        if !requested && needs_request(state, target) {
            requested = true;
            let sent = match target {
                SandboxState::Started => api.start_sandbox(id).await,
                _ => api.stop_sandbox(id).await,
            };
            match sent {
                // Already moving: the next look sees where it went.
                Ok(()) | Err(DaytonaApiError::Conflict(_)) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        if Instant::now() >= deadline {
            return Err(DaytonaProviderError::SettleTimedOut { state, target });
        }
        tokio::time::sleep(wait.interval).await;
    }
}
