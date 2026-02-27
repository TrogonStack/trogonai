use agent_client_protocol::{Client, SessionNotification};
use tracing::{instrument, warn};

#[instrument(name = "acp.client.session.update", skip(payload, client))]
pub async fn handle<C: Client>(payload: &[u8], client: &C) {
    match serde_json::from_slice::<SessionNotification>(payload) {
        Ok(notification) => {
            if let Err(e) = client.session_notification(notification).await {
                warn!(error = %e, "Failed to send session notification");
            }
        }
        Err(e) => {
            warn!(error = %e, "Failed to parse session notification");
        }
    }
}
