use std::sync::Arc;

use async_nats::Message;
use futures::future::OptionFuture;
use tracing::warn;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{Provider, RedispatchGuard};

use crate::extract_cancel_lease_id;
use crate::routing::Routing;

/// Cancels the lease a cancel subject names and forgets its grant so a reused id launches afresh.
pub(crate) async fn handle_cancel(
    msg: Message,
    provider: Arc<dyn Provider>,
    grants: Arc<RedispatchGuard>,
    routing: Arc<Routing>,
) {
    let lease = extract_cancel_lease_id(msg.subject.as_str());
    OptionFuture::from(lease.map(|lease_id| cancel_lease(lease_id, provider, grants, routing))).await;
}

async fn cancel_lease(
    lease_id: LeaseId,
    provider: Arc<dyn Provider>,
    grants: Arc<RedispatchGuard>,
    routing: Arc<Routing>,
) {
    match provider.cancel(&lease_id).await {
        Ok(()) => {
            grants.forget(&lease_id);
            if routing.external {
                routing.host.forget_routed_grant(&lease_id).await;
            }
        }
        Err(error) => warn!(lease_id = %lease_id.as_str(), %error, "cancel failed"),
    }
}
