use async_nats::{Client, HeaderMap, Message, Request, RequestErrorKind};
use bytes::Bytes;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    LeaseReleased, SnapshotExportError, SnapshotExportOutcome, SnapshotExportRequest, SnapshotPolicy,
    SnapshotPushTarget, SnapshotReceipt,
};
use trogon_sandbox_protocol::subjects::worker_snapshot_export;
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, VerbScope};

use crate::ExportError;
use crate::constants::{INITIAL_EXPORT_BACKOFF, MAX_EXPORT_ATTEMPTS};

/// Where the worker pushes the snapshot a release asks for, when it asks for one.
pub fn push_target_from_release(event: &LeaseReleased, default_registry: &str) -> Option<SnapshotPushTarget> {
    let policy = event.snapshot_out.as_ref()?;
    Some(push_target_from_policy(policy, default_registry))
}

pub fn push_target_from_policy(policy: &SnapshotPolicy, default_registry: &str) -> SnapshotPushTarget {
    SnapshotPushTarget {
        registry: default_registry.to_string(),
        repository: policy.target_repository.clone(),
        artifact_type: policy.artifact_type,
    }
}

/// Sends one export request, signed afresh each time so a retry is not a replay. Without a signer the request goes
/// out unsigned, which a worker in `warn` mode accepts.
async fn send_export(
    nats: &Client,
    signer: Option<&DispatchSigner>,
    subject: &str,
    event: &LeaseReleased,
    payload: &[u8],
) -> Result<Result<Message, async_nats::client::RequestError>, ExportError> {
    let mut request = Request::new().payload(Bytes::copy_from_slice(payload));
    if let Some(signer) = signer {
        let scope = VerbScope {
            epoch: None,
            tenant: event.tenant.clone(),
            lease_id: event.lease_id.clone(),
            worker_id: event.worker_id.clone(),
            attempt_id: None,
        };
        let token = signer
            .sign_verb(subject, payload, &scope)
            .map_err(|error| ExportError::Permanent(format!("failed to sign the export request: {error}")))?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTH_HEADER, token.as_str());
        request = request.headers(headers);
    }
    Ok(nats.send_request(subject.to_owned(), request).await)
}

/// Asks the worker that held the lease to export its snapshot, retrying only when no worker was listening.
pub async fn export_with_retry(
    nats: &Client,
    signer: Option<&DispatchSigner>,
    event: &LeaseReleased,
    default_registry: &str,
) -> Result<SnapshotReceipt, ExportError> {
    let worker_id = event.worker_id.as_ref().ok_or(ExportError::MissingWorkerId)?;

    let Some(policy) = event.snapshot_out.as_ref() else {
        return Err(ExportError::Permanent(
            "snapshot_out missing on release event".to_string(),
        ));
    };

    let request = SnapshotExportRequest {
        lease_id: event.lease_id.clone(),
        target: push_target_from_policy(policy, default_registry),
    };

    let subject = worker_snapshot_export(worker_id);
    let payload = serde_json::to_vec(&request).map_err(|error| ExportError::Permanent(error.to_string()))?;

    let mut backoff = INITIAL_EXPORT_BACKOFF;
    let mut last_err = String::from("no attempts made");

    for attempt in 1..=MAX_EXPORT_ATTEMPTS {
        match send_export(nats, signer, &subject, event, &payload).await? {
            Ok(response) => {
                let receipt = decode_outcome(&response.payload)?;
                info!(
                    lease_id = %event.lease_id.as_str(),
                    worker_id = %worker_id.as_str(),
                    digest = %receipt.digest,
                    bytes = receipt.bytes,
                    "worker snapshot export succeeded"
                );
                return Ok(receipt);
            }
            Err(error) if error.kind() == RequestErrorKind::NoResponders => {
                last_err = format!("NATS request was not dispatched: {error}");
                warn!(
                    lease_id = %event.lease_id.as_str(),
                    attempt,
                    error = %last_err,
                    "worker export had no responder, will retry"
                );
            }
            Err(error) => {
                return Err(ExportError::OutcomeUnknown(format!(
                    "NATS export result unknown: {error}"
                )));
            }
        }

        if attempt < MAX_EXPORT_ATTEMPTS {
            tokio::time::sleep(backoff).await;
            backoff = backoff.saturating_mul(2);
        }
    }

    Err(ExportError::Exhausted(last_err))
}

fn decode_outcome(payload: &[u8]) -> Result<SnapshotReceipt, ExportError> {
    let outcome: SnapshotExportOutcome = serde_json::from_slice(payload)
        .map_err(|error| ExportError::Permanent(format!("invalid worker export response: {error}")))?;

    match outcome {
        SnapshotExportOutcome::Ok(receipt) => Ok(receipt),
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { message }) => {
            Err(ExportError::OutcomeUnknown(message))
        }
        SnapshotExportOutcome::Err(
            SnapshotExportError::Permanent { message } | SnapshotExportError::Unsupported { message },
        ) => Err(ExportError::Permanent(message)),
        SnapshotExportOutcome::Native(_) => Err(ExportError::Permanent(
            "worker replied with a provider-native snapshot, which has no OCI artifact to export".to_string(),
        )),
    }
}
