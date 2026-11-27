//! On the dedicated TLS provider the firewall accepts TCP and drops application traffic, so a connected socket proves
//! nothing. The denied-connect check there counts a lease as denied only when an authenticated TLS control before and
//! after it reaches the same peer and the restricted handshake is ended by the network, with complete diagnostics.

mod support;

use std::collections::BTreeSet;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::{Client, Message};
use futures::StreamExt;
use support::locked;
use support::tls_egress::{self, Event, Phase};
use support::wire::Sendable;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{CheckFailureError, GrantLedger, Target, checks};
use trogon_sandbox_contract::{
    CommandTimeoutStatus, LeaseError, NetworkPolicy, OutputTruncation, ProviderDispatchPayload, ProviderKind,
    ProviderLease, RunCommandRequest, RunCommandResponse, SandboxId,
};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch, provider_run_command};

const KIND: ProviderKind = ProviderKind::E2b;

/// How the worker spoils the restricted lease's command result.
#[derive(Clone, Copy, Debug)]
enum CommandFailure {
    Refused,
    Malformed,
    Lost,
    Exit,
    Stderr,
    TimedOut,
    Truncated,
    BlockedOutput(&'static str),
}

#[derive(Default)]
struct Recorded {
    dispatches: Vec<ProviderDispatchPayload>,
    commands: Vec<RunCommandRequest>,
    events: Vec<Event>,
    canceled: BTreeSet<String>,
}

struct Receipt {
    outcome: Result<(), CheckFailureError>,
    recorded: Arc<Mutex<Recorded>>,
}

fn policy(dispatch: &ProviderDispatchPayload) -> Sendable<NetworkPolicy> {
    dispatch
        .request
        .spec
        .network_policy()
        .cloned()
        .ok_or_else(|| "every egress lease names its policy".into())
}

struct Worker {
    client: Client,
    state: Arc<Mutex<Recorded>>,
    scenario: &'static str,
    failure: Option<CommandFailure>,
}

impl Worker {
    fn dispatch(&self, message: &Message) -> Sendable<Option<Vec<u8>>> {
        let dispatch: ProviderDispatchPayload = serde_json::from_slice(&message.payload)?;
        let native = ProviderLease {
            sandbox_id: SandboxId::new(format!("tls-egress-{}", dispatch.lease_id.as_str()))?,
            evidence_subject: format!("sandbox.evidence.{}", dispatch.lease_id.as_str()),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        };
        locked(&self.state).dispatches.push(dispatch);
        Ok(Some(serde_json::to_vec(&native)?))
    }

    async fn command(
        &self,
        message: &Message,
        known: &[ProviderDispatchPayload],
        held: &ProviderDispatchPayload,
    ) -> Sendable<Option<Vec<u8>>> {
        let command: RunCommandRequest = serde_json::from_slice(&message.payload)?;
        let restricted = policy(held)? == NetworkPolicy::DenyAll;
        let mut after = false;
        for dispatch in known {
            after |= policy(dispatch)? == NetworkPolicy::DenyAll;
        }
        let phase = match (restricted, after) {
            (true, _) => Phase::Restricted,
            (false, true) => Phase::After,
            (false, false) => Phase::Before,
        };
        let (mut output, events) = tls_egress::execute(&command, self.scenario, phase).await?;
        {
            let mut state = locked(&self.state);
            state.commands.push(command);
            state.events.extend(events);
        }
        let response = match self.failure.filter(|_| restricted) {
            Some(CommandFailure::Refused) => RunCommandResponse::Denied(LeaseError::Backend {
                message: "scripted execution refusal".to_owned(),
            }),
            Some(CommandFailure::Malformed) => return Ok(Some(b"not a command reply".to_vec())),
            Some(CommandFailure::Lost) => return Ok(None),
            spoiled => {
                match spoiled {
                    Some(CommandFailure::Exit) => output.exit_code = 127,
                    Some(CommandFailure::Stderr) => output.stderr = b"scripted error".to_vec(),
                    Some(CommandFailure::TimedOut) => output.timed_out = CommandTimeoutStatus::TimedOut,
                    Some(CommandFailure::Truncated) => output.truncated = OutputTruncation::Truncated,
                    Some(CommandFailure::BlockedOutput(value)) => output.stdout = value.as_bytes().to_vec(),
                    Some(CommandFailure::Refused | CommandFailure::Malformed | CommandFailure::Lost) | None => {}
                }
                RunCommandResponse::Completed(output)
            }
        };
        Ok(Some(serde_json::to_vec(&response)?))
    }

