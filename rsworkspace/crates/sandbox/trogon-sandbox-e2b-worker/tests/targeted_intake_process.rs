#![cfg(unix)]

mod authority_support;
mod worker_process_support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use authority_support::{Api, Boxed, lease_request};
use futures::StreamExt;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderDispatchPayload, ProviderKind, ProviderLease, WorkerDispatchProtocol,
    WorkerHeartbeat,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope, nats_transfer, subjects};
use worker_process_support::Nats;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

#[tokio::test]
async fn the_production_worker_serves_and_replays_its_exact_signed_targeted_assignment() -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let mut api = Api::start().await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("process-intake-fixture", [37; 32]);
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    nats.client.flush().await?;
    let mut worker = worker_process_support::worker(&nats.url, &api.url, &api.url, &ledger, &signer)?;

    let heartbeat = tokio::time::timeout(Duration::from_secs(30), heartbeats.next())
        .await?
        .ok_or("the owned production worker did not publish a heartbeat")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    assert_eq!(heartbeat.provider, ProviderKind::E2b);
    assert!(
        worker.is_alive()?,
        "the heartbeat must come from the live worker process"
    );
    assert_eq!(heartbeat.dispatch_protocol, WorkerDispatchProtocol::TargetedV1);
    assert!(heartbeat.supports_lease_epoch.is_supported());

    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new("process-intake-attempt")?,
        epoch: Some(LeaseEpoch::FIRST),
    };
    let lease_id = LeaseId::new("process-intake-lease");
    let request = lease_request()?;
    let tenant = request.tenant.clone();
    let dispatch = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(target.clone()),
    };
    let subject = subjects::worker_dispatch(ProviderKind::E2b, &target.worker_id);
    let send_dispatch = || async {
        let body = signer.sign_payload(&serde_json::to_vec(&dispatch)?, OP_LEASE_DISPATCH)?;
        let reply = nats_transfer::request(&nats.client, &subject, body.into(), None, REQUEST_TIMEOUT).await?;
        Ok::<ProviderLease, Boxed>(serde_json::from_slice(&reply)?)
    };
    let grant = send_dispatch().await?;
    assert_eq!(grant.worker_id.as_ref(), Some(&target.worker_id));
    assert_eq!(grant.attempt_id.as_ref(), Some(&target.attempt_id));
    assert_eq!(grant.epoch, Some(LeaseEpoch::FIRST));

    let replay = send_dispatch().await?;
    assert_eq!(
        replay.sandbox_id, grant.sandbox_id,
        "a replay must retain the first native grant"
    );
    assert_eq!(replay.attempt_id, grant.attempt_id);
    assert_eq!(replay.epoch, grant.epoch);
    assert_eq!(
        api.control.creates.load(Ordering::SeqCst),
        1,
        "dispatch and replay allocate once"
    );

    let cleanup = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: lease_id.clone(),
            provider: ProviderKind::E2b,
            target: target.clone(),
        },
        intent: CancelIntent::Release,
    };
    let payload = serde_json::to_vec(&cleanup)?;
    let subject = subjects::worker_cleanup(ProviderKind::E2b, &target.worker_id);
    let auth = signer.sign_verb(
        &subject,
        &payload,
        &VerbScope {
            tenant,
            lease_id,
            worker_id: Some(target.worker_id.clone()),
            attempt_id: Some(target.attempt_id.clone()),
            epoch: Some(LeaseEpoch::FIRST),
        },
    )?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, auth);
    let reply = nats_transfer::request(&nats.client, &subject, payload.into(), Some(headers), REQUEST_TIMEOUT).await?;
    let response: ProviderCleanupResponse = serde_json::from_slice(&reply)?;
    assert!(
        matches!(response, ProviderCleanupResponse::Cleaned { .. }),
        "{response:?}"
    );
    assert_eq!(
        api.control.live_count(),
        0,
        "exact targeted cleanup removes the backend resource"
    );
    assert!(ledger.is_file(), "the configured dispatch ledger must exist");

    worker.stop()?;
    api.shutdown().await;
    Ok(())
}
