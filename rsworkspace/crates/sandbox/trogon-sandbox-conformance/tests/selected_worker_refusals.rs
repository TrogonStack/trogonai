//! A target selected from a heartbeat keeps the worker, attempt and epoch it dispatched to, and refuses grants that
//! name anything else.

mod support;

use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use support::heartbeat;
use support::wire;
use tokio::task::JoinHandle;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{
    AssignmentFaultError, CheckFailureError, ExecModel, GrantLedger, Target, checks_for_target,
};
use trogon_sandbox_contract::{
    CancelIntent, CapabilitySupport, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch,
    ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxId,
    WorkerDispatchProtocol, WorkerId,
};
use trogon_sandbox_protocol::subjects::{provider_dispatch, worker_cleanup, worker_dispatch};
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

/// Whether the target is bound to the worker its heartbeat names.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Selection {
    Selected,
    Unselected,
}

struct Receipt {
    outcome: Result<(), CheckFailureError>,
    ledger: GrantLedger,
    native: ProviderLease,
    assignment: Option<DispatchTarget>,
    cancel: Option<VerbScope>,
}

fn worker() -> Result<WorkerId, Box<dyn Error>> {
    Ok(WorkerId::new("selected-worker")?)
}

fn signing() -> Result<(Arc<DispatchSigner>, DispatchVerifier), Box<dyn Error>> {
    let signer = Arc::new(DispatchSigner::new("selected-worker-test", [9; 32]));
    let verifier = DispatchVerifier::new(EnforcementMode::Require)
        .with_key(signer.key_id(), &signer.public_key_b64())?
        .with_epoch_mode(EpochMode::Require);
    Ok((signer, verifier))
}

fn lease_is_granted(target: Target) -> Result<JoinHandle<Result<(), CheckFailureError>>, Box<dyn Error>> {
    let (_, check) = checks_for_target(&target)
        .into_iter()
        .find(|(name, _)| *name == "lease_is_granted")
        .ok_or("the grant check is in the suite")?;
    Ok(tokio::spawn(async move { check(&target).await }))
}

fn skewed(mut native: ProviderLease, case: GrantCase) -> Result<ProviderLease, Box<dyn Error>> {
    match case {
        GrantCase::Exact => {}
        GrantCase::ForeignWorker => native.worker_id = Some(WorkerId::new("foreign-worker")?),
        GrantCase::MissingWorker => native.worker_id = None,
        GrantCase::ForeignAttempt => native.attempt_id = Some(DispatchAttemptId::new("foreign-attempt")?),
        GrantCase::MissingAttempt => native.attempt_id = None,
        GrantCase::ForeignEpoch => native.epoch = Some(LeaseEpoch::new(2)?),
        GrantCase::MissingEpoch => native.epoch = None,
    }
    Ok(native)
}

/// Answers the typed cleanup of an assigned grant and returns the scope its signature binds.
async fn assigned_cleanup(
    client: &async_nats::Client,
    cleanup: &mut async_nats::Subscriber,
    verifier: &DispatchVerifier,
    request: &ProviderDispatchPayload,
    assignment: &DispatchTarget,
) -> Result<VerbScope, Box<dyn Error>> {
    let message = wire::local(wire::next(cleanup).await)?;
    let cleanup: ProviderCleanupRequest = serde_json::from_slice(&message.payload)?;
    assert_eq!(
        cleanup.identity,
        LeaseAttemptIdentity {
            lease_id: request.lease_id.clone(),
            provider: KIND,
            target: assignment.clone(),
        }
    );
    assert_eq!(cleanup.intent, CancelIntent::Release);
    let verified = verifier
        .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)?
        .ok_or("the assigned cleanup is not signed")?;
    assert!(verified.matches(message.subject.as_str(), &message.payload));
    let scope = verified.scope().clone();
    assert_eq!(scope.tenant, request.request.tenant);
    assert_eq!(scope.lease_id, request.lease_id);
    let answer = ProviderCleanupResponse::Cleaned {
        identity: cleanup.identity,
        intent: cleanup.intent,
    };
    wire::local(wire::reply(client, &message, &answer).await)?;
    Ok(scope)
}