    async fn answer(&self, message: Message) -> Sendable<()> {
        let subject = message.subject.as_str();
        let reply = if subject == provider_dispatch(KIND) {
            self.dispatch(&message)?
        } else {
            let known = locked(&self.state).dispatches.clone();
            if let Some(dispatch) = known
                .iter()
                .find(|dispatch| subject == provider_cancel(KIND, &dispatch.lease_id))
            {
                locked(&self.state)
                    .canceled
                    .insert(dispatch.lease_id.as_str().to_owned());
                None
            } else if let Some(held) = known
                .iter()
                .find(|dispatch| subject == provider_run_command(KIND, &dispatch.lease_id))
            {
                self.command(&message, &known, held).await?
            } else {
                None
            }
        };
        if let (Some(subject), Some(reply)) = (message.reply, reply) {
            self.client.publish(subject, reply.into()).await?;
        }
        Ok(())
    }
}

async fn exercise(scenario: &'static str, failure: Option<CommandFailure>) -> Result<Receipt, Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut messages = client.subscribe("sandbox.provider.>").await?;
    client.flush().await?;
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let serving = Worker {
        client: client.clone(),
        state: recorded.clone(),
        scenario,
        failure,
    };
    let worker = tokio::spawn(async move {
        while let Some(message) = messages.next().await {
            serving.answer(message).await?;
        }
        Sendable::Ok(())
    });
    let ledger = GrantLedger::default();
    let mut target = Target::new(client.clone(), KIND).with_grant_ledger(ledger.clone());
    assert_eq!(target.egress_probe_address.to_string(), "1.1.1.1:443");
    target.timeout = Duration::from_secs(2);
    let (_, check) = checks()
        .into_iter()
        .find(|(name, _)| *name == "denied_connect_fails")
        .ok_or("the egress check is in the suite")?;
    let outcome = tokio::time::timeout(Duration::from_secs(20), check(&target)).await?;
    client.flush().await?;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let done = {
                let state = locked(&recorded);
                state.canceled.len() == state.dispatches.len()
            };
            if done || worker.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    if worker.is_finished() {
        support::wire::join(worker).await?;
        return Err("the worker stopped serving".into());
    }
    worker.abort();
    let state = locked(&recorded);
    let grants = ledger.snapshot();
    assert!(!grants.is_empty(), "the unrestricted control grant is kept");
    assert_eq!(grants.len(), state.dispatches.len());
    for dispatch in &state.dispatches {
        assert!(
            state.canceled.contains(dispatch.lease_id.as_str()),
            "every lease is released"
        );
        assert!(grants.iter().any(|grant| grant.lease_id == dispatch.lease_id));
    }
    assert!(
        state
            .commands
            .iter()
            .all(|command| command.timeout_seconds == 20 && command.max_output_bytes == Some(4096))
    );
    drop(state);
    Ok(Receipt { outcome, recorded })
}

fn policies(receipt: &Receipt) -> Result<Vec<NetworkPolicy>, Box<dyn Error>> {
    locked(&receipt.recorded)
        .dispatches
        .iter()
        .map(policy)
        .collect::<Sendable<Vec<_>>>()
        .map_err(|error| -> Box<dyn Error> { error })
}

fn observed(receipt: &Receipt, phase: Phase, event: &str) -> bool {
    locked(&receipt.recorded)
        .events
        .iter()
        .any(|seen| seen.phase == phase && seen.event == event)
}

fn healthy_controls(receipt: &Receipt) -> bool {
    [Phase::Before, Phase::After]
        .into_iter()
        .all(|phase| observed(receipt, phase, "tls_authenticated"))
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_tcp_without_tls_delivery_is_a_denial() -> Result<(), Box<dyn Error>> {
    let receipt = exercise("blocked", None).await?;
    assert!(
        receipt.outcome.is_ok(),
        "an intercepted connection delivers nothing: {:?}",
        receipt.outcome
    );
    assert_eq!(
        policies(&receipt)?,
        vec![NetworkPolicy::AllowAll, NetworkPolicy::DenyAll]
    );
    assert!(
        healthy_controls(&receipt),
        "both controls authenticate the original endpoint"
    );
    assert!(observed(&receipt, Phase::Restricted, "tcp_connected"));
    assert!(observed(&receipt, Phase::Restricted, "tls_blocked"));
    assert!(!observed(&receipt, Phase::Restricted, "tls_authenticated"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_restricted_tls_is_refused_after_both_controls() -> Result<(), Box<dyn Error>> {
    let receipt = exercise("delivered", None).await?;
    assert!(receipt.outcome.is_err(), "authenticated TLS proves delivery");
    assert!(healthy_controls(&receipt));
    assert!(observed(&receipt, Phase::Restricted, "tls_authenticated"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unusable_tls_preflight_never_acquires_a_restricted_grant() -> Result<(), Box<dyn Error>> {
    for scenario in ["missing_ssl", "missing_roots"] {
        let receipt = exercise(scenario, None).await?;
        assert!(receipt.outcome.is_err(), "{scenario} is not denial evidence");
        assert_eq!(policies(&receipt)?, vec![NetworkPolicy::AllowAll], "{scenario}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unhealthy_tls_controls_never_prove_denial() -> Result<(), Box<dyn Error>> {
    for scenario in [
        "control_before_certificate",
        "control_before_protocol",
        "control_before_transport",
        "control_before_endpoint",
    ] {
        let receipt = exercise(scenario, None).await?;
        assert!(receipt.outcome.is_err(), "{scenario} stays unknown");
        assert_eq!(policies(&receipt)?, vec![NetworkPolicy::AllowAll], "{scenario}");
    }
    for scenario in [
        "control_after_certificate",
        "control_after_protocol",
        "control_after_transport",
        "control_after_endpoint",
    ] {
        let receipt = exercise(scenario, None).await?;
        assert!(receipt.outcome.is_err(), "{scenario} stays unknown");
        assert!(observed(&receipt, Phase::Before, "tls_authenticated"), "{scenario}");
        assert!(
            observed(&receipt, Phase::After, "tcp_attempt"),
            "{scenario} still runs the after control"
        );
    }
    let receipt = exercise("control_after_changed_certificate", None).await?;
    assert!(receipt.outcome.is_err(), "a changed peer identity stays unknown");
    assert!(healthy_controls(&receipt));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn arbitrary_restricted_tls_errors_never_count_as_denial() -> Result<(), Box<dyn Error>> {
    for scenario in [
        "restricted_certificate",
        "restricted_protocol",
        "restricted_transport",
        "restricted_different_peer",
    ] {
        let receipt = exercise(scenario, None).await?;
        assert!(receipt.outcome.is_err(), "{scenario} does not earn denial");
        assert!(healthy_controls(&receipt), "{scenario}");
        assert!(observed(&receipt, Phase::Restricted, "tls_attempt"), "{scenario}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unusable_restricted_command_results_keep_the_after_control() -> Result<(), Box<dyn Error>> {
    for failure in [
        CommandFailure::Refused,
        CommandFailure::Malformed,
        CommandFailure::Lost,
        CommandFailure::Exit,
        CommandFailure::Stderr,
        CommandFailure::TimedOut,
        CommandFailure::Truncated,
    ] {
        let receipt = exercise("blocked", Some(failure)).await?;
        assert!(receipt.outcome.is_err(), "{failure:?} cannot prove denial");
        assert!(healthy_controls(&receipt), "{failure:?}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_extra_or_contradictory_blocked_diagnostics_never_prove_denial() -> Result<(), Box<dyn Error>> {
    for (case, output) in [
        ("malformed", r#"{"outcome":"blocked""#),
        ("extra", r#"{"outcome":"blocked","unexpected":true}"#),
        ("contradictory", r#"{"outcome":"blocked","authenticated_tls":true}"#),
    ] {
        let receipt = exercise("blocked", Some(CommandFailure::BlockedOutput(output))).await?;
        assert!(receipt.outcome.is_err(), "{case} blocked diagnostics stay unknown");
        assert!(healthy_controls(&receipt), "{case}");
        assert!(observed(&receipt, Phase::Restricted, "tls_blocked"), "{case}");
    }
    Ok(())
}
