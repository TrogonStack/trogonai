use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, LifecycleEvent, ProviderDispatchPayload, ProviderDispatchReply, ProviderLease, SandboxId,
    SandboxPhase,
};
use trogon_sandbox_protocol::lifecycle_subject;
use trogon_sandbox_protocol::{
    PhaseTimer, Provider, ProviderError, RedispatchGuard, decode_verified, dispatch_failure_reply,
};

use crate::VercelWorkerError;

/// The cancellation token of every lease whose launch is in flight.
pub(crate) type ActiveLeases = Arc<Mutex<HashMap<LeaseId, CancellationToken>>>;

async fn publish_lifecycle(js: &jetstream::Context, subject: &str, event: &LifecycleEvent) {
    let sent: Result<(), Box<dyn std::error::Error>> = async {
        let payload = serde_json::to_vec(event)?;
        js.publish(subject.to_owned(), payload.into()).await?;
        Ok(())
    }
    .await;
    if let Err(error) = sent {
        warn!(subject, %error, "failed to publish lifecycle event");
    }
}

/// Runs `allocate` for `lease_id` between its lifecycle events: Started and the allocation phase before, Completed or
/// Failed after.
#[tracing::instrument(skip_all, fields(lease_id = %lease_id))]
pub(crate) async fn with_lifecycle<E: std::fmt::Display>(
    lease_id: &LeaseId,
    js: &jetstream::Context,
    allocate: impl Future<Output = Result<ProviderLease, E>>,
) -> Result<ProviderLease, E> {
    let subject = lifecycle_subject(lease_id);
    // A lease id the protocol would not accept as a sandbox id has no lifecycle events to name it in.
    let named = SandboxId::new(lease_id.as_str()).ok();
    if let Some(sandbox_id) = &named {
        publish_lifecycle(
            js,
            &subject,
            &LifecycleEvent::Started {
                sandbox_id: sandbox_id.clone(),
            },
        )
        .await;
    }
    let provision = PhaseTimer::start(SandboxPhase::Allocate);
    publish_lifecycle(js, &subject, &provision.phase_started()).await;
    let result = allocate.await;
    match &result {
        Ok(lease) => {
            publish_lifecycle(js, &subject, &provision.complete()).await;
            info!(sandbox_id = %lease.sandbox_id, "sandbox launched");
            publish_lifecycle(
                js,
                &subject,
                &LifecycleEvent::Completed {
                    sandbox_id: lease.sandbox_id.clone(),
                },
            )
            .await;
        }
        Err(error) => {
            publish_lifecycle(js, &subject, &provision.fail(error.to_string())).await;
            warn!(%error, "sandbox launch failed");
            if let Some(sandbox_id) = named {
                publish_lifecycle(
                    js,
                    &subject,
                    &LifecycleEvent::Failed {
                        sandbox_id,
                        error: error.to_string(),
                    },
                )
                .await;
            }
        }
    }
    result
}

async fn launch(
    request: &LeaseRequest,
    lease_id: &LeaseId,
    js: &jetstream::Context,
    provider: &Arc<dyn Provider>,
    active: &ActiveLeases,
) -> Result<ProviderLease, ProviderError> {
    active.lock().await.insert(lease_id.clone(), CancellationToken::new());
    let result = with_lifecycle(lease_id, js, provider.lease(request, lease_id)).await;
    active.lock().await.remove(lease_id);
    result
}

/// Launches the lease one dispatch names and replies with the grant or the reason it failed. A redelivered dispatch
/// gets the first grant instead of a second sandbox.
pub(crate) async fn handle_lease_msg(
    message: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
) -> Result<(), VercelWorkerError> {
    let payload = match decode_verified::<ProviderDispatchPayload>(&message.payload, None) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(%error, "invalid dispatch payload");
            if let Some(reply) = message.reply {
                let rejected = ProviderDispatchReply::DispatchRejected {
                    decode_error: error.to_string(),
                };
                client.publish(reply, serde_json::to_vec(&rejected)?.into()).await?;
            }
            return Ok(());
        }
    };
    info!(lease_id = %payload.lease_id, "lease accepted");
    let result = grants
        .get_or_launch(&payload.lease_id, || {
            launch(&payload.request, &payload.lease_id, &js, &provider, &active)
        })
        .await;
    if let Some(reply) = message.reply {
        let body = match result {
            Ok(lease) => ProviderDispatchReply::Granted(lease),
            Err(error) => dispatch_failure_reply(&error),
        };
        client.publish(reply, serde_json::to_vec(&body)?.into()).await?;
    }
    Ok(())
}