async fn grant_case(
    server: &CoreTestServer,
    protocol: WorkerDispatchProtocol,
    selection: Selection,
    case: GrantCase,
) -> Result<Receipt, Box<dyn Error>> {
    let client = async_nats::connect(server.address()).await?;
    let worker = worker()?;
    let selected = heartbeat(KIND, &worker, protocol, CapabilitySupport::Supported);
    let (signer, verifier) = signing()?;
    let ledger = GrantLedger::default();
    let mut target = Target::new(client.clone(), KIND);
    if selection == Selection::Selected {
        target = target.for_worker(&selected)?;
    }
    target = target.with_grant_ledger(ledger.clone());
    target.exec = ExecModel::None;
    target.signer = Some(signer);
    target.timeout = wire::WAIT;
    let assigned = selection == Selection::Selected && protocol == WorkerDispatchProtocol::TargetedV1;
    let subject = if assigned {
        worker_dispatch(KIND, &worker)
    } else {
        provider_dispatch(KIND)
    };
    let mut dispatch = client.subscribe(subject).await?;
    let mut controls = client.subscribe("sandbox.provider.mock.*.*").await?;
    let mut cleanup = client.subscribe(worker_cleanup(KIND, &worker)).await?;
    client.flush().await?;
    let check = lease_is_granted(target)?;
    let message = wire::local(wire::next(&mut dispatch).await)?;
    let request: ProviderDispatchPayload =
        verifier.decode(&message.payload, OP_LEASE_DISPATCH, assigned.then_some(&worker))?;
    let assignment = request.target.clone();
    assert_eq!(assignment.is_some(), assigned);
    let native = skewed(
        ProviderLease {
            sandbox_id: SandboxId::new(format!("native-{}", request.lease_id.as_str()))?,
            evidence_subject: format!("sandbox.evidence.{}", request.lease_id.as_str()),
            worker_id: Some(worker.clone()),
            attempt_id: assignment.as_ref().map(|target| target.attempt_id.clone()),
            epoch: assignment.as_ref().and_then(|target| target.epoch),
        },
        case,
    )?;
    wire::local(wire::reply(&client, &message, &native).await)?;
    let assigned_cancel = match &assignment {
        Some(assignment) => Some(assigned_cleanup(&client, &mut cleanup, &verifier, &request, assignment).await?),
        None => None,
    };
    let outcome = tokio::time::timeout(Duration::from_secs(10), check).await??;
    let cancel = match assigned_cancel {
        Some(scope) => Some(scope),
        None => match tokio::time::timeout(Duration::from_millis(200), controls.next()).await {
            Ok(Some(message)) => {
                assert!(message.subject.as_str().contains(".cancel."), "{}", message.subject);
                verifier
                    .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)?
                    .map(|verified| verified.scope().clone())
            }
            Ok(None) | Err(_) => None,
        },
    };
    let grants = ledger.snapshot();
    assert_eq!(grants.len(), 1, "a complete mismatch stays visible for reconciliation");
    assert_eq!(grants[0].lease_id, request.lease_id);
    assert_eq!(grants[0].native, native);
    Ok(Receipt {
        outcome,
        ledger,
        native,
        assignment,
        cancel,
    })
}

