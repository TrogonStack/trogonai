//! A terminal driven over NATS end to end against an in-process SSH server standing in for Daytona's SSH gateway: open,
//! input, output, resize and close, plus the access-token and session-lifetime rules.

mod gateway_support;
#[allow(dead_code, reason = "shared test support; this binary uses a subset")]
mod nats_support;
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_nats::{Client, Subscriber};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::StreamExt;
use gateway_support::{FakeAccess, GatewayServer, TestResult, eventually, request_until_answered, unanswered};
use nats_support::Nats;
use tokio::task::JoinHandle;
use trogon_sandbox_contract::{
    LeaseId, ProviderKind, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalInput, TerminalOp,
    TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_daytona_worker::{RusshGatewayShell, run_terminal_handler};
use trogon_sandbox_protocol::subjects::{provider_terminal, terminal_output};
use trogon_sandbox_protocol::terminal_output_bytes;
use trogon_sandbox_ssh_gateway::{TerminalHub, TunnelError};

struct Rig {
    _nats: Nats,
    client: Client,
    server: GatewayServer,
    access: Arc<FakeAccess>,
    shell: Arc<RusshGatewayShell>,
    hub: Arc<TerminalHub>,
    handler: JoinHandle<Result<(), TunnelError>>,
}

async fn rig() -> TestResult<Rig> {
    rig_refusing(0).await
}

/// A rig whose gateway refuses the first `stale` tokens it is shown.
async fn rig_refusing(stale: usize) -> TestResult<Rig> {
    let nats = Nats::start().await;
    let client = nats.client().await;
    let server = GatewayServer::start().await?;
    let access = FakeAccess::stale(&server, stale);
    let shell = Arc::new(RusshGatewayShell::new(Arc::clone(&access) as _));
    let hub = Arc::new(TerminalHub::new(Arc::clone(&shell) as _, client.clone()));
    let handler = tokio::spawn(run_terminal_handler(client.clone(), Arc::clone(&hub)));
    Ok(Rig {
        _nats: nats,
        client,
        server,
        access,
        shell,
        hub,
        handler,
    })
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn request(lease: &str, id: &str, op: TerminalOp) -> TestResult<(String, Vec<u8>)> {
    let request = TerminalRequest {
        lease_id: LeaseId::new(lease),
        terminal_id: TerminalId::new(id)?,
        op,
    };
    let subject = provider_terminal(ProviderKind::Daytona, &request.lease_id);
    Ok((subject, serde_json::to_vec(&request)?))
}

/// Opens a terminal, retrying until the handler, which subscribes in a spawned task, answers. Opening is idempotent,
/// so a retry cannot open a second terminal.
async fn open(rig: &Rig, lease: &str, id: &str, op: TerminalOp) -> TestResult<TerminalResponse> {
    let (subject, body) = request(lease, id, op)?;
    let reply = request_until_answered(&rig.client, &subject, body).await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

/// Sends one request to a terminal that is already open, once.
async fn send(rig: &Rig, id: &str, op: TerminalOp) -> TestResult<TerminalResponse> {
    let (subject, body) = request("lease-1", id, op)?;
    let reply = tokio::time::timeout(Duration::from_secs(10), rig.client.request(subject, body.into())).await??;
    Ok(serde_json::from_slice(&reply.payload)?)
}

fn open_op(cols: u16, rows: u16) -> TestResult<TerminalOp> {
    Ok(TerminalOp::Open {
        cols: TerminalDimension::new(cols)?,
        rows: TerminalDimension::new(rows)?,
        cmd: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        resume_from: None,
    })
}

fn input_op(text: &str) -> TestResult<TerminalOp> {
    Ok(TerminalOp::Input {
        data: TerminalInput::new(STANDARD.encode(text))?,
    })
}

async fn next_frame(output: &mut Subscriber) -> TestResult<TerminalOutput> {
    let message = output.next().await.ok_or("the output subscription ended")?;
    Ok(serde_json::from_slice(&message.payload)?)
}

/// Reads a terminal's output frames until they contain `want`, failing after ten seconds or at the terminal's end.
async fn read_until(output: &mut Subscriber, want: &str) -> TestResult<String> {
    let read = async {
        let mut seen = String::new();
        while !seen.contains(want) {
            let frame = next_frame(output).await?;
            let bytes = terminal_output_bytes(&frame).ok_or_else(|| format!("ended before {want:?}; saw {seen:?}"))?;
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(seen)
    };
    tokio::time::timeout(Duration::from_secs(10), read).await?
}

/// The exit code of a terminal's exit frame, skipping its output.
async fn next_exit(output: &mut Subscriber) -> TestResult<Option<i32>> {
    let wait = async {
        loop {
            if let TerminalOutput::Exit { exit_code, .. } = next_frame(output).await? {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(exit_code);
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await?
}

#[tokio::test]
async fn a_terminal_opens_types_resizes_and_closes_through_nats() -> TestResult {
    let rig = rig().await?;
    let id = TerminalId::new("term-1")?;
    let mut output = rig.client.subscribe(terminal_output(&id)).await?;

    let opened = open(&rig, "lease-1", "term-1", open_op(100, 30)?).await?;
    assert_eq!(opened, TerminalResponse::Opened { terminal_id: id });
    read_until(&mut output, "ready").await?;

    assert_eq!(
        send(&rig, "term-1", input_op("echo hello\n")?).await?,
        TerminalResponse::Done
    );
    read_until(&mut output, "hello").await?;

    let resize = TerminalOp::Resize {
        cols: TerminalDimension::new(120)?,
        rows: TerminalDimension::new(40)?,
    };
    assert_eq!(send(&rig, "term-1", resize).await?, TerminalResponse::Done);
    read_until(&mut output, "size 120x40").await?;

    assert_eq!(send(&rig, "term-1", TerminalOp::Close).await?, TerminalResponse::Done);
    assert_eq!(next_exit(&mut output).await?, None);
    eventually(|| rig.server.seen().closed_channels > 0).await?;

    let seen = rig.server.seen();
    assert_eq!(seen.ptys, [("xterm-256color".to_string(), 100, 30)]);
    assert_eq!(seen.sizes, [(120, 40)]);
    assert!(seen.execs.is_empty(), "a plain terminal starts a shell");
    Ok(())
}

#[tokio::test]
async fn terminals_share_one_session_and_one_access_token_per_lease() -> TestResult {
    let rig = rig().await?;
    for n in 0..3 {
        let opened = open(&rig, "lease-1", &format!("t{n}"), open_op(80, 24)?).await?;
        assert!(matches!(opened, TerminalResponse::Opened { .. }), "{opened:?}");
    }
    assert_eq!(rig.access.mint_count(), 1);
    assert_eq!(rig.server.seen().logins, 1);
    assert_eq!(rig.shell.session_count(), 1);

    rig.hub.close_lease(&lease()).await;

    assert_eq!(
        rig.shell.session_count(),
        0,
        "releasing the lease drops its session and token"
    );
    Ok(())
}

#[tokio::test]
async fn a_command_runs_as_an_exec_and_its_exit_code_is_reported() -> TestResult {
    let rig = rig().await?;
    let id = TerminalId::new("term-x")?;
    let mut output = rig.client.subscribe(terminal_output(&id)).await?;
    let op = TerminalOp::Open {
        cols: TerminalDimension::new(80)?,
        rows: TerminalDimension::new(24)?,
        cmd: vec![TerminalArgument::new("top")?],
        env: BTreeMap::new(),
        cwd: Some("/w".to_string()),
        resume_from: None,
    };

    let opened = open(&rig, "lease-1", "term-x", op).await?;
    assert!(matches!(opened, TerminalResponse::Opened { .. }), "{opened:?}");
    send(&rig, "term-x", input_op("exit\n")?).await?;

    assert_eq!(next_exit(&mut output).await?, Some(3));
    assert_eq!(rig.server.seen().execs, ["cd '/w' || exit 1; exec 'top'"]);
    Ok(())
}

#[tokio::test]
async fn a_refused_token_is_replaced_once_when_a_terminal_opens() -> TestResult {
    let rig = rig_refusing(1).await?;

    let opened = open(&rig, "lease-1", "t", open_op(80, 24)?).await?;

    assert!(matches!(opened, TerminalResponse::Opened { .. }), "{opened:?}");
    assert_eq!(rig.access.mint_count(), 2);
    assert_eq!(rig.server.seen().logins, 1);
    Ok(())
}

#[tokio::test]
async fn a_token_the_gateway_keeps_refusing_fails_the_terminal_without_leaking_it() -> TestResult {
    let rig = rig_refusing(100).await?;

    let failed = open(&rig, "lease-1", "t", open_op(80, 24)?).await?;

    let TerminalResponse::Failed(TerminalError::SpawnFailed { reason }) = &failed else {
        return Err(format!("expected a spawn failure, got {failed:?}").into());
    };
    assert!(!reason.contains("token-"), "the token must not be echoed: {reason}");
    assert_eq!(rig.server.seen().logins, 0);
    Ok(())
}

#[tokio::test]
async fn a_lease_the_worker_does_not_hold_gets_no_answer() -> TestResult {
    let rig = rig().await?;
    // The handler is subscribed once a held lease round-trips.
    open(&rig, "lease-1", "warm", open_op(80, 24)?).await?;

    let (subject, body) = request("lease-other", "t", open_op(80, 24)?)?;

    assert!(
        unanswered(&rig.client, &subject, body).await,
        "an unheld lease must not be answered"
    );
    Ok(())
}

#[tokio::test]
async fn the_handler_returns_once_the_client_drains() -> TestResult {
    let rig = rig().await?;
    open(&rig, "lease-1", "warm", open_op(80, 24)?).await?;

    rig.client.drain().await?;

    tokio::time::timeout(Duration::from_secs(10), rig.handler).await???;
    Ok(())
}
