use std::collections::HashMap;
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
use trogon_sandbox_protocol::{
    PhaseTimer, Provider, ProviderError, RedispatchGuard, decode_verified, dispatch_failure_reply, lifecycle_subject,
};

use crate::publish_lifecycle::publish_lifecycle;
use crate::worker::E2bWorkerError;
use crate::worker::constants::ASSIGNED_ON_SHARED_INTAKE;

/// The cancellation token of every lease whose launch is in flight.
pub(crate) type ActiveLeases = Arc<Mutex<HashMap<LeaseId, CancellationToken>>>;

#[tracing::instrument(skip_all, fields(tenant = %request.tenant, lease_id = %lease_id))]
async fn launch(
    request: &LeaseRequest,
    lease_id: &LeaseId,
    js: &jetstream::Context,
    provider: &Arc<dyn Provider>,
    active: &ActiveLeases,
) -> Result<ProviderLease, ProviderError> {
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
    active.lock().await.insert(lease_id.clone(), CancellationToken::new());
    let provision = PhaseTimer::start(SandboxPhase::Allocate);
    publish_lifecycle(js, &subject, &provision.phase_started()).await;
    let result = provider.lease(request, lease_id).await;
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
    active.lock().await.remove(lease_id);
    result
}

async fn reject(client: &Client, message: async_nats::Message, reason: String) -> Result<(), E2bWorkerError> {
    if let Some(reply) = message.reply {
        let rejected = ProviderDispatchReply::DispatchRejected { decode_error: reason };
        client.publish(reply, serde_json::to_vec(&rejected)?.into()).await?;
    }
    Ok(())
}

/// Launches the lease one dispatch names and replies with the grant or the reason it failed. A redelivered dispatch
/// gets the first grant instead of a second sandbox. A dispatch that names an assignment belongs on the exact worker
/// intake subject, so the shared one refuses it.
pub(crate) async fn handle_lease_msg(
    message: async_nats::Message,
    client: Client,
    js: jetstream::Context,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
) -> Result<(), E2bWorkerError> {
    let payload = match decode_verified::<ProviderDispatchPayload>(&message.payload, None) {
        Ok(payload) => payload,
        Err(error) => {
            warn!(%error, "invalid dispatch payload");
            return reject(&client, message, error.to_string()).await;
        }
    };
    info!(lease_id = %payload.lease_id, tenant = %payload.request.tenant, "lease accepted");
    if payload.target.is_some() {
        return reject(&client, message, ASSIGNED_ON_SHARED_INTAKE.to_owned()).await;
    }
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

#[cfg(test)]
mod tests;
