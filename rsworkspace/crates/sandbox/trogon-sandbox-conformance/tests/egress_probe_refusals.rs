//! The denied-connect check counts a lease as denied only when a healthy connection before and after it proves the
//! endpoint and the probe work, and only when the restricted probe reports an explicit, complete denial.

mod support;

use std::collections::{BTreeSet, VecDeque};
use std::error::Error;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::{Client, Message};
use futures::StreamExt;
use support::egress_probe::{self, Proof, Scenario};
use support::locked;
use support::wire::Sendable;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{CheckFailureError, GrantLedger, Target, checks};
use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, LeaseError, NetworkPolicy, OutputTruncation, ProviderDispatchPayload,
    ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, SandboxId,
};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch, provider_run_command};

const KIND: ProviderKind = ProviderKind::Mock;

/// How the hand-written worker answers one command.
#[derive(Clone)]
enum Reply {
    Completed(CommandOutput),
    Denied,
    Malformed,
    Lost,
    PythonSocket(Scenario),
}

fn marker(value: &str) -> Reply {
    Reply::Completed(CommandOutput {
        stdout: format!("{value}\n").into_bytes(),
        ..CommandOutput::default()
    })
}

#[derive(Default)]
struct Recorded {
    dispatches: Vec<ProviderDispatchPayload>,
    commands: Vec<RunCommandRequest>,
    probes: Vec<RunCommandRequest>,
    canceled: BTreeSet<String>,
    python_runs: Vec<(CommandOutput, Proof)>,
}

struct Receipt {
    outcome: Result<(), CheckFailureError>,
    recorded: Arc<Mutex<Recorded>>,
    ledger: GrantLedger,
}

struct Worker {
    client: Client,
    state: Arc<Mutex<Recorded>>,
    preflight: Reply,
    probes: VecDeque<Reply>,
}

impl Worker {
    fn dispatch(&self, message: &Message) -> Sendable<Option<Vec<u8>>> {
        let dispatch: ProviderDispatchPayload = serde_json::from_slice(&message.payload)?;
        let native = ProviderLease {
            sandbox_id: SandboxId::new(format!("mock-egress-{}", dispatch.lease_id.as_str()))?,
            evidence_subject: format!("sandbox.evidence.{}", dispatch.lease_id.as_str()),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        };
        locked(&self.state).dispatches.push(dispatch);
        Ok(Some(serde_json::to_vec(&native)?))
    }

    async fn command(&mut self, message: &Message) -> Sendable<Option<Vec<u8>>> {
        let command: RunCommandRequest = serde_json::from_slice(&message.payload)?;
        let network_attempt = command.command.first().is_some_and(|name| name == "python3")
            || command.command.iter().any(|arg| arg.contains("nc -z"));
        locked(&self.state).commands.push(command.clone());
        let scripted = if network_attempt {
            locked(&self.state).probes.push(command.clone());
            self.probes.pop_front().unwrap_or(Reply::Denied)
        } else {
            self.preflight.clone()
        };
        let response = match scripted {
            Reply::Completed(output) => RunCommandResponse::Completed(output),
            Reply::Denied => RunCommandResponse::Denied(LeaseError::Backend {
                message: "mock execution refusal".to_owned(),
            }),
            Reply::Malformed => return Ok(Some(b"not a command reply".to_vec())),
            Reply::Lost => return Ok(None),
            Reply::PythonSocket(scenario) => {
                let (output, proof) = egress_probe::execute(&command, scenario).await?;
                locked(&self.state).python_runs.push((output.clone(), proof));
                RunCommandResponse::Completed(output)
            }
        };
        Ok(Some(serde_json::to_vec(&response)?))
    }

