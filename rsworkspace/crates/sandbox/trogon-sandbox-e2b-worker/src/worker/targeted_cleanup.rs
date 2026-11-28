use std::sync::Arc;

use async_nats::Client;
use trogon_sandbox_contract::{ProviderCleanupRequest, ProviderCleanupResponse};
use trogon_sandbox_protocol::{INVALID_CLEANUP_SCOPE, VerifiedRequest, nats_transfer};

use crate::E2bWorkerProvider;
use crate::worker::reply::publish_json;

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
            .filter(|scope| scope.matches(message.subject.as_str(), &bytes) && scope.scope().names_assignment(&request))
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
