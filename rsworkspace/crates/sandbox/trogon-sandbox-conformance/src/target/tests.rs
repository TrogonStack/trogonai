use std::error::Error;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_contract::{CapabilitySupport, WorkerId};
use trogon_sandbox_protocol::subjects::provider_token;

use super::*;

type Outcome = Result<(), Box<dyn Error>>;

const KIND: ProviderKind = ProviderKind::Subprocess;

async fn fixture() -> Result<(CoreTestServer, Target), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    Ok((server, Target::new(client, KIND)))
}

fn signer() -> Arc<DispatchSigner> {
    Arc::new(DispatchSigner::new("k1", [7; 32]))
}

fn assign(
    target: &mut Target,
    protocol: WorkerDispatchProtocol,
    epoch: CapabilitySupport,
) -> Result<WorkerId, Box<dyn Error>> {
    let id = WorkerId::new("w1")?;
    target.selected_worker = Some(SelectedWorker {
        id: id.clone(),
        protocol,
        supports_lease_epoch: epoch,
    });
    Ok(id)
}

fn faulted<T>(result: Result<T, CheckFailureError>, fault: AssignmentFaultError) -> bool {
    matches!(result, Err(CheckFailureError::Assignment(found)) if found == fault)
}

fn grant(
    worker: Option<&WorkerId>,
    assignment: Option<&DispatchTarget>,
    evidence: &str,
) -> Result<ProviderLease, Box<dyn Error>> {
    let mut lease = json!({ "sandbox_id": "s", "evidence_subject": evidence });
    lease["worker_id"] = json!(worker);
    lease["attempt_id"] = json!(assignment.map(|target| &target.attempt_id));
    lease["epoch"] = json!(assignment.and_then(|target| target.epoch));
    Ok(serde_json::from_value(lease)?)
}

const DISPATCH: &[u8] = br#"{"lease_id":"l1","request":{"tenant":"conformance"}}"#;

fn verb_subject(lease: &LeaseId) -> String {
    format!("sandbox.provider.{}.run_command.{lease}", provider_token(KIND))
}

fn cleanup_request(target: DispatchTarget, lease: &LeaseId) -> ProviderCleanupRequest {
    let identity = LeaseAttemptIdentity {
        lease_id: lease.clone(),
        provider: KIND,
        target,
    };
    ProviderCleanupRequest {
        identity,
        intent: CancelIntent::Release,
    }
}

async fn answer_cleanups(client: Client, subject: String, confirm: bool) -> Outcome {
    let mut subscription = client.subscribe(subject).await?;
    client.flush().await?;
    tokio::spawn(async move {
        while let Some(message) = subscription.next().await {
            let Some(reply) = message.reply.clone() else { continue };
            let Ok(bytes) = nats_transfer::receive(&client, &message).await else {
                continue;
            };
            let Ok(request) = serde_json::from_slice::<ProviderCleanupRequest>(&bytes) else {
                continue;
            };
            let (identity, intent) = (request.identity, request.intent);
            let response = if confirm {
                ProviderCleanupResponse::Cleaned { identity, intent }
            } else {
                ProviderCleanupResponse::Failed {
                    identity,
                    message: "no".to_owned(),
                }
            };
            let Ok(body) = serde_json::to_vec(&response) else {
                continue;
            };
            let _ = nats_transfer::respond(&client, reply.as_str(), Bytes::from(body)).await;
        }
    });
    Ok(())
}

#[tokio::test]
async fn a_grant_is_judged_against_what_the_suite_assigned() -> Outcome {
    let (_server, mut target) = fixture().await?;
    let id = LeaseId::new("l1");
    let lease = grant(None, None, "e")?;
    assert_eq!(
        target.validate_grant(&id, &lease),
        Ok(NativeCleanupAuthority::ReconciliationRequired)
    );
    assert_eq!(
        target.validate_grant(&id, &grant(None, None, "")?),
        Err(AssignmentFaultError::GrantMissingEvidence)
    );

    let worker = assign(
        &mut target,
        WorkerDispatchProtocol::Legacy,
        CapabilitySupport::Unsupported,
    )?;
    let named = grant(Some(&worker), None, "e")?;
    assert_eq!(
        target.validate_grant(&id, &named),
        Ok(NativeCleanupAuthority::RequestMatched)
    );
    assert_eq!(
        target.validate_grant(&id, &lease),
        Err(AssignmentFaultError::GrantDiffersFromSelectedWorker)
    );

    assign(
        &mut target,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Supported,
    )?;
    assert_eq!(
        target.validate_grant(&id, &named),
        Err(AssignmentFaultError::GrantWithoutAssignment)
    );
    let assignment = target.assignment(&id)?.ok_or("no assignment")?;
    let exact = grant(Some(&assignment.worker_id), Some(&assignment), "e")?;
    assert_eq!(
        target.validate_grant(&id, &exact),
        Ok(NativeCleanupAuthority::RequestMatched)
    );
    assert_eq!(
        target.validate_grant(&id, &named),
        Err(AssignmentFaultError::GrantDiffersFromAssignment)
    );
    Ok(())
}

