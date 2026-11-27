use std::time::Duration;

use trogon_sandbox_contract::ShortLivedToken;

use crate::{AcaError, AcaSessionApi, SessionIdentifier, SessionInfo};

/// Polls a session until it is ready. A session the service does not know yet counts as not ready;
/// any other error ends the wait.
pub async fn wait_session_ready(
    api: &dyn AcaSessionApi,
    bearer: &ShortLivedToken,
    identifier: &SessionIdentifier,
    max_attempts: u32,
    poll_interval: Duration,
) -> Result<SessionInfo, AcaError> {
    for _ in 0..max_attempts {
        match api.get_session(bearer, identifier).await {
            Ok(info) if info.status.is_ready() => return Ok(info),
            Ok(_) | Err(AcaError::AcaApi { status: 404, .. }) => {}
            Err(error) => return Err(error),
        }
        tokio::time::sleep(poll_interval).await;
    }
    Err(AcaError::SessionNotReady { attempts: max_attempts })
}
