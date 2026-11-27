#![cfg(feature = "server")]

use std::error::Error;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::{Client, Response};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_actor_agent::{
    ActorIdentitySource, AgentState, BIND_PATH, EXEC_PATH, EXEC_WRITE_STDIN_PATH, ExecFrame, GUEST_PROTOCOL_VERSION,
    IDENTITY_PATH, ReverseProxy, TERMINAL_OP_PATH, TERMINAL_OPEN_PATH, TerminalFrame, WORKER_PROTOCOL_VERSION_HEADER,
    router, serve, serve_vsock,
};
use trogon_sandbox_contract::{LeaseError, RunCommandResponse, TerminalOutput, TerminalResponse, WriteStdinResponse};
use trogon_sandbox_protocol::terminal_output_bytes;

type TestResult = Result<(), Box<dyn Error>>;

struct Agent {
    base: String,
    client: Client,
    cancel: CancellationToken,
    _dir: tempfile::TempDir,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Agent {
    async fn start() -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let state = AgentState::new(ActorIdentitySource::new(dir.path().join("actor-id")));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let cancel = CancellationToken::new();
        tokio::spawn(serve(listener, router(Arc::new(state)), cancel.clone()));
        let agent = Self {
            base,
            client: Client::new(),
            cancel,
            _dir: dir,
        };
        let bound = agent.post(BIND_PATH, &json!({"lease_id": "01J0"})).await?;
        assert_eq!(bound.status(), 204);
        Ok(agent)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Response, Box<dyn Error>> {
        let request = self.client.post(format!("{}{path}", self.base));
        let request = request.header(WORKER_PROTOCOL_VERSION_HEADER, GUEST_PROTOCOL_VERSION);
        Ok(request.json(body).send().await?)
    }

    async fn terminal(&self, lease_id: &str, id: &str, op: Value) -> Result<Response, Box<dyn Error>> {
        let mut body = json!({"lease_id": lease_id, "terminal_id": id});
        if let (Some(target), Some(op)) = (body.as_object_mut(), op.as_object()) {
            target.extend(op.clone());
        }
        let path = if op.get("op").and_then(Value::as_str) == Some("open") {
            TERMINAL_OPEN_PATH
        } else {
            TERMINAL_OP_PATH
        };
        self.post(path, &body).await
    }
}

fn frames(body: &str) -> Result<Vec<TerminalFrame>, Box<dyn Error>> {
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn open(command: &[&str]) -> Value {
    json!({"op": "open", "cols": 80, "rows": 24, "cmd": command})
}

#[tokio::test]
async fn an_opened_terminal_streams_an_opened_frame_then_output_then_one_exit() -> TestResult {
    let agent = Agent::start().await?;
    let response = agent
        .terminal("01J0", "t1", open(&["/bin/sh", "-c", "printf hello; exit 4"]))
        .await?;
    let lines = frames(&response.text().await?)?;
    assert!(matches!(
        lines.first(),
        Some(TerminalFrame::Response(TerminalResponse::Opened { .. }))
    ));
    assert!(matches!(
        lines.last(),
        Some(TerminalFrame::Output(TerminalOutput::Exit { exit_code: Some(4), .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn a_terminal_for_a_stale_lease_is_refused_in_one_frame() -> TestResult {
    let agent = Agent::start().await?;
    let response = agent.terminal("01J-STALE", "t1", open(&["/bin/cat"])).await?;
    let lines = frames(&response.text().await?)?;
    assert!(
        matches!(
            lines.as_slice(),
            [TerminalFrame::Response(TerminalResponse::Denied(
                LeaseError::LeaseNotFound { .. }
            ))]
        ),
        "got {lines:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_terminal_that_cannot_start_is_refused_in_one_frame() -> TestResult {
    let agent = Agent::start().await?;
    let response = agent.terminal("01J0", "t1", open(&["/nonexistent/program"])).await?;
    let lines = frames(&response.text().await?)?;
    assert!(
        matches!(lines.as_slice(), [TerminalFrame::Response(TerminalResponse::Failed(_))]),
        "got {lines:?}"
    );
    Ok(())
}

#[tokio::test]
async fn terminal_operations_reach_the_open_terminal() -> TestResult {
    let agent = Agent::start().await?;
    let mut stream = agent.terminal("01J0", "t1", open(&["/bin/cat"])).await?;
    let input = json!({"op": "input", "data": STANDARD.encode("ping\n")});
    let done: TerminalResponse = agent.terminal("01J0", "t1", input).await?.json().await?;
    assert_eq!(done, TerminalResponse::Done);

    let mut pending = String::new();
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("ping") {
        let chunk = stream.chunk().await?.ok_or("the stream ended early")?;
        pending.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = pending.find('\n') {
            let line: String = pending.drain(..=end).collect();
            if let Ok(TerminalFrame::Output(output)) = serde_json::from_str(line.trim()) {
                seen.extend(terminal_output_bytes(&output).unwrap_or_default());
            }
        }
    }
    let closed: TerminalResponse = agent
        .terminal("01J0", "t1", json!({"op": "close"}))
        .await?
        .json()
        .await?;
    assert_eq!(closed, TerminalResponse::Done);
    while stream.chunk().await?.is_some() {}
    Ok(())
}

#[tokio::test]
async fn a_terminal_operation_for_a_stale_lease_is_refused() -> TestResult {
    let agent = Agent::start().await?;
    let answer: TerminalResponse = agent
        .terminal("01J-STALE", "t1", json!({"op": "close"}))
        .await?
        .json()
        .await?;
    assert!(matches!(answer, TerminalResponse::Denied(_)), "got {answer:?}");
    Ok(())
}

async fn started_execution(agent: &Agent) -> Result<String, Box<dyn Error>> {
    let body = json!({"lease_id": "01J0", "command": ["/bin/cat"], "detached": true, "stdin": "interactive"});
    let text = agent.post(EXEC_PATH, &body).await?.text().await?;
    let frame: ExecFrame = serde_json::from_str(text.trim())?;
    match frame {
        ExecFrame::Response(RunCommandResponse::Started(started)) => Ok(started.execution_id.as_str().to_string()),
        other => Err(format!("expected a started response, got {other:?}").into()),
    }
}

#[tokio::test]
async fn stdin_written_over_http_reaches_the_command() -> TestResult {
    let agent = Agent::start().await?;
    let execution = started_execution(&agent).await?;
    let write = json!({"lease_id": "01J0", "execution_id": execution, "seq": 0, "data": [104, 105], "eof": true});
    let answer: WriteStdinResponse = agent.post(EXEC_WRITE_STDIN_PATH, &write).await?.json().await?;
    assert!(matches!(answer, WriteStdinResponse::Written { .. }), "got {answer:?}");
    Ok(())
}

#[tokio::test]
async fn stdin_for_a_stale_lease_is_refused() -> TestResult {
    let agent = Agent::start().await?;
    let write = json!({"lease_id": "01J-STALE", "execution_id": "x", "seq": 0, "data": [1], "eof": false});
    let answer: WriteStdinResponse = agent.post(EXEC_WRITE_STDIN_PATH, &write).await?.json().await?;
    assert!(matches!(answer, WriteStdinResponse::Denied(_)), "got {answer:?}");
    Ok(())
}

#[tokio::test]
async fn an_unreadable_identity_is_service_unavailable() -> TestResult {
    let agent = Agent::start().await?;
    let response = agent
        .client
        .get(format!("{}{IDENTITY_PATH}", agent.base))
        .header(WORKER_PROTOCOL_VERSION_HEADER, GUEST_PROTOCOL_VERSION)
        .send()
        .await?;
    assert_eq!(response.status(), 503);
    Ok(())
}

#[test]
fn the_default_reverse_proxy_is_a_new_one() {
    let _ = ReverseProxy::default();
}

#[tokio::test]
async fn serving_on_vsock_either_binds_or_reports_why_not() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state = AgentState::new(ActorIdentitySource::new(dir.path().join("actor-id")));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let outcome = serve_vsock(u32::MAX - 1, router(Arc::new(state)), cancel).await;
    #[cfg(not(target_os = "linux"))]
    assert_eq!(
        outcome.err().map(|error| error.kind()),
        Some(std::io::ErrorKind::Unsupported)
    );
    #[cfg(target_os = "linux")]
    let _ = outcome;
    Ok(())
}
