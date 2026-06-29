use agent_client_protocol::{Client, SessionNotification};
use async_nats::header::HeaderMap;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_SESSION_UPDATE;

#[instrument(name = ACP_CLIENT_SESSION_UPDATE, skip(headers, payload, client))]
pub async fn handle<C: Client>(headers: &HeaderMap, payload: &[u8], client: &C, has_reply: bool) {
    if has_reply {
        warn!("Unexpected reply subject on notification request");
    }
    match crate::wire::decode_notification_params::<SessionNotification>("session/update", headers, payload) {
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

#[cfg(test)]
mod tests;
