use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream;
use trogon_sandbox_contract::{LeaseAttemptIdentity, ProviderDispatchPayload, ProviderKind};
use trogon_sandbox_protocol::{ProviderError, decode_verified, dispatch_failure_reply, nats_transfer};

use crate::E2bWorkerProvider;
use crate::worker::constants::{ASSIGNMENT_REQUIRED, LEDGER_UNAVAILABLE};
use crate::worker::reply::publish_json;

/// Answers one dispatch that names this worker: launches the lease it assigns and replies with the receipt, or with
/// the reason the launch failed.
pub(crate) async fn answer_dispatch(
    client: Client,
    js: jetstream::Context,
    e2b: Arc<E2bWorkerProvider>,
    message: async_nats::Message,
) {
    let result: Result<_, ProviderError> = async {
        let bytes = nats_transfer::receive(&client, &message)
            .await
            .map_err(ProviderError::backend)?;
        let ledger = e2b
            .dispatch_ledger()
            .ok_or_else(|| ProviderError::backend(LEDGER_UNAVAILABLE))?;
        let payload: ProviderDispatchPayload =
            decode_verified(&bytes, Some(ledger.worker_id())).map_err(ProviderError::backend)?;
        let target = payload
            .target
            .ok_or_else(|| ProviderError::backend(ASSIGNMENT_REQUIRED))?;
        let identity = LeaseAttemptIdentity {
            lease_id: payload.lease_id,
            provider: ProviderKind::E2b,
            target,
        };
        e2b.dispatch_targeted(&payload.request, &identity, &js)
            .await
            .map_err(ProviderError::backend)
    }
    .await;
    let Some(reply) = message.reply else { return };
    match result {
        Ok(receipt) => publish_json(&client, reply, &receipt, "targeted dispatch").await,
        Err(error) => publish_json(&client, reply, &dispatch_failure_reply(&error), "targeted dispatch").await,
    }
}
