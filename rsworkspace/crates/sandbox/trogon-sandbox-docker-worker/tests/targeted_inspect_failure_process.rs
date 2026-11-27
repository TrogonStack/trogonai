#![cfg(unix)]

use std::time::Duration;

use anyhow::{Context, ensure};
use futures::StreamExt;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind, WorkerDispatchProtocol,
    WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::AttemptLedger;
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope};

mod inspect_failure_support;
mod targeted_process_support;
use inspect_failure_support::{Proxy, Worker};
use targeted_process_support::{Native, Nats, fresh_id};

fn payload(lease: &LeaseId, worker: WorkerId) -> anyhow::Result<ProviderDispatchPayload> {
    let target = DispatchTarget {
        worker_id: worker,
        attempt_id: DispatchAttemptId::new(format!("{}-attempt", lease.as_str()))?,
        epoch: Some(LeaseEpoch::new(2)?),
    };
    targeted_process_support::dispatch_payload(
        "docker-inspect-durability",
        lease,
        targeted_process_support::registry_image(),
        None,
        Some(target),
    )
}

async fn heartbeat(
    nats: &Nats,
    sub: &mut async_nats::Subscriber,
    worker: &mut Worker,
) -> anyhow::Result<WorkerHeartbeat> {
    let message = tokio::time::timeout(Duration::from_secs(15), sub.next())
        .await?
        .context("actual fault-fixture worker heartbeat absent")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
    ensure!(
        worker.alive()?
            && heartbeat.provider == ProviderKind::LocalDocker
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
            && heartbeat.supports_lease_epoch.is_supported(),
        "actual fault-fixture worker did not install healthy targeted intake"
    );
    let worker_id = heartbeat.worker_id.clone();
    let legacy = nats
        .subscriptions(&trogon_sandbox_protocol::subjects::provider_dispatch(
            ProviderKind::LocalDocker,
        ))
        .await?;
    let targeted = nats
        .subscriptions(&trogon_sandbox_protocol::subjects::worker_dispatch(
            ProviderKind::LocalDocker,
            &worker_id,
        ))
        .await?;
    ensure!(
        legacy.len() == 1 && targeted.len() == 1 && legacy[0].cid == targeted[0].cid,
        "actual fault-fixture intake subscriptions did not share the owned worker connection"
    );
    Ok(heartbeat)
}

async fn dispatch(
    nats: &Nats,
    signer: &DispatchSigner,
    payload: &ProviderDispatchPayload,
) -> anyhow::Result<ProviderDispatchReply> {
    let target = payload.target.as_ref().context("dispatch carries no target")?;
    let bytes = signer
        .sign_payload(&serde_json::to_vec(payload)?, OP_LEASE_DISPATCH)
        .map_err(anyhow::Error::msg)?;
    let message = nats
        .client
        .send_request(
            trogon_sandbox_protocol::subjects::worker_dispatch(ProviderKind::LocalDocker, &target.worker_id),
            async_nats::Request::new()
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(30))),
        )
        .await?;
    Ok(serde_json::from_slice(&message.payload)?)
}

