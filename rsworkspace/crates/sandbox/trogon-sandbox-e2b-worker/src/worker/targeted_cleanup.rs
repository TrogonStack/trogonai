use std::sync::Arc;

use async_nats::Client;
use trogon_sandbox_contract::{ProviderCleanupRequest, ProviderCleanupResponse};
use trogon_sandbox_protocol::{VerbScope, VerifiedRequest, nats_transfer};

use crate::E2bWorkerProvider;
use crate::worker::constants::INVALID_CLEANUP_SCOPE;
use crate::worker::reply::publish_json;

/// Whether `scope` was signed for exactly the lease, worker, attempt and epoch `request` names.
pub(crate) fn names_assignment(scope: &VerbScope, request: &ProviderCleanupRequest) -> bool {
    let identity = &request.identity;
    scope.lease_id == identity.lease_id
        && scope.worker_id.as_ref() == Some(&identity.target.worker_id)
        && scope.attempt_id.as_ref() == Some(&identity.target.attempt_id)
        && scope.epoch == identity.target.epoch
}

/// Answers one cleanup request for an assignment of this worker. Cleanup destroys state, so it runs only for a request
/// signed for exactly the assignment it names; any other gets a failure.
pub(crate) async fn answer_cleanup(client: Client, e2b: Arc<E2bWorkerProvider>, verified: VerifiedRequest) {
    let VerifiedRequest { message, scope } = verified;
    let Some(reply) = message.reply.clone() else {
        return;
    };
    let Ok(bytes) = nats_transfer::receive(&client, &message).await else {
        return;
    };
    let Ok(request) = serde_json::from_slice::<ProviderCleanupRequest>(&bytes) else {
        return;
    };
    let response = async {
        let scope = scope
            .filter(|scope| {
                scope.matches(message.subject.as_str(), &bytes) && names_assignment(scope.scope(), &request)
            })
            .ok_or(INVALID_CLEANUP_SCOPE)?;
        Ok::<_, &str>(e2b.cleanup_targeted(&request, &scope.scope().tenant).await)
    }
    .await
    .unwrap_or_else(|reason| ProviderCleanupResponse::Failed {
        identity: request.identity.clone(),
        message: reason.to_owned(),
    });
    publish_json(&client, reply, &response, "targeted cleanup").await;
}

#[cfg(test)]
mod tests;
