use std::sync::Arc;

use async_nats::{Client, Message};
use tracing::{info, warn};
use trogon_sandbox_contract::ProviderDispatchPayload;
use trogon_sandbox_protocol::{Provider, RedispatchGuard, decode_verified};

use crate::dispatch_failure::DispatchFailure;
use crate::publish_reply::publish_reply;
use crate::routing::Routing;

/// Launches the dispatched lease once, however often the broker redelivers it, and replies with
/// the grant or the reason it failed.
pub(crate) async fn handle_lease(
    msg: Message,
    client: Client,
    provider: Arc<dyn Provider>,
    grants: Arc<RedispatchGuard>,
    routing: Arc<Routing>,
) {
    let payload = match decode_verified::<ProviderDispatchPayload>(&msg.payload, None) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(%error, "invalid dispatch payload");
            publish_reply(
                &client,
                msg.reply,
                &DispatchFailure::Decode {
                    decode_error: error.to_string(),
                },
            )
            .await;
            return;
        }
    };
    info!(lease_id = %payload.lease_id.as_str(), "lease accepted");
    let launched = grants
        .get_or_launch(&payload.lease_id, || {
            provider.lease(&payload.request, &payload.lease_id)
        })
        .await;
    match launched {
        Ok(mut grant) => {
            grant.worker_id = Some(routing.worker.clone());
            if routing.external {
                routing.host.remember_routed_grant(&payload.lease_id).await;
            }
            publish_reply(&client, msg.reply, &grant).await;
        }
        Err(error) => {
            publish_reply(
                &client,
                msg.reply,
                &DispatchFailure::Provider {
                    provider_error: error.to_string(),
                },
            )
            .await
        }
    }
}