#[tokio::test]
async fn an_assignment_is_kept_per_lease_and_epochs_follow_the_worker() -> Outcome {
    let (_server, mut target) = fixture().await?;
    let id = LeaseId::new("l1");
    assert!(target.assignment(&id)?.is_none());
    assign(
        &mut target,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Supported,
    )?;
    let first = target.assignment(&id)?.ok_or("no assignment")?;
    assert_eq!(first.epoch, Some(LeaseEpoch::FIRST));
    assert_eq!(target.assignment(&id)?, Some(first.clone()));
    assert_eq!(
        target.dispatch_subject(Some(&first)),
        worker_dispatch(KIND, &first.worker_id)
    );
    assert_eq!(target.dispatch_subject(None), provider_dispatch(KIND));
    assign(
        &mut target,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Unsupported,
    )?;
    let other = target.assignment(&LeaseId::new("l2"))?.ok_or("no assignment")?;
    assert_eq!(other.epoch, None);
    Ok(())
}

#[tokio::test]
async fn an_admitted_grant_is_recorded_even_when_it_is_refused() -> Outcome {
    let (_server, target) = fixture().await?;
    let ledger = GrantLedger::default();
    let target = target.with_grant_ledger(ledger.clone());
    let id = LeaseId::new("l1");
    assert_eq!(target.admit_grant(&id, &grant(None, None, "e")?), Ok(()));
    assert_eq!(
        target.admit_grant(&id, &grant(None, None, "")?),
        Err(AssignmentFaultError::GrantMissingEvidence)
    );
    let authorities: Vec<_> = ledger
        .snapshot()
        .iter()
        .map(|grant| grant.native_cleanup_authority)
        .collect();
    assert_eq!(authorities, [NativeCleanupAuthority::ReconciliationRequired; 2]);
    Ok(())
}

#[tokio::test]
async fn requests_are_signed_the_way_the_subject_requires() -> Outcome {
    let (_server, mut target) = fixture().await?;
    let lease = LeaseId::new("l1");
    let subject = verb_subject(&lease);
    let (plain, headers) = target.sign(&subject, b"{}".to_vec())?;
    assert!(plain == b"{}" && headers.is_none());

    target.signer = Some(signer());
    let (signed, headers) = target.sign(&provider_dispatch(KIND), DISPATCH.to_vec())?;
    assert!(signed != DISPATCH && headers.is_none());
    let (body, headers) = target.sign(&subject, b"{}".to_vec())?;
    assert!(body == b"{}" && headers.is_some_and(|headers| headers.get(AUTH_HEADER).is_some()));
    let (body, headers) = target.sign("other.subject", b"{}".to_vec())?;
    assert!(body == b"{}" && headers.is_none());
    Ok(())
}

#[tokio::test]
async fn an_assigned_worker_is_only_reached_with_a_signed_assignment() -> Outcome {
    let (_server, mut target) = fixture().await?;
    let worker = assign(
        &mut target,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Supported,
    )?;
    let lease = LeaseId::new("l1");
    assert!(faulted(
        target.sign(&verb_subject(&lease), Vec::new()),
        AssignmentFaultError::SignerRequired
    ));

    target.signer = Some(signer());
    assert!(faulted(
        target.sign(&verb_subject(&lease), Vec::new()),
        AssignmentFaultError::RequestWithoutAssignment
    ));
    let (signed, headers) = target.sign(&worker_dispatch(KIND, &worker), DISPATCH.to_vec())?;
    assert!(signed != DISPATCH && headers.is_none());
    target.assignment(&lease)?;
    let (_, headers) = target.sign(&verb_subject(&lease), Vec::new())?;
    assert!(headers.is_some());
    Ok(())
}

#[tokio::test]
async fn a_cleanup_is_signed_only_for_the_retained_assignment() -> Outcome {
    let (_server, mut target) = fixture().await?;
    target.signer = Some(signer());
    let lease = LeaseId::new("l1");
    let other = WorkerId::new("w2")?;
    let stray = DispatchTarget {
        worker_id: other.clone(),
        attempt_id: ids::fresh_attempt_id()?,
        epoch: None,
    };
    let body = serde_json::to_vec(&cleanup_request(stray.clone(), &lease))?;
    let subject = worker_cleanup(KIND, &other);
    assert!(faulted(
        target.sign(&subject, body.clone()),
        AssignmentFaultError::CleanupWithoutWorker
    ));
    assert!(matches!(
        target.sign(&subject, b"x".to_vec()),
        Err(CheckFailureError::Decode { .. })
    ));

    let worker = assign(
        &mut target,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Supported,
    )?;
    assert!(faulted(
        target.sign(&subject, body),
        AssignmentFaultError::CleanupWithoutAssignment
    ));
    let assignment = target.assignment(&lease)?.ok_or("no assignment")?;
    let right = serde_json::to_vec(&cleanup_request(assignment.clone(), &lease))?;
    let (_, headers) = target.sign(&worker_cleanup(KIND, &worker), right.clone())?;
    assert!(headers.is_some());
    assert!(faulted(
        target.sign(&subject, right),
        AssignmentFaultError::CleanupDiffers
    ));
    let wrong = serde_json::to_vec(&cleanup_request(stray, &lease))?;
    assert!(faulted(
        target.sign(&worker_cleanup(KIND, &worker), wrong),
        AssignmentFaultError::CleanupDiffers
    ));
    Ok(())
}

