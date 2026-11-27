use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{subjects, subscribe_verified, subscribe_verified_context};

use crate::E2bWorkerProvider;
use crate::worker::E2bWorkerError;
use crate::worker::targeted_cleanup::answer_cleanup;
use crate::worker::targeted_dispatch::answer_dispatch;

/// Serves the assignments addressed to this worker alone: dispatches on its intake subject and cleanups on its cleanup
/// subject. A worker with no dispatch ledger takes no assignments.
pub(crate) async fn serve_targeted(
    client: &Client,
    js: &jetstream::Context,
    e2b: &Arc<E2bWorkerProvider>,
    cancel: &CancellationToken,
) -> Result<(), E2bWorkerError> {
    let Some(ledger) = e2b.dispatch_ledger() else {
        return Ok(());
    };
    let mut dispatch =
        subscribe_verified(client, subjects::worker_dispatch(ProviderKind::E2b, ledger.worker_id())).await?;
    let mut cleanup =
        subscribe_verified_context(client, subjects::worker_cleanup(ProviderKind::E2b, ledger.worker_id())).await?;
    let (client, js, e2b, cancel) = (client.clone(), js.clone(), Arc::clone(e2b), cancel.clone());
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                next = dispatch.next() => {
                    let Some(message) = next else { break };
                    tokio::spawn(answer_dispatch(client.clone(), js.clone(), Arc::clone(&e2b), message));
                }
                next = cleanup.next() => {
                    let Some(verified) = next else { break };
                    tokio::spawn(answer_cleanup(client.clone(), Arc::clone(&e2b), verified));
                }
            }
        }
    });
    Ok(())
}
