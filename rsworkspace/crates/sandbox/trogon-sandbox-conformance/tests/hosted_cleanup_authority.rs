//! Only a grant that matched its exact request may have its native resource deleted as a cleanup fallback.

mod support;

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use support::cleanup::run_with_exact_cleanup;
use support::wire::{self, Sendable};
use support::{heartbeat, locked};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{ExecModel, GrantLedger, Target};
use trogon_sandbox_contract::{
    CapabilitySupport, DispatchAttemptId, LeaseEpoch, ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxId,
    WorkerDispatchProtocol, WorkerId,
};
use trogon_sandbox_protocol::subjects::{provider_dispatch, worker_dispatch};
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH};

const KIND: ProviderKind = ProviderKind::Mock;

/// How long the suite and its cleanup may take once the one dispatch is answered.
const RUN_BOUND: Duration = Duration::from_secs(20);

/// How the grant differs from the assignment it answers.
#[derive(Clone, Copy, Debug)]
enum GrantCase {
    Exact,
    ForeignWorker,
    MissingWorker,
    ForeignAttempt,
    MissingAttempt,
    ForeignEpoch,
    MissingEpoch,
}

/// Whether the worker's own cleanup removes the native resource.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkerCleanup {
    Removes,
    LeavesItBehind,
}

#[derive(Default)]
struct NativeCalls {
    reads: Vec<SandboxId>,
    deletes: Vec<SandboxId>,
}

struct Receipt {
    outcome: Sendable<()>,
    calls: Arc<Mutex<NativeCalls>>,
    ledger: GrantLedger,
    native: ProviderLease,
}

async fn cleanup_case(
    protocol: WorkerDispatchProtocol,
    case: GrantCase,
    worker_cleanup: WorkerCleanup,
) -> Result<Receipt, Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = WorkerId::new("cleanup-selected-worker")?;
    let selected = heartbeat(KIND, &worker, protocol, CapabilitySupport::Supported);
    let signer = Arc::new(DispatchSigner::new("cleanup-authority-test", [5; 32]));
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key(signer.key_id(), &signer.public_key_b64())?
        .with_epoch_mode(EpochMode::Require);
    let ledger = GrantLedger::default();
    let mut target = Target::new(client.clone(), KIND)
        .for_worker(&selected)?
        .with_grant_ledger(ledger.clone());
    target.exec = ExecModel::None;
    target.signer = Some(signer);
    target.timeout = wire::WAIT;
    let assigned = protocol == WorkerDispatchProtocol::TargetedV1;
    let subject = if assigned {
        worker_dispatch(KIND, &worker)
    } else {
        provider_dispatch(KIND)
    };
    let mut dispatch = client.subscribe(subject).await?;
    client.flush().await?;
    let calls = Arc::new(Mutex::new(NativeCalls::default()));
    let run = {
        let reads = calls.clone();
        let deletes = calls.clone();
        let ledger = ledger.clone();
        tokio::spawn(async move {
            run_with_exact_cleanup(
                &target,
                &ledger,
                move |id| {
                    let mut calls = locked(&reads);
                    calls.reads.push(id);
                    let deleted = !calls.deletes.is_empty();
                    let result: Sendable<bool> = if worker_cleanup == WorkerCleanup::LeavesItBehind && !deleted {
                        Err("native absence is not confirmed".into())
                    } else {
                        Ok(true)
                    };
                    std::future::ready(result)
                },
                move |id| {
                    locked(&deletes).deletes.push(id);
                    std::future::ready(Sendable::Ok(()))
                },
            )
            .await
        })
    };
    let message = wire::local(wire::next(&mut dispatch).await)?;
    // Later dispatches find no worker and fail at once instead of waiting out the timeout.
    dispatch.unsubscribe().await?;
    let request: ProviderDispatchPayload =
        verifier.decode(&message.payload, OP_LEASE_DISPATCH, assigned.then_some(&worker))?;
    assert_eq!(request.target.is_some(), assigned);
    let mut native = ProviderLease {
        sandbox_id: SandboxId::new(format!("native-{}", request.lease_id.as_str()))?,
        evidence_subject: format!("sandbox.evidence.{}", request.lease_id.as_str()),
        worker_id: Some(worker.clone()),
        attempt_id: request.target.as_ref().map(|target| target.attempt_id.clone()),
        epoch: request.target.as_ref().and_then(|target| target.epoch),
    };
    match case {
        GrantCase::Exact => {}
        GrantCase::ForeignWorker => native.worker_id = Some(WorkerId::new("foreign-cleanup-worker")?),
        GrantCase::MissingWorker => native.worker_id = None,
        GrantCase::ForeignAttempt => native.attempt_id = Some(DispatchAttemptId::new("foreign-cleanup-attempt")?),
        GrantCase::MissingAttempt => native.attempt_id = None,
        GrantCase::ForeignEpoch => native.epoch = Some(LeaseEpoch::new(2)?),
        GrantCase::MissingEpoch => native.epoch = None,
    }
    wire::local(wire::reply(&client, &message, &native).await)?;
    let outcome = tokio::time::timeout(RUN_BOUND, run).await??;
    let grants = ledger.snapshot();
    assert_eq!(grants.len(), 1, "a complete reply stays visible for reconciliation");
    assert_eq!(grants[0].lease_id, request.lease_id);
    assert_eq!(grants[0].native, native);
    Ok(Receipt {
        outcome,
        calls,
        ledger,
        native,
    })
}

