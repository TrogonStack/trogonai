use std::sync::Arc;

use async_nats::Message;
use trogon_sandbox_contract::{LeaseId, LeaseReleasedReason};

use crate::auth::cancel_lease_for_client;
use crate::serve::constants::LEASE_CANCEL_PREFIX;
use crate::serve::reply::publish_json;
use crate::serve::route_context::RouteContext;

/// Reads a cancel body. Clients send an empty body, which is a plain cancel, and a worker that
/// lost the sandbox sends `failed`. No other reason may be claimed through a cancel.
pub(crate) fn cancel_reason(body: &[u8]) -> LeaseReleasedReason {
    match serde_json::from_slice(body) {
        Ok(LeaseReleasedReason::Failed) => LeaseReleasedReason::Failed,
        _ => LeaseReleasedReason::Cancelled,
    }
}

/// Cancels the lease the subject names. Cancel began as a plain publish, so only a caller that
/// asked for a reply gets one.
pub(crate) async fn handle(context: Arc<RouteContext>, message: Message) {
    let lease_id = LeaseId::new(message.subject.strip_prefix(LEASE_CANCEL_PREFIX).unwrap_or_default());
    let reason = cancel_reason(&message.payload);
    let broker = &context.broker;
    let response = cancel_lease_for_client(
        &context.auth,
        message.headers.as_ref(),
        &lease_id,
        broker.lease_tenant(&lease_id),
        || broker.handle_cancel(lease_id.clone(), reason),
    )
    .await;
    if let Some(reply) = message.reply {
        publish_json(&context.nats, reply, &response).await;
    }
}

#[cfg(test)]
mod tests;
