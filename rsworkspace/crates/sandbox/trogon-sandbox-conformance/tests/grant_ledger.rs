//! A grant ledger keeps every native grant a worker handed out, so exact cleanup survives a failed check.

mod support;

use std::error::Error;
use std::time::Duration;

use support::wire::{self, Sendable};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{CheckFailureError, GrantLedger, Target, checks_for_target};
use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, DispatchAttemptId, LeaseEpoch, LeaseId, OutputTruncation,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandResponse, SandboxId, WorkerId,
};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch, provider_run_command};

const KIND: ProviderKind = ProviderKind::Mock;

/// What the hand-written worker does with the one dispatch it gets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grant {
    Refused,
    Granted,
    /// Granted, then a command answers with output the check does not expect.
    GrantedThenWrong,
}

fn native(lease_id: &LeaseId, suffix: &str) -> Sendable<ProviderLease> {
    Ok(ProviderLease {
        sandbox_id: SandboxId::new(format!("native-{}{suffix}", lease_id.as_str()))?,
        evidence_subject: format!("sandbox.evidence.{}", lease_id.as_str()),
        worker_id: Some(WorkerId::new("fixture-worker")?),
        attempt_id: Some(DispatchAttemptId::new("fixture-attempt")?),
        epoch: Some(LeaseEpoch::new(7)?),
    })
}

async fn exercise(grant: Grant) -> Result<(GrantLedger, Option<(LeaseId, ProviderLease)>), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let ledger = GrantLedger::default();
    let mut target = Target::new(client.clone(), KIND).with_grant_ledger(ledger.clone());
    target.timeout = Duration::from_secs(3);
    let mut dispatch = client.subscribe(provider_dispatch(KIND)).await?;
    let mut run = client.subscribe("sandbox.provider.mock.run_command.*").await?;
    let mut cancel = client.subscribe("sandbox.provider.mock.cancel.*").await?;
    client.flush().await?;
    let responder = tokio::spawn(async move {
        let request = wire::next(&mut dispatch).await?;
        let payload: ProviderDispatchPayload = wire::body(&request)?;
        if grant == Grant::Refused {
            wire::reply_raw(
                &client,
                &request,
                &br#"{"provider_error":"fixture refused allocation"}"#[..],
            )
            .await?;
            return Sendable::Ok(None);
        }
        let native = native(&payload.lease_id, "")?;
        wire::reply(&client, &request, &native).await?;
        if grant == Grant::GrantedThenWrong {
            let command = wire::next(&mut run).await?;
            let subject = provider_run_command(KIND, &payload.lease_id);
            if command.subject.as_str() != subject {
                return Err(format!("the command went to {}, not {subject}", command.subject).into());
            }
            let output = RunCommandResponse::Completed(CommandOutput {
                exit_code: 0,
                stdout: b"deliberately-wrong-output".to_vec(),
                stderr: vec![],
                timed_out: CommandTimeoutStatus::WithinDeadline,
                truncated: OutputTruncation::Complete,
            });
            wire::reply(&client, &command, &output).await?;
        }
        let released = wire::next(&mut cancel).await?;
        let subject = provider_cancel(KIND, &payload.lease_id);
        if released.subject.as_str() != subject {
            return Err(format!("the cancel went to {}, not {subject}", released.subject).into());
        }
        Ok(Some((payload.lease_id, native)))
    });
    let name = match grant {
        Grant::GrantedThenWrong => "run_returns_stdout_and_exit_zero",
        Grant::Refused | Grant::Granted => "lease_is_granted",
    };
    let (_, check) = checks_for_target(&target)
        .into_iter()
        .find(|(candidate, _)| *candidate == name)
        .ok_or("the check is in the suite")?;
    let outcome = check(&target).await;
    assert_eq!(outcome.is_ok(), grant == Grant::Granted, "{outcome:?}");
    let granted = wire::join(responder).await?;
    Ok((ledger, granted))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_successful_allocation_retains_its_exact_native_grant_for_cleanup() -> Result<(), Box<dyn Error>> {
    let (ledger, granted) = exercise(Grant::Granted).await?;
    let (lease_id, native) = granted.ok_or("the lease was granted")?;
    let recorded = ledger.snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].provider, KIND);
    assert_eq!(recorded[0].lease_id, lease_id);
    assert_eq!(recorded[0].native, native);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_check_keeps_its_prior_exact_native_grant() -> Result<(), Box<dyn Error>> {
    let (ledger, granted) = exercise(Grant::GrantedThenWrong).await?;
    let (lease_id, native) = granted.ok_or("the lease was granted")?;
    let recorded = ledger.snapshot();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].lease_id, lease_id);
    assert_eq!(recorded[0].native, native);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_allocation_never_invents_a_native_identity() -> Result<(), Box<dyn Error>> {
    let (ledger, granted) = exercise(Grant::Refused).await?;
    assert!(granted.is_none());
    assert!(ledger.snapshot().is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn divergent_redispatch_grants_remain_available_for_exact_cleanup() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let ledger = GrantLedger::default();
    let mut target = Target::new(client.clone(), KIND).with_grant_ledger(ledger.clone());
    target.timeout = Duration::from_secs(3);
    target.redispatch_idempotent = true;
    let mut dispatch = client.subscribe(provider_dispatch(KIND)).await?;
    let mut write = client.subscribe("sandbox.provider.mock.write_files.*").await?;
    let mut cancel = client.subscribe("sandbox.provider.mock.cancel.*").await?;
    client.flush().await?;
    let responder = tokio::spawn(async move {
        let mut granted = Vec::new();
        tokio::time::timeout(wire::WAIT, async {
            loop {
                tokio::select! {
                    request = wire::next(&mut dispatch) => {
                        let request = request?;
                        let payload: ProviderDispatchPayload = wire::body(&request)?;
                        // Every dispatch of the one lease lands on a different sandbox.
                        let native = native(&payload.lease_id, &format!("-{}", granted.len()))?;
                        wire::reply(&client, &request, &native).await?;
                        granted.push((payload.lease_id, native));
                    }
                    request = wire::next(&mut write) => {
                        wire::reply_raw(&client, &request?, &br#"{"outcome":"written"}"#[..]).await?;
                    }
                    request = wire::next(&mut cancel) => {
                        request?;
                        return Sendable::Ok(());
                    }
                }
            }
        })
        .await??;
        Sendable::Ok(granted)
    });
    let (_, check) = checks_for_target(&target)
        .into_iter()
        .find(|(name, _)| *name == "redispatching_a_lease_keeps_its_state")
        .ok_or("the check is in the suite")?;
    let outcome = check(&target).await;
    assert!(
        outcome.is_err_and(|error| matches!(
            error,
            CheckFailureError::Mismatch { expected, .. } if expected == "a re-dispatch to keep one sandbox"
        )),
        "divergent grants are refused"
    );
    let granted = wire::join(responder).await?;
    assert_eq!(granted.len(), 3);
    let recorded = ledger.snapshot();
    assert_eq!(recorded.len(), granted.len());
    for (record, (lease_id, native)) in recorded.iter().zip(granted) {
        assert_eq!(record.provider, KIND);
        assert_eq!(record.lease_id, lease_id);
        assert_eq!(record.native, native);
    }
    Ok(())
}
