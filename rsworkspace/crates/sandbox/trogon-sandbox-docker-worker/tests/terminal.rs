//! Interactive terminal (PTY exec) integration tests.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker --test terminal
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request};

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures::StreamExt as _;
use trogon_sandbox_contract::{
    BootMode, LeaseId, LeaseRequest, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalInput,
    TerminalOp, TerminalOutput, TerminalRequest, TerminalResponse,
};
use trogon_sandbox_docker_worker::adapter::DockerWorkerProvider;
use trogon_sandbox_docker_worker::terminal::DockerTerminals;
use trogon_sandbox_protocol::Provider;

async fn lease(provider: &DockerWorkerProvider) -> TestResult<LeaseId> {
    let lease_id = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &lease_id).await?;
    Ok(lease_id)
}

fn request(lease_id: &LeaseId, id: &TerminalId, op: TerminalOp) -> TestResult<Vec<u8>> {
    Ok(serde_json::to_vec(&TerminalRequest {
        lease_id: lease_id.clone(),
        terminal_id: id.clone(),
        op,
    })?)
}

fn input(text: &str) -> TestResult<TerminalOp> {
    Ok(TerminalOp::Input {
        data: TerminalInput::new(base64::engine::general_purpose::STANDARD.encode(text))?,
    })
}

fn dim(value: u16) -> TestResult<TerminalDimension> {
    Ok(TerminalDimension::new(value)?)
}

fn data_text(frame: &TerminalOutput) -> Option<String> {
    match frame {
        TerminalOutput::Data { data, .. } => base64::engine::general_purpose::STANDARD
            .decode(data.as_str())
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
        _ => None,
    }
}

/// Reads output frames until `needle` shows up in the accumulated text.
async fn read_until(
    frames: &mut (impl futures::Stream<Item = async_nats::Message> + Unpin),
    needle: &str,
) -> TestResult<String> {
    let wait = async {
        let mut text = String::new();
        while let Some(msg) = frames.next().await {
            if let Ok(frame) = serde_json::from_slice::<TerminalOutput>(&msg.payload) {
                if let Some(chunk) = data_text(&frame) {
                    text.push_str(&chunk);
                }
                if text.contains(needle) {
                    return Ok(text);
                }
            }
        }
        Err(format!("output ended before {needle:?}; saw {text:?}"))
    };
    Ok(tokio::time::timeout(Duration::from_secs(20), wait).await??)
}

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request("terminal-test", BootMode::Bare)
}

#[tokio::test(flavor = "multi_thread")]
async fn pty_terminal_runs_a_shell_resizes_and_closes() -> TestResult {
    let provider = build_provider("terminal").await?;
    let nats = async_nats::connect(provider.server.address()).await?;
    let lease_id = lease(&provider).await?;
    let terminals = Arc::new(DockerTerminals::default());
    let tid = TerminalId::new(format!("t{}", docker_support::fresh_id()))?;
    let mut frames = nats
        .subscribe(trogon_sandbox_protocol::subjects::terminal_output(&tid))
        .await?;
    nats.flush().await?;

    let open = TerminalOp::Open {
        cols: dim(100)?,
        rows: dim(30)?,
        cmd: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        resume_from: None,
    };
    let opened = terminals
        .handle(&provider, &nats, &request(&lease_id, &tid, open)?)
        .await;
    assert!(matches!(opened, TerminalResponse::Opened { .. }), "{opened:?}");

    // A real tty: the shell sees the requested size and echoes input.
    let done = terminals
        .handle(
            &provider,
            &nats,
            &request(&lease_id, &tid, input("stty size; echo ready-$((20+22))\n")?)?,
        )
        .await;
    assert_eq!(done, TerminalResponse::Done);
    let seen = read_until(&mut frames, "ready-42").await?;
    assert!(seen.contains("30 100"), "stty size missing in {seen:?}");

    let resize = TerminalOp::Resize {
        cols: dim(120)?,
        rows: dim(40)?,
    };
    let done = terminals
        .handle(&provider, &nats, &request(&lease_id, &tid, resize)?)
        .await;
    assert_eq!(done, TerminalResponse::Done);
    terminals
        .handle(
            &provider,
            &nats,
            &request(&lease_id, &tid, input("stty size; echo again-$((1+1))\n")?)?,
        )
        .await;
    let seen = read_until(&mut frames, "again-2").await?;
    assert!(seen.contains("40 120"), "resized stty size missing in {seen:?}");

    // Another lease cannot touch this terminal.
    let other = LeaseId::new("someone-else");
    let denied = terminals
        .handle(&provider, &nats, &request(&other, &tid, input("x")?)?)
        .await;
    assert_eq!(denied, TerminalResponse::Failed(TerminalError::IdInUse));

    let closed = terminals
        .handle(&provider, &nats, &request(&lease_id, &tid, TerminalOp::Close)?)
        .await;
    assert_eq!(closed, TerminalResponse::Done);
    let exit = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(msg) = frames.next().await {
            if let Ok(TerminalOutput::Exit { .. }) = serde_json::from_slice(&msg.payload) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(exit, "no exit frame after close");
    let gone = terminals
        .handle(&provider, &nats, &request(&lease_id, &tid, input("x")?)?)
        .await;
    assert_eq!(gone, TerminalResponse::Failed(TerminalError::NotFound));

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_that_exits_ends_the_terminal_with_its_code() -> TestResult {
    let provider = build_provider("terminal").await?;
    let nats = async_nats::connect(provider.server.address()).await?;
    let lease_id = lease(&provider).await?;
    let terminals = Arc::new(DockerTerminals::default());
    let tid = TerminalId::new(format!("t{}", docker_support::fresh_id()))?;
    let mut frames = nats
        .subscribe(trogon_sandbox_protocol::subjects::terminal_output(&tid))
        .await?;
    nats.flush().await?;

    let open = TerminalOp::Open {
        cols: dim(80)?,
        rows: dim(24)?,
        cmd: vec![
            TerminalArgument::new("sh")?,
            TerminalArgument::new("-c")?,
            TerminalArgument::new("echo bye; exit 3")?,
        ],
        env: BTreeMap::new(),
        cwd: None,
        resume_from: None,
    };
    let opened = terminals
        .handle(&provider, &nats, &request(&lease_id, &tid, open)?)
        .await;
    assert!(matches!(opened, TerminalResponse::Opened { .. }), "{opened:?}");
    let code = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(msg) = frames.next().await {
            if let Ok(TerminalOutput::Exit { exit_code, .. }) = serde_json::from_slice(&msg.payload) {
                return exit_code;
            }
        }
        None
    })
    .await?;
    assert_eq!(code, Some(3));

    provider.cancel(&lease_id).await?;
    Ok(())
}
