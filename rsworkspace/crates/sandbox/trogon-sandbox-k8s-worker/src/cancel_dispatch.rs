use std::sync::Arc;

use async_nats::Message;
use futures::future::OptionFuture;
use tracing::{info, warn};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{Provider, RedispatchGuard};

use crate::{K8sTerminals, extract_cancel_lease_id};

/// Closes the terminals of the lease a cancel subject names, cancels it, and forgets its grant so a reused id
/// launches afresh.
pub(crate) async fn handle_cancel(
    msg: Message,
    provider: Arc<dyn Provider>,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<K8sTerminals>,
) {
    let lease = extract_cancel_lease_id(msg.subject.as_str());
    OptionFuture::from(lease.map(|lease_id| cancel_lease(lease_id, provider, grants, terminals))).await;
}

async fn cancel_lease(
    lease_id: LeaseId,
    provider: Arc<dyn Provider>,
    grants: Arc<RedispatchGuard>,
    terminals: Arc<K8sTerminals>,
) {
    terminals.close_lease(&lease_id).await;
    match provider.cancel(&lease_id).await {
        Ok(()) => {
            grants.forget(&lease_id);
            info!(lease_id = %lease_id, "lease released");
        }
        Err(error) => warn!(lease_id = %lease_id, %error, "cancel failed"),
    }
}
