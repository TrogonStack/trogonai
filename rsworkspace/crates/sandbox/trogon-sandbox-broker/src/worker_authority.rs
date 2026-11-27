//! Answers a worker asking which of the leases it holds are still its own.
//!
//! The request arrives on `sandbox.worker.authority.{worker_id}` (see
//! [`trogon_sandbox_protocol::subjects::worker_authority_check`]). Workers hold no
//! credential the broker can verify, so the answer is built to disclose
//! nothing the asker does not already have: only lease ids the request itself
//! lists are considered, and the broker answers only when the subject's worker
//! id equals the body's. The answer separates three cases: assigned to the
//! asker (`owned`, with its epoch), recorded but assigned elsewhere or
//! finished (`not_assigned`), and unknown to the broker (in neither list).
//!
//! Deployment requirement: each worker's NATS user must be allowed to publish
//! only on its own `sandbox.worker.authority.<worker_id>` subject. Without
//! that scoping one worker could ask about another worker's leases by naming
//! it in both the subject and the body.
//!
//! A store failure is never answered as "owns nothing". The worker reads a
//! missing answer as a failed check and waits out its partition window, while
//! an empty answer would start freezing leases it should keep.

use std::sync::Arc;

use async_nats::Client;
use bytes::Bytes;
use tracing::warn;
use trogon_sandbox_contract::{
    LeaseRecord, LeaseStatus, OwnedLease, WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId,
};
use trogon_sandbox_protocol::MAX_LEASES_PER_REQUEST;
use trogon_sandbox_protocol::subjects::parse_worker_authority_check;

use crate::active_lease::ActiveLeaseStore;

#[derive(Debug, Eq, PartialEq)]
pub enum AuthorityRefusal {
    /// The subject names no worker.
    BadSubject,
    /// The body names a different worker than the subject.
    WorkerMismatch,
    TooManyLeases,
    /// The lease store could not be read.
    Store(String),
}

/// The leases in `request` that `store` still assigns to `subject_worker`.
pub async fn answer(
    store: &dyn ActiveLeaseStore,
    subject_worker: &WorkerId,
    request: &WorkerAuthorityRequest,
) -> Result<WorkerAuthorityResponse, AuthorityRefusal> {
    if &request.worker_id != subject_worker {
        return Err(AuthorityRefusal::WorkerMismatch);
    }
    if request.lease_ids.len() > MAX_LEASES_PER_REQUEST {
        return Err(AuthorityRefusal::TooManyLeases);
    }
    let mut owned = Vec::new();
    let mut not_assigned = Vec::new();
    for lease_id in &request.lease_ids {
        let record = store
            .get(lease_id)
            .await
            .map_err(|error| AuthorityRefusal::Store(error.to_string()))?;
        match record {
            Some(record) if assigned_to(&record, subject_worker) => owned.push(OwnedLease {
                lease_id: lease_id.clone(),
                epoch: record.epoch,
            }),
            Some(record)
                if record.worker_id.is_none()
                    && !matches!(record.status, LeaseStatus::Released | LeaseStatus::Failed) => {}
            Some(_) => not_assigned.push(lease_id.clone()),
            None => {}
        }
    }
    Ok(WorkerAuthorityResponse { owned, not_assigned })
}

/// An unrecorded owner grants no authority. Finished leases are never owned.
fn assigned_to(record: &LeaseRecord, worker: &WorkerId) -> bool {
    if matches!(record.status, LeaseStatus::Released | LeaseStatus::Failed) {
        return false;
    }
    record.worker_id.as_ref().is_some_and(|assigned| assigned == worker)
}

/// Serves one request message: decodes it, answers from `store` and replies.
/// A request that cannot be answered gets no reply.
pub async fn serve(client: &Client, store: &Arc<dyn ActiveLeaseStore>, message: async_nats::Message) {
    let Some(reply) = message.reply else {
        return;
    };
    let Some(subject_worker) = parse_worker_authority_check(message.subject.as_str()) else {
        warn!(subject = %message.subject, "worker authority request on a malformed subject");
        return;
    };
    let request: WorkerAuthorityRequest = match serde_json::from_slice(&message.payload) {
        Ok(request) => request,
        Err(error) => {
            warn!(%error, "failed to decode WorkerAuthorityRequest");
            return;
        }
    };
    match answer(store.as_ref(), &subject_worker, &request).await {
        Ok(response) => {
            if let Ok(payload) = serde_json::to_vec(&response) {
                let _ = client.publish(reply, Bytes::from(payload)).await;
            }
        }
        Err(refusal) => {
            warn!(worker_id = %subject_worker, ?refusal, "worker authority request not answered");
        }
    }
}

#[cfg(test)]
mod tests;
