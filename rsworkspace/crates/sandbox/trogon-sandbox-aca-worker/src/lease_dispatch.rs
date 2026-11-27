use std::sync::Arc;

use async_nats::{Client, Message};
use serde::Serialize;
use tracing::{info, warn};
use trogon_sandbox_contract::ProviderDispatchPayload;
use trogon_sandbox_protocol::{Provider, RedispatchGuard, decode_verified};

use crate::publish_reply::publish_reply;

#[derive(Serialize)]
enum DispatchFailure {
    #[serde(rename = "decode_error")]
    Decode(String),
    #[serde(rename = "provider_error")]
    Provider(String),
}

/// Launches the dispatched lease once, however often the broker redelivers it, and replies with
/// the grant or the reason it failed.
pub(crate) async fn handle_lease(
    msg: Message,
    client: Client,
    provider: Arc<dyn Provider>,
    grants: Arc<RedispatchGuard>,
) {
    let payload = match decode_verified::<ProviderDispatchPayload>(&msg.payload, None) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(%error, "invalid dispatch payload");
            publish_reply(&client, msg.reply, &DispatchFailure::Decode(error.to_string())).await;
            return;
        }
    };
    info!(lease_id = %payload.lease_id, "lease accepted");
    let launched = grants
        .get_or_launch(&payload.lease_id, || {
            provider.lease(&payload.request, &payload.lease_id)
        })
        .await;
    match launched {
        Ok(grant) => publish_reply(&client, msg.reply, &grant).await,
        Err(error) => publish_reply(&client, msg.reply, &DispatchFailure::Provider(error.to_string())).await,
    }
}
