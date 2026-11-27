//! Releasing a lease through the public latency run takes the route the selected worker's protocol names: a typed,
//! signed cleanup that waits for the worker's acknowledgement for an assigned worker, and a signed provider cancel
//! for a legacy one. A grant that does not match its assignment never becomes native deletion authority.

mod support;

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use async_nats::Message;
use futures::StreamExt;
use support::heartbeat;
use support::wire;
use tokio::task::JoinHandle;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{
    ExecModel, GrantLedger, LatencyReport, NativeCleanupAuthority, Target, measure_latency,
};
use trogon_sandbox_contract::{
    CancelIntent, CapabilitySupport, DispatchAttemptId, LeaseAttemptIdentity, LeaseEpoch, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxId, WorkerDispatchProtocol,
    WorkerId,
};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch, worker_cleanup, worker_dispatch};
use trogon_sandbox_protocol::{
    DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, OP_LEASE_DISPATCH, VerbScope,
};

const KIND: ProviderKind = ProviderKind::Mock;

/// How the grant differs from the dispatch it answers.
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

/// Which deletion route the release took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    AssignedCleanup,
    ProviderCancel,
}

/// Aborts the lease run when the test ends early.
struct Operation(JoinHandle<LatencyReport>);

impl Drop for Operation {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Receipt {
    granted: bool,
    route: Route,
    scope: VerbScope,
    request: ProviderDispatchPayload,
    native: ProviderLease,
    ledger: GrantLedger,
    premature_completion: bool,
    reply_present: bool,
}

fn skewed(mut native: ProviderLease, case: GrantCase) -> Result<ProviderLease, Box<dyn Error>> {
    match case {
        GrantCase::Exact => {}
        GrantCase::ForeignWorker => native.worker_id = Some(WorkerId::new("foreign-reply-worker")?),
        GrantCase::MissingWorker => native.worker_id = None,
        GrantCase::ForeignAttempt => native.attempt_id = Some(DispatchAttemptId::new("foreign-reply-attempt")?),
        GrantCase::MissingAttempt => native.attempt_id = None,
        GrantCase::ForeignEpoch => native.epoch = Some(LeaseEpoch::new(2)?),
        GrantCase::MissingEpoch => native.epoch = None,
    }
    Ok(native)
}

async fn next_control(
    targeted: &mut async_nats::Subscriber,
    legacy: &mut async_nats::Subscriber,
) -> Option<(Route, Message)> {
    tokio::select! {
        message = targeted.next() => message.map(|message| (Route::AssignedCleanup, message)),
        message = legacy.next() => message.map(|message| (Route::ProviderCancel, message)),
    }
}

async fn release_case(
    server: &CoreTestServer,
    protocol: WorkerDispatchProtocol,
    case: GrantCase,
) -> Result<Receipt, Box<dyn Error>> {
    let client = async_nats::connect(server.address()).await?;
    let worker = WorkerId::new("release-worker")?;
    let selected = heartbeat(KIND, &worker, protocol, CapabilitySupport::Supported);
    let signer = Arc::new(DispatchSigner::new("release-fixture", [4; 32]));
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
    let dispatch_subject = if assigned {
        worker_dispatch(KIND, &worker)
    } else {
        provider_dispatch(KIND)
    };
    let mut dispatches = client.subscribe(dispatch_subject).await?;
    let mut targeted = client.subscribe(worker_cleanup(KIND, &worker)).await?;
    let mut legacy = client.subscribe("sandbox.provider.mock.cancel.>").await?;
    client.flush().await?;
    let mut operation = Operation(tokio::spawn(async move { measure_latency(&target, 1, None).await }));

    let message = wire::local(wire::next(&mut dispatches).await)?;
    let request: ProviderDispatchPayload =
        verifier.decode(&message.payload, OP_LEASE_DISPATCH, assigned.then_some(&worker))?;
    assert_eq!(request.request.tenant.as_str(), "conformance");
    assert_eq!(request.target.is_some(), assigned);
    if let Some(assignment) = &request.target {
        assert_eq!(assignment.worker_id, worker);
        assert_eq!(assignment.epoch, Some(LeaseEpoch::FIRST));
    }
    let native = skewed(
        ProviderLease {
            sandbox_id: SandboxId::new(format!("fixture-native-{}", request.lease_id.as_str()))?,
            evidence_subject: format!("sandbox.evidence.{}", request.lease_id.as_str()),
            worker_id: Some(worker.clone()),
            attempt_id: request.target.as_ref().map(|target| target.attempt_id.clone()),
            epoch: request.target.as_ref().and_then(|target| target.epoch),
        },
        case,
    )?;
    wire::local(wire::reply(&client, &message, &native).await)?;

    let (route, control) = tokio::time::timeout(wire::WAIT, next_control(&mut targeted, &mut legacy))
        .await?
        .ok_or("a control subscription ended")?;
    let verified = verifier
        .admit_message_scope(control.subject.as_str(), control.headers.as_ref(), &control.payload)?
        .ok_or("the release is not signed")?;
    assert!(verified.matches(control.subject.as_str(), &control.payload));
    let scope = verified.scope().clone();
    assert_eq!(scope.tenant, request.request.tenant);
    assert_eq!(scope.lease_id, request.lease_id);
    assert_eq!(
        scope.worker_id,
        request.target.as_ref().map(|target| target.worker_id.clone())
    );
    assert_eq!(
        scope.attempt_id,
        request.target.as_ref().map(|target| target.attempt_id.clone())
    );
    assert_eq!(scope.epoch, request.target.as_ref().and_then(|target| target.epoch));
    let reply_present = control.reply.is_some();
    let mut premature_completion = false;
    match route {
        Route::AssignedCleanup => {
            let cleanup: ProviderCleanupRequest = serde_json::from_slice(&control.payload)?;
            let assignment = request
                .target
                .clone()
                .ok_or("an assigned cleanup needs its assignment")?;
            assert_eq!(
                cleanup.identity,
                LeaseAttemptIdentity {
                    lease_id: request.lease_id.clone(),
                    provider: KIND,
                    target: assignment,
                }
            );
            assert_eq!(cleanup.intent, CancelIntent::Release);
            assert!(reply_present, "an assigned release waits for a cleanup acknowledgement");
            tokio::time::sleep(Duration::from_millis(25)).await;
            premature_completion = operation.0.is_finished();
            let cleaned = ProviderCleanupResponse::Cleaned {
                identity: cleanup.identity,
                intent: CancelIntent::Release,
            };
            wire::local(wire::reply(&client, &control, &cleaned).await)?;
        }
        Route::ProviderCancel => {
            assert_eq!(control.subject.as_str(), provider_cancel(KIND, &request.lease_id));
            assert!(control.payload.is_empty());
        }
    }
    let report = tokio::time::timeout(wire::WAIT, &mut operation.0).await??;
    client.flush().await?;
    let extra = tokio::time::timeout(Duration::from_millis(50), next_control(&mut targeted, &mut legacy)).await;
    assert!(extra.is_err(), "a release does not take a second deletion route");
    let acquired = &report.steps[0];
    Ok(Receipt {
        granted: acquired.failures == 0 && acquired.samples.len() == 1,
        route,
        scope,
        request,
        native,
        ledger,
        premature_completion,
        reply_present,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selected_targeted_release_uses_the_signed_cleanup_and_waits_for_its_acknowledgement()
-> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let receipt = release_case(&server, WorkerDispatchProtocol::TargetedV1, GrantCase::Exact).await?;
    assert!(receipt.granted);
    assert_eq!(receipt.route, Route::AssignedCleanup);
    assert!(receipt.reply_present && !receipt.premature_completion);
    let recorded = receipt.ledger.snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].native_cleanup_authority,
        NativeCleanupAuthority::RequestMatched
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selected_legacy_release_keeps_the_provider_cancel_route() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let receipt = release_case(&server, WorkerDispatchProtocol::Legacy, GrantCase::Exact).await?;
    assert!(receipt.granted);
    assert_eq!(receipt.route, Route::ProviderCancel);
    assert!(receipt.request.target.is_none());
    assert!(receipt.scope.worker_id.is_none() && receipt.scope.attempt_id.is_none() && receipt.scope.epoch.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mismatched_complete_grant_cannot_supply_native_deletion_authority() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    for case in [
        GrantCase::ForeignWorker,
        GrantCase::MissingWorker,
        GrantCase::ForeignAttempt,
        GrantCase::MissingAttempt,
        GrantCase::ForeignEpoch,
        GrantCase::MissingEpoch,
    ] {
        let receipt = release_case(&server, WorkerDispatchProtocol::TargetedV1, case).await?;
        assert!(!receipt.granted, "a mismatched {case:?} grant gives no usable lease");
        let recorded = receipt.ledger.snapshot();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].native, receipt.native);
        assert_eq!(recorded[0].lease_id, receipt.request.lease_id);
        assert_eq!(
            recorded[0].native_cleanup_authority,
            NativeCleanupAuthority::ReconciliationRequired,
            "a mismatched {case:?} reply stays evidence, not native deletion authority"
        );
        let assignment = receipt.request.target.as_ref().ok_or("the dispatch was assigned")?;
        assert_eq!(receipt.scope.worker_id.as_ref(), Some(&assignment.worker_id));
        assert_eq!(receipt.scope.attempt_id.as_ref(), Some(&assignment.attempt_id));
        assert_eq!(receipt.scope.epoch, assignment.epoch);
    }
    Ok(())
}
