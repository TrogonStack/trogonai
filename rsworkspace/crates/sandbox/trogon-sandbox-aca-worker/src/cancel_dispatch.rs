use std::sync::Arc;

use async_nats::Message;
use tracing::{error, warn};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{Provider, RedispatchGuard};

use crate::{AcaError, extract_cancel_lease_id};

/// The lease a cancel subject names. A subject that names none is an error: guessing a lease from it
/// could cancel a lease the sender never meant.
pub(crate) fn lease_to_cancel(subject: &str) -> Result<LeaseId, AcaError> {
    extract_cancel_lease_id(subject).ok_or_else(|| AcaError::CancelSubject(subject.to_owned()))
}

/// Forgets the grant of the lease a cancel subject names, so a reused id launches afresh, and
/// cancels the lease. A subject that names no lease cancels nothing and is reported at error.
pub(crate) async fn handle_cancel(msg: Message, provider: Arc<dyn Provider>, grants: Arc<RedispatchGuard>) {
    let id = match lease_to_cancel(msg.subject.as_str()) {
        Ok(id) => id,
        Err(error) => {
            error!(%error, "refusing to cancel");
            return;
        }
    };
    grants.forget(&id);
    if let Err(error) = provider.cancel(&id).await {
        warn!(lease_id = %id, %error, "cancel failed");
    }
}

#[cfg(test)]
mod tests;