async fn cleanup(
    nats: &Nats,
    signer: &DispatchSigner,
    payload: &ProviderDispatchPayload,
) -> anyhow::Result<ProviderCleanupResponse> {
    let target = payload.target.as_ref().context("dispatch carries no target")?;
    let request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: payload.lease_id.clone(),
            provider: ProviderKind::LocalDocker,
            target: target.clone(),
        },
        intent: CancelIntent::Release,
    };
    let subject = trogon_sandbox_protocol::subjects::worker_cleanup(ProviderKind::LocalDocker, &target.worker_id);
    let bytes = serde_json::to_vec(&request)?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(
        AUTH_HEADER,
        signer.sign_verb(
            &subject,
            &bytes,
            &VerbScope {
                tenant: payload.request.tenant.clone(),
                lease_id: payload.lease_id.clone(),
                worker_id: Some(target.worker_id.clone()),
                attempt_id: Some(target.attempt_id.clone()),
                epoch: target.epoch,
            },
        )?,
    );
    let message = nats
        .client
        .send_request(
            subject,
            async_nats::Request::new()
                .headers(headers)
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(20))),
        )
        .await?;
    Ok(serde_json::from_slice(&message.payload)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_post_allocation_inspection_retains_the_real_native_receipt_durably() -> anyhow::Result<()> {
    let nats = Nats::start().await?;
    let directory = tempfile::tempdir()?;
    let state = directory.path().join("state");
    let signer = targeted_process_support::signer()?;
    let owner = format!("docker-inspect-durability-{}", fresh_id());
    let control_lease = LeaseId::new("docker-inspect-control");
    let fault_lease = LeaseId::new("docker-inspect-fault");
    let mut native = Native::new(owner.clone()).await?;
    native.register_lease(control_lease.as_str());
    native.register_lease(fault_lease.as_str());
    let mut proxy = Proxy::start(&owner, control_lease.as_str(), fault_lease.as_str()).await?;
    let mut sub = nats
        .client
        .subscribe(trogon_sandbox_protocol::subjects::worker_heartbeat_filter(
            ProviderKind::LocalDocker,
        ))
        .await?;
    nats.client.flush().await?;
    let mut worker = Worker::start(&nats.url, &state, &owner, &signer, Some(&proxy))?;
    let mut restarted = None;
    let mut stage = "unfaulted_proxy_control";
    let observation = async {
        let initial = heartbeat(&nats, &mut sub, &mut worker).await?;
        let worker_id = initial.worker_id.clone();
        let control = payload(&control_lease, worker_id.clone())?;
        let control_grant = match dispatch(&nats, &signer, &control).await? {
            ProviderDispatchReply::Granted(grant) => grant,
            _ => anyhow::bail!("unfaulted actual-worker proxy control dispatch was not granted"),
        };
        native.register_id(control_grant.sandbox_id.as_str());
        let control_raw = proxy.allocation(control_lease.as_str());
        ensure!(control_raw.creates == 1 && control_raw.container.as_deref() == Some(control_grant.sandbox_id.as_str())
            && control_raw.started && control_raw.baseline_complete && !control_raw.fault_injected,
            "unfaulted proxy control did not positively observe actual create/start/baseline completion");
        native.running(control_grant.sandbox_id.as_str(), control_lease.as_str()).await?;
        ensure!(matches!(cleanup(&nats, &signer, &control).await?, ProviderCleanupResponse::Cleaned { .. }),
            "unfaulted actual-worker proxy control cleanup failed");
        native.wait_absent(control_grant.sandbox_id.as_str()).await?;
        ensure!(native.owned().await?.is_empty(), "unfaulted proxy control left owned native assets");
        eprintln!("inspect-fault setup: actual_worker_live=true targeted_same_connection=true unfaulted_actual_create_start_baseline=true exact_unfaulted_cleanup=true");

        stage = "post_allocation_inspect_fault";
        let assigned = payload(&fault_lease, worker_id.clone())?;
        let refused = dispatch(&nats, &signer, &assigned).await?;
        let raw = proxy.allocation(fault_lease.as_str());
        ensure!(raw.creates == 1 && raw.started && raw.baseline_complete && raw.fault_injected,
            "the owned proxy did not inject exactly after actual allocation/start/baseline completion");
        let id = raw.container.context("the actual Docker create did not retain its canonical ID")?;
        native.register_id(&id);
        ensure!(matches!(refused, ProviderDispatchReply::LaunchFailed { .. }),
            "the injected native inspection failure did not cause an explicit provider refusal");
        native.running(&id, fault_lease.as_str()).await?;
        ensure!(proxy.failed_connections() == 0, "proxy transport or framing failed instead of the intended native fault");
        worker.stop()?;
        stage = "durable_ledger_reopen";
        let stored = {
            let ledger = AttemptLedger::open(state.join("dispatch.sqlite"), ProviderKind::LocalDocker, Some(worker_id.clone()))?;
            ledger.try_states().await?.into_iter().find(|entry| entry.identity.lease_id == fault_lease)
        };
        let identity = LeaseAttemptIdentity { lease_id: fault_lease.clone(), provider: ProviderKind::LocalDocker, target: assigned.target.clone().context("assigned dispatch carries no target")? };
        let retained = stored.as_ref().is_some_and(|entry| entry.identity == identity && entry.allocation_started
            && !entry.backend_cleaned && entry.receipt.as_ref().is_some_and(|receipt| receipt.sandbox_id.as_str() == id
                && receipt.worker_id.as_ref() == Some(&identity.target.worker_id)
                && receipt.attempt_id.as_ref() == Some(&identity.target.attempt_id)
                && receipt.epoch == identity.target.epoch));
        eprintln!("inspect-fault observation: actual_create_start_baseline=true injected_inspect_failure=true dispatch_refused=true exact_native_still_running=true durable_exact_raw_receipt={retained}");

        sub.unsubscribe().await?;
        sub = nats.client.subscribe(trogon_sandbox_protocol::subjects::worker_heartbeat_filter(ProviderKind::LocalDocker)).await?;
        nats.client.flush().await?;
        stage = "actual_worker_restart";
        restarted = Some(Worker::start(&nats.url, &state, &owner, &signer, None)?);
        let recovered = heartbeat(&nats, &mut sub, restarted.as_mut().context("fixture state not prepared")?).await?;
        ensure!(recovered.worker_id == initial.worker_id, "actual restart changed durable assigned worker identity");
        native.wait_absent(&id).await?;
        let cleanup = cleanup(&nats, &signer, &assigned).await?;
        let exact_cleanup = matches!(cleanup, ProviderCleanupResponse::Cleaned { identity: ref confirmed, intent: CancelIntent::Release } if *confirmed == identity);
        eprintln!("inspect-fault recovery: actual_identity_stable=true exact_original_native_absent=true exact_targeted_cleanup={exact_cleanup}");
        ensure!(native.owned().await?.is_empty(), "inspect-fault restart left native assets behind");
        Ok::<_, anyhow::Error>((retained, exact_cleanup))
    }.await;
    if observation.is_err() {
        eprintln!("inspect-fault primary observation: stage={stage} status=failed_redacted");
    }
    let first_stop = worker.stop();
    let restart_stop = restarted.as_mut().map(Worker::stop).transpose();
    let final_raw = proxy.allocation(fault_lease.as_str());
    if let Some(id) = final_raw.container {
        native.register_id(&id);
    }
    let cleanup = native.finish().await;
    let proxy_stop = proxy.stop().await;
    first_stop?;
    restart_stop?;
    proxy_stop?;
    ensure!(
        !cleanup?,
        "fixture reconciliation cannot promote inspect-fault product cleanup to acceptance"
    );
    let (retained, exact_cleanup) = observation?;
    ensure!(
        retained,
        "MISSING PRODUCT GUARANTEE: an actual allocation followed by inspection failure must retain its complete raw native receipt durably before that fallible check"
    );
    ensure!(
        exact_cleanup,
        "retained actual allocation could not complete exact signed targeted cleanup after restart"
    );
    Ok(())
}