fn assert_scope(cancel: &VerbScope, assignment: &DispatchTarget) {
    assert_eq!(cancel.worker_id.as_ref(), Some(&assignment.worker_id));
    assert_eq!(cancel.attempt_id.as_ref(), Some(&assignment.attempt_id));
    assert_eq!(cancel.epoch, assignment.epoch);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_selector_refuses_a_foreign_provider_or_profile() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = worker()?;
    let protocol = WorkerDispatchProtocol::TargetedV1;
    let mut foreign_provider = heartbeat(KIND, &worker, protocol, CapabilitySupport::Supported);
    foreign_provider.provider = ProviderKind::E2b;
    let mut foreign_profile = heartbeat(KIND, &worker, protocol, CapabilitySupport::Supported);
    foreign_profile.profile.provider = ProviderKind::E2b;
    for invalid in [foreign_provider, foreign_profile] {
        assert!(Target::new(client.clone(), KIND).for_worker(&invalid).is_err());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_assigned_target_without_a_signer_never_sends_a_dispatch() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut dispatches = client.subscribe("sandbox.provider.mock.>").await?;
    client.flush().await?;
    let ledger = GrantLedger::default();
    let selected = heartbeat(
        KIND,
        &worker()?,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Supported,
    );
    let target = Target::new(client.clone(), KIND)
        .for_worker(&selected)?
        .with_grant_ledger(ledger.clone());
    let outcome = lease_is_granted(target)?.await?;
    assert!(outcome.is_err_and(|error| matches!(
        error,
        CheckFailureError::Assignment(AssignmentFaultError::SignerRequired)
    )));
    client.flush().await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), dispatches.next())
            .await
            .is_err(),
        "missing signing authority fails before any provider effect"
    );
    assert!(ledger.snapshot().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exact_assigned_grant_is_released_with_its_requested_cancel_scope() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let receipt = grant_case(
        &server,
        WorkerDispatchProtocol::TargetedV1,
        Selection::Selected,
        GrantCase::Exact,
    )
    .await?;
    assert!(receipt.outcome.is_ok(), "{:?}", receipt.outcome);
    let assignment = receipt.assignment.ok_or("the dispatch was assigned")?;
    let cancel = receipt.cancel.ok_or("the exact owned grant is released")?;
    assert_scope(&cancel, &assignment);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn assigned_grant_mismatches_are_refused_without_borrowing_reply_authority() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    for case in [
        GrantCase::ForeignWorker,
        GrantCase::MissingWorker,
        GrantCase::ForeignAttempt,
        GrantCase::MissingAttempt,
        GrantCase::ForeignEpoch,
        GrantCase::MissingEpoch,
    ] {
        let receipt = grant_case(&server, WorkerDispatchProtocol::TargetedV1, Selection::Selected, case).await?;
        assert!(receipt.outcome.is_err(), "assigned {case:?} is refused");
        let assignment = receipt.assignment.ok_or("the dispatch was assigned")?;
        if let Some(cancel) = receipt.cancel {
            assert_scope(&cancel, &assignment);
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_exact_selected_legacy_grant_remains_usable() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let receipt = grant_case(
        &server,
        WorkerDispatchProtocol::Legacy,
        Selection::Selected,
        GrantCase::Exact,
    )
    .await?;
    assert!(receipt.outcome.is_ok(), "{:?}", receipt.outcome);
    assert!(receipt.assignment.is_none());
    assert!(receipt.cancel.is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unselected_legacy_target_keeps_accepting_unbound_grants() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let receipt = grant_case(
        &server,
        WorkerDispatchProtocol::Legacy,
        Selection::Unselected,
        GrantCase::ForeignWorker,
    )
    .await?;
    assert!(receipt.outcome.is_ok(), "{:?}", receipt.outcome);
    assert!(receipt.assignment.is_none());
    assert!(receipt.cancel.is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selected_legacy_target_refuses_missing_or_foreign_worker_grants() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let mut accepted = Vec::new();
    for case in [GrantCase::ForeignWorker, GrantCase::MissingWorker] {
        let receipt = grant_case(&server, WorkerDispatchProtocol::Legacy, Selection::Selected, case).await?;
        assert_eq!(receipt.ledger.snapshot()[0].native, receipt.native);
        if receipt.outcome.is_ok() {
            accepted.push(format!("{case:?}"));
        }
    }
    assert!(
        accepted.is_empty(),
        "selected legacy targets accepted nonmatching worker grants: {accepted:?}"
    );
    Ok(())
}