    async fn answer(&mut self, message: Message) -> Sendable<()> {
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
            } else if known
                .iter()
                .any(|dispatch| subject == provider_run_command(KIND, &dispatch.lease_id))
            {
                self.command(&message).await?
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

async fn exercise(preflight: Reply, probes: Vec<Reply>) -> Result<Receipt, Box<dyn Error>> {
    exercise_at(preflight, probes, "192.0.2.50:34567".parse()?).await
}

async fn exercise_at(preflight: Reply, probes: Vec<Reply>, endpoint: SocketAddr) -> Result<Receipt, Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut messages = client.subscribe("sandbox.provider.mock.>").await?;
    client.flush().await?;
    let recorded = Arc::new(Mutex::new(Recorded::default()));
    let mut serving = Worker {
        client: client.clone(),
        state: recorded.clone(),
        preflight,
        probes: VecDeque::from(probes),
    };
    let worker = tokio::spawn(async move {
        while let Some(message) = messages.next().await {
            serving.answer(message).await?;
        }
        Sendable::Ok(())
    });
    let ledger = GrantLedger::default();
    let mut target = Target::new(client.clone(), KIND).with_grant_ledger(ledger.clone());
    target.egress_probe_address = endpoint;
    target.timeout = Duration::from_millis(500);
    let (_, check) = checks()
        .into_iter()
        .find(|(name, _)| *name == "denied_connect_fails")
        .ok_or("the egress check is in the suite")?;
    let outcome = tokio::time::timeout(Duration::from_secs(10), check(&target)).await?;
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
    assert_eq!(grants.len(), state.dispatches.len(), "every complete grant is kept");
    for dispatch in &state.dispatches {
        assert!(
            state.canceled.contains(dispatch.lease_id.as_str()),
            "every lease is released"
        );
        assert!(grants.iter().any(|grant| grant.lease_id == dispatch.lease_id));
    }
    assert!(state.commands.iter().all(|command| command.timeout_seconds == 20));
    drop(state);
    Ok(Receipt {
        outcome,
        recorded,
        ledger,
    })
}

fn policies(receipt: &Receipt) -> Result<Vec<NetworkPolicy>, Box<dyn Error>> {
    locked(&receipt.recorded)
        .dispatches
        .iter()
        .map(|dispatch| {
            dispatch
                .request
                .spec
                .network_policy()
                .cloned()
                .ok_or_else(|| "every egress lease names its policy".into())
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_or_invalid_probe_preflight_never_creates_a_denied_lease() -> Result<(), Box<dyn Error>> {
    for preflight in [
        marker("NO_PROBE"),
        marker("DENIED"),
        Reply::Denied,
        Reply::Malformed,
        Reply::Completed(CommandOutput {
            exit_code: 1,
            stdout: b"PYTHON3\n".to_vec(),
            ..CommandOutput::default()
        }),
    ] {
        let receipt = exercise(preflight, Vec::new()).await?;
        assert!(receipt.outcome.is_err(), "an unusable probe fails");
        assert_eq!(policies(&receipt)?, vec![NetworkPolicy::AllowAll]);
        assert!(locked(&receipt.recorded).probes.is_empty());
        assert_eq!(receipt.ledger.snapshot().len(), 1);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_first_control_cannot_earn_denial_or_switch_tools() -> Result<(), Box<dyn Error>> {
    let receipt = exercise(marker("NC"), vec![marker("DENIED")]).await?;
    assert!(receipt.outcome.is_err(), "an unreachable endpoint is not policy proof");
    assert_eq!(policies(&receipt)?, vec![NetworkPolicy::AllowAll]);
    let state = locked(&receipt.recorded);
    assert_eq!(state.probes.len(), 1);
    assert!(state.probes[0].command.iter().any(|arg| arg.contains("nc -z")));
    assert!(
        state
            .commands
            .iter()
            .all(|command| command.command.first().is_none_or(|name| name != "python3"))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_endpoint_unreachable_after_the_control_cannot_earn_denial() -> Result<(), Box<dyn Error>> {
    let receipt = exercise(
        marker("NC"),
        vec![marker("CONNECTED"), marker("DENIED"), marker("DENIED")],
    )
    .await?;
    assert!(
        receipt.outcome.is_err(),
        "an endpoint that disappears is not policy proof"
    );
    assert_eq!(
        policies(&receipt)?,
        vec![NetworkPolicy::AllowAll, NetworkPolicy::DenyAll]
    );
    assert_eq!(locked(&receipt.recorded).probes.len(), 3);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restricted_execution_errors_and_malformed_results_never_earn_denial() -> Result<(), Box<dyn Error>> {
    let denied = || b"DENIED\n".to_vec();
    let invalid = vec![
        marker("unexpected"),
        marker("DENIED\nextra"),
        Reply::Denied,
        Reply::Malformed,
        Reply::Lost,
        Reply::Completed(CommandOutput {
            stdout: vec![0xff],
            ..CommandOutput::default()
        }),
        Reply::Completed(CommandOutput {
            exit_code: 127,
            stdout: denied(),
            ..CommandOutput::default()
        }),
        Reply::Completed(CommandOutput {
            stdout: denied(),
            stderr: b"unexpected interpreter error".to_vec(),
            ..CommandOutput::default()
        }),
        Reply::Completed(CommandOutput {
            exit_code: CommandOutput::TIMED_OUT_EXIT_CODE,
            stdout: denied(),
            timed_out: CommandTimeoutStatus::TimedOut,
            ..CommandOutput::default()
        }),
        Reply::Completed(CommandOutput {
            stdout: denied(),
            truncated: OutputTruncation::Truncated,
            ..CommandOutput::default()
        }),
    ];
    for restricted in invalid {
        let receipt = exercise(
            marker("PYTHON3"),
            vec![marker("CONNECTED"), restricted, marker("CONNECTED")],
        )
        .await?;
        assert!(
            receipt.outcome.is_err(),
            "only an explicit, clean negative probe counts"
        );
        assert_eq!(
            policies(&receipt)?,
            vec![NetworkPolicy::AllowAll, NetworkPolicy::DenyAll]
        );
        assert_eq!(
            locked(&receipt.recorded).probes.len(),
            3,
            "the after control still runs when the restricted probe errs"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restricted_lease_that_connects_is_refused_after_both_controls() -> Result<(), Box<dyn Error>> {
    let receipt = exercise(
        marker("PYTHON3"),
        vec![marker("CONNECTED"), marker("CONNECTED"), marker("CONNECTED")],
    )
    .await?;
    assert!(
        receipt.outcome.is_err(),
        "a connected restricted lease violates DenyAll"
    );
    assert_eq!(locked(&receipt.recorded).probes.len(), 3);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_selected_tool_and_raw_endpoint_are_used_for_every_connection() -> Result<(), Box<dyn Error>> {
    for tool in ["NC", "PYTHON3"] {
        let receipt = exercise(
            marker(tool),
            vec![marker("CONNECTED"), marker("DENIED"), marker("CONNECTED")],
        )
        .await?;
        assert!(receipt.outcome.is_ok(), "{tool}: {:?}", receipt.outcome);
        assert_eq!(
            policies(&receipt)?,
            vec![NetworkPolicy::AllowAll, NetworkPolicy::DenyAll]
        );
        let state = locked(&receipt.recorded);
        assert_eq!(
            state.commands.len(),
            4,
            "select once, then run the same probe three times"
        );
        assert_eq!(state.probes.len(), 3);
        assert!(
            state
                .probes
                .iter()
                .all(|command| command.command == state.probes[0].command)
        );
        assert!(state.probes[0].command.iter().any(|arg| arg == "192.0.2.50"));
        assert!(state.probes[0].command.iter().any(|arg| arg == "34567"));
        assert_eq!(state.probes[0].command[0] == "python3", tool == "PYTHON3");
    }
    Ok(())
}

async fn python_socket_case(restricted: Scenario) -> Result<Receipt, Box<dyn Error>> {
    let receipt = exercise_at(
        marker("PYTHON3"),
        vec![
            Reply::PythonSocket(Scenario::Connected),
            Reply::PythonSocket(restricted),
            Reply::PythonSocket(Scenario::Connected),
        ],
        "1.1.1.1:443".parse()?,
    )
    .await?;
    assert_eq!(
        policies(&receipt)?,
        vec![NetworkPolicy::AllowAll, NetworkPolicy::DenyAll]
    );
    {
        let state = locked(&receipt.recorded);
        assert_eq!(state.probes.len(), 3, "the before, restricted and after probes all run");
        assert_eq!(state.python_runs.len(), 3);
        assert!(
            state
                .probes
                .iter()
                .all(|command| command.command == state.probes[0].command)
        );
        for (output, proof) in &state.python_runs {
            assert_eq!(proof.connect_calls, 1);
            assert_eq!(proof.close_calls, 1);
            assert_eq!(proof.timeout_seconds, 3);
            assert!(proof.endpoint_preserved);
            assert!(!output.timed_out.is_timed_out() && !output.truncated.is_truncated());
        }
        for index in [0, 2] {
            let output = &state.python_runs[index].0;
            assert_eq!(output.exit_code, 0);
            assert_eq!(output.stdout, b"CONNECTED\n");
            assert!(output.stderr.is_empty(), "a healthy control completes exactly");
        }
    }
    Ok(receipt)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_socket_timeout_without_an_errno_is_a_denial_between_healthy_controls() -> Result<(), Box<dyn Error>> {
    let receipt = python_socket_case(Scenario::LegacyTimeout).await?;
    {
        let state = locked(&receipt.recorded);
        let proof = &state.python_runs[1].1;
        assert!(proof.socket_timeout_raised && proof.errno_none);
        assert!(
            !proof.builtin_timeout_raised,
            "the socket timeout is distinct from the built-in TimeoutError"
        );
    }
    assert!(receipt.outcome.is_ok(), "{:?}", receipt.outcome);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_builtin_timeout_is_a_denial_between_healthy_controls() -> Result<(), Box<dyn Error>> {
    let receipt = python_socket_case(Scenario::BuiltinTimeout).await?;
    {
        let state = locked(&receipt.recorded);
        let proof = &state.python_runs[1].1;
        assert!(proof.builtin_timeout_raised && proof.errno_none);
        assert!(!proof.socket_timeout_raised);
    }
    assert!(receipt.outcome.is_ok(), "{:?}", receipt.outcome);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_os_and_arbitrary_python_failures_never_earn_denial() -> Result<(), Box<dyn Error>> {
    for scenario in [Scenario::UnknownOsError, Scenario::ArbitraryError] {
        let receipt = python_socket_case(scenario).await?;
        assert!(
            receipt.outcome.is_err(),
            "an unrecognized interpreter failure fails closed"
        );
        let state = locked(&receipt.recorded);
        let (output, proof) = &state.python_runs[1];
        assert_ne!(output.exit_code, 0);
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
        assert!(!proof.socket_timeout_raised && !proof.builtin_timeout_raised);
    }
    Ok(())
}
