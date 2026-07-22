use crate::client_handler::ClientHandler;
use crate::telemetry::metrics::Metrics;
use agent_client_protocol::schema::v1::CompleteElicitationNotification;
use async_nats::header::HeaderMap;
use tracing::{instrument, warn};
use trogon_semconv::span::ACP_CLIENT_ELICITATION_COMPLETE;

#[instrument(name = ACP_CLIENT_ELICITATION_COMPLETE, skip(headers, payload, client, metrics))]
pub async fn handle<C: ClientHandler + Sync>(
    headers: &HeaderMap,
    payload: &[u8],
    client: &C,
    has_reply: bool,
    metrics: &Metrics,
) {
    if has_reply {
        warn!("Unexpected reply subject on notification request");
    }
    match crate::wire::decode_notification_params::<CompleteElicitationNotification>(
        "elicitation/complete",
        headers,
        payload,
    ) {
        Ok(notification) => {
            if let Err(e) = client.elicitation_complete(notification).await {
                warn!(error = %e, "Failed to send elicitation complete notification");
            }
        }
        Err(e) => {
            metrics.record_error("elicitation_complete", "decode_failure");
            warn!(error = %e, "Failed to parse elicitation complete notification; dropping update");
        }
    }
}

#[cfg(test)]
mod tests;
