use std::sync::Arc;

use tracing::{info, warn};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{Provider, RedispatchGuard};

use crate::terminal::E2bTerminals;
use crate::worker::lease_dispatch::ActiveLeases;

/// The lease a cancel subject names: its last token.
pub(crate) fn cancel_lease_id(subject: &str) -> Option<LeaseId> {
    subject
        .split('.')
        .next_back()
        .filter(|token| !token.is_empty())
        .map(LeaseId::new)
}

/// Releases the lease a cancel message names: forgets its grant so a reused id launches afresh, closes its terminals,
/// stops its launch if one is in flight, and cancels it on the provider.
pub(crate) async fn handle_cancel_msg(
    message: async_nats::Message,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<E2bTerminals>,
) {
    let Some(lease_id) = cancel_lease_id(message.subject.as_str()) else {
        return;
    };
    grants.forget(&lease_id);
    terminals.close_lease(&lease_id).await;
    if let Some(token) = active.lock().await.get(&lease_id) {
        token.cancel();
    }
    match provider.cancel(&lease_id).await {
        Ok(()) => info!(%lease_id, "lease released"),
        Err(error) => warn!(%lease_id, %error, "cancel failed"),
    }
}

#[cfg(test)]
mod tests;