#[tokio::test]
async fn cancelling_an_unassigned_lease_publishes_to_the_cancel_subject() -> Outcome {
    let (_server, mut target) = fixture().await?;
    let lease = LeaseId::new("l1");
    let mut subscription = target.client.subscribe(provider_cancel(KIND, &lease)).await?;
    target.client.flush().await?;
    target.cancel(&lease).await;
    let unsigned = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await?
        .ok_or("no message")?;
    assert!(unsigned.headers.is_none());
    target.signer = Some(signer());
    target.cancel(&lease).await;
    let signed = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await?
        .ok_or("no message")?;
    assert!(signed.headers.is_some());
    Ok(())
}

#[tokio::test]
async fn cancelling_an_assigned_lease_asks_its_worker_to_clean_up() -> Outcome {
    let (_server, mut target) = fixture().await?;
    target.signer = Some(signer());
    target.timeout = Duration::from_secs(5);
    let worker = assign(
        &mut target,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Supported,
    )?;
    let (kept, lost) = (LeaseId::new("kept"), LeaseId::new("lost"));
    assert!(faulted(
        target.release_assigned(&lost).await,
        AssignmentFaultError::CleanupWithoutAssignment
    ));
    target.cancel(&lost).await;

    target.assignment(&kept)?;
    let subject = worker_cleanup(KIND, &worker);
    answer_cleanups(target.client.clone(), subject.clone(), true).await?;
    assert!(target.release_assigned(&kept).await.is_ok());
    target.cancel(&kept).await;
    Ok(())
}

#[tokio::test]
async fn a_worker_that_does_not_confirm_the_cleanup_is_reported() -> Outcome {
    let (_server, mut target) = fixture().await?;
    target.signer = Some(signer());
    target.timeout = Duration::from_secs(5);
    let worker = assign(
        &mut target,
        WorkerDispatchProtocol::TargetedV1,
        CapabilitySupport::Supported,
    )?;
    let lease = LeaseId::new("l1");
    target.assignment(&lease)?;
    answer_cleanups(target.client.clone(), worker_cleanup(KIND, &worker), false).await?;
    assert!(faulted(
        target.release_assigned(&lease).await,
        AssignmentFaultError::CleanupNotConfirmed
    ));
    Ok(())
}

#[tokio::test]
async fn a_request_that_gets_no_answer_is_a_transport_failure() -> Outcome {
    let (_server, target) = fixture().await?;
    let reply: Result<serde_json::Value, CheckFailureError> = target
        .call("nobody.listens".to_owned(), &json!({}), Duration::from_millis(200))
        .await;
    assert!(matches!(reply, Err(CheckFailureError::Transport { .. })));
    Ok(())
}

#[tokio::test]
async fn the_lease_request_follows_the_target_spec_and_network_policy() -> Outcome {
    let (_server, mut target) = fixture().await?;
    let request = target.lease_request_with_network(NetworkPolicy::DenyAll)?;
    assert_eq!(request.spec.network_policy(), Some(&NetworkPolicy::DenyAll));
    target.spec = Some(request.spec);
    let again = target.lease_request()?;
    assert_eq!(again.spec.network_policy(), Some(&NetworkPolicy::DenyAll));
    Ok(())
}

#[tokio::test]
async fn no_key_in_the_environment_leaves_the_target_unsigned() -> Outcome {
    let (_server, target) = fixture().await?;
    assert!(
        target
            .signed_from_env(&trogon_std::env::InMemoryEnv::new())?
            .signer
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn a_reply_the_contract_does_not_describe_is_a_decode_failure() -> Outcome {
    let (_server, target) = fixture().await?;
    let mut subscription = target.client.subscribe("decode.me").await?;
    target.client.flush().await?;
    let client = target.client.clone();
    tokio::spawn(async move {
        while let Some(message) = subscription.next().await {
            let Some(reply) = message.reply else { continue };
            let _ = nats_transfer::respond(&client, reply.as_str(), Bytes::from_static(b"not json")).await;
        }
    });
    let reply: Result<serde_json::Value, CheckFailureError> = target
        .call("decode.me".to_owned(), &json!({}), Duration::from_secs(5))
        .await;
    assert!(matches!(reply, Err(CheckFailureError::Decode { .. })));
    Ok(())
}