const PROTOCOLS: [WorkerDispatchProtocol; 2] = [WorkerDispatchProtocol::Legacy, WorkerDispatchProtocol::TargetedV1];

#[tokio::test(flavor = "multi_thread")]
async fn exact_selected_grants_allow_native_absence_verification_without_fallback() -> Result<(), Box<dyn Error>> {
    for protocol in PROTOCOLS {
        let receipt = cleanup_case(protocol, GrantCase::Exact, WorkerCleanup::Removes).await?;
        assert!(receipt.outcome.is_ok(), "{:?}", receipt.outcome);
        assert_eq!(receipt.ledger.snapshot()[0].native, receipt.native);
        let calls = locked(&receipt.calls);
        assert_eq!(calls.reads, vec![receipt.native.sandbox_id.clone()]);
        assert!(calls.deletes.is_empty());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_owned_fallback_still_cannot_turn_worker_cleanup_failure_into_acceptance() -> Result<(), Box<dyn Error>> {
    for protocol in PROTOCOLS {
        let receipt = cleanup_case(protocol, GrantCase::Exact, WorkerCleanup::LeavesItBehind).await?;
        assert!(
            receipt.outcome.is_err(),
            "the fallback keeps the worker's cleanup failure"
        );
        let id = receipt.native.sandbox_id.clone();
        let calls = locked(&receipt.calls);
        assert_eq!(calls.deletes, vec![id.clone()]);
        assert_eq!(calls.reads, vec![id.clone(), id]);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_assignment_grants_never_authorize_fallback_native_deletion() -> Result<(), Box<dyn Error>> {
    let mut violations = Vec::new();
    for case in [
        GrantCase::ForeignWorker,
        GrantCase::MissingWorker,
        GrantCase::ForeignAttempt,
        GrantCase::MissingAttempt,
        GrantCase::ForeignEpoch,
        GrantCase::MissingEpoch,
    ] {
        let receipt = cleanup_case(WorkerDispatchProtocol::TargetedV1, case, WorkerCleanup::LeavesItBehind).await?;
        assert!(receipt.outcome.is_err(), "a mismatched assignment stays failed");
        assert_eq!(receipt.ledger.snapshot()[0].native, receipt.native);
        if !locked(&receipt.calls).deletes.is_empty() {
            violations.push(format!("{case:?}"));
        }
    }
    assert!(violations.is_empty(), "mismatched grants were deleted: {violations:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn nonmatching_selected_legacy_grants_never_authorize_fallback_native_deletion() -> Result<(), Box<dyn Error>> {
    let mut violations = Vec::new();
    for case in [GrantCase::ForeignWorker, GrantCase::MissingWorker] {
        let receipt = cleanup_case(WorkerDispatchProtocol::Legacy, case, WorkerCleanup::LeavesItBehind).await?;
        assert!(receipt.outcome.is_err());
        if !locked(&receipt.calls).deletes.is_empty() {
            violations.push(format!("{case:?}"));
        }
    }
    assert!(
        violations.is_empty(),
        "nonmatching legacy grants were deleted: {violations:?}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unselected_legacy_grant_remains_evidence_without_native_delete_authority() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let ledger = GrantLedger::default();
    let mut target = Target::new(client.clone(), KIND).with_grant_ledger(ledger.clone());
    target.exec = ExecModel::None;
    target.timeout = Duration::from_secs(3);
    let mut dispatch = client.subscribe(provider_dispatch(KIND)).await?;
    client.flush().await?;
    let deletes = Arc::new(Mutex::new(Vec::new()));
    let run = {
        let ledger = ledger.clone();
        let reads = deletes.clone();
        let deletes = deletes.clone();
        tokio::spawn(async move {
            run_with_exact_cleanup(
                &target,
                &ledger,
                move |_| {
                    let result: Sendable<bool> = if locked(&reads).is_empty() {
                        Err("native absence is not confirmed".into())
                    } else {
                        Ok(true)
                    };
                    std::future::ready(result)
                },
                move |id| {
                    locked(&deletes).push(id);
                    std::future::ready(Sendable::Ok(()))
                },
            )
            .await
        })
    };
    let message = wire::local(wire::next(&mut dispatch).await)?;
    // Later dispatches find no worker and fail at once instead of waiting out the timeout.
    dispatch.unsubscribe().await?;
    let request: ProviderDispatchPayload = serde_json::from_slice(&message.payload)?;
    assert!(request.target.is_none());
    let native = ProviderLease {
        sandbox_id: SandboxId::new(format!("native-{}", request.lease_id.as_str()))?,
        evidence_subject: format!("sandbox.evidence.{}", request.lease_id.as_str()),
        worker_id: Some(WorkerId::new("unselected-reply-worker")?),
        attempt_id: None,
        epoch: None,
    };
    wire::local(wire::reply(&client, &message, &native).await)?;
    let outcome = tokio::time::timeout(RUN_BOUND, run).await??;
    assert!(outcome.is_err(), "unverified native custody stays unresolved");
    let recorded = ledger.snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].lease_id, request.lease_id);
    assert_eq!(recorded[0].native, native);
    assert!(
        locked(&deletes).is_empty(),
        "an unselected grant has no delete authority"
    );
    Ok(())
}
