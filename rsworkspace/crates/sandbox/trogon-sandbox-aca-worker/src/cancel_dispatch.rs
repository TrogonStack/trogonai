use std::sync::Arc;

use async_nats::Message;
use tracing::warn;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{Provider, RedispatchGuard};

use crate::extract_cancel_lease_id;

/// Forgets the grant of the lease a cancel subject names, so a reused id launches afresh, and
/// cancels the lease. A subject that names no lease cancels nothing, since no lease has it as an id.
pub(crate) async fn handle_cancel(msg: Message, provider: Arc<dyn Provider>, grants: Arc<RedispatchGuard>) {
    let subject = msg.subject.as_str();
    let id = extract_cancel_lease_id(subject).unwrap_or_else(|| LeaseId::new(subject));
    grants.forget(&id);
    if let Err(error) = provider.cancel(&id).await {
        warn!(lease_id = %id, %error, "cancel failed");
    }
}
