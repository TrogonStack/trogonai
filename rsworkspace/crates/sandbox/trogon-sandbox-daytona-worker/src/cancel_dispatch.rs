use std::sync::Arc;

use tracing::{info, warn};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{Provider, RedispatchGuard};

use crate::DaytonaTerminals;
use crate::lease_dispatch::ActiveLeases;

/// The lease a cancel subject names: its last token.
pub(crate) fn cancel_lease_id(subject: &str) -> Option<LeaseId> {
    subject
        .split('.')
        .next_back()
        .filter(|token| !token.is_empty())
        .map(LeaseId::new)
}

/// Releases the lease a cancel message names, ends its terminals, and forgets its grant so a reused id launches
/// afresh.
pub(crate) async fn handle_cancel_msg(
    message: async_nats::Message,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<DaytonaTerminals>,
) {
    let Some(lease_id) = cancel_lease_id(message.subject.as_str()) else {
        return;
    };
    grants.forget(&lease_id);
    if let Some(token) = active.lock().await.get(&lease_id) {
        token.cancel();
    }
    terminals.close_lease(&lease_id).await;
    match provider.cancel(&lease_id).await {
        Ok(()) => info!(%lease_id, "lease released"),
        Err(error) => warn!(%lease_id, %error, "cancel failed"),
    }
}

#[cfg(test)]
mod tests;
