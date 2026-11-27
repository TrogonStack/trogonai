mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use support::{TestResult, lock, project_dir};
use trogon_sandbox_client::{Bus, BusError, BusMessage, SandboxClient};
use trogon_sandbox_contract::{
    CommandOutput, CommandOutputChunk, ExecutionId, KillCommandResponse, LeaseId, OutputStream, PollCommandResponse,
    RunCommandResponse, RunCommandStartedResponse, StdinEof, WriteStdinResponse,
};
use trogon_sandbox_lsp::{
    ExitState, LaunchCommand, LspError, LspLauncher, LspPresetName, LspServer, LspServerOptions, OutputKind,
    SandboxLauncher, Signal,
};

type Sent = Arc<Mutex<Vec<(String, Value)>>>;

/// A bus that answers each lease operation from a fixed reply and serves scripted evidence frames.
struct ScriptBus {
    sent: Sent,
    poll: PollCommandResponse,
    frames: Vec<BusMessage>,
    fail_run: bool,
    fail_subscribe: bool,
}

impl ScriptBus {
    fn new(frames: Vec<BusMessage>) -> (Self, Sent) {
        let sent = Sent::default();
        let bus = Self {
            sent: sent.clone(),
            poll: PollCommandResponse::Running,
            frames,
            fail_run: false,
            fail_subscribe: false,
        };
        (bus, sent)
    }
}

fn json_bytes(value: &impl serde::Serialize) -> Result<Option<Bytes>, BusError> {
    serde_json::to_vec(value)
        .map(|bytes| Some(Bytes::from(bytes)))
        .map_err(|e| BusError::Custom(Box::new(e)))
}

#[async_trait]
impl Bus for ScriptBus {
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        _timeout: Duration,
        _headers: Option<async_nats::HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        let body: Value = serde_json::from_slice(&payload).map_err(|e| BusError::Custom(Box::new(e)))?;
        lock(&self.sent).push((subject.to_string(), body.clone()));
        if subject.starts_with("sandbox.lease.run_command.") {
            if self.fail_run {
                return Err(BusError::Custom("bus down".into()));
            }
            return json_bytes(&RunCommandResponse::Started(RunCommandStartedResponse {
                execution_id: ExecutionId::new("exec-1"),
                origin: Default::default(),
            }));
        }
        if subject.starts_with("sandbox.lease.poll_command.") {
            return json_bytes(&self.poll);
        }
        if subject.starts_with("sandbox.lease.kill_command.") {
            return json_bytes(&KillCommandResponse::Signaled);
        }
        if subject.starts_with("sandbox.lease.write_stdin.") {
            let seq = body["seq"].as_u64().unwrap_or(0);
            return json_bytes(&WriteStdinResponse::Written {
                next_seq: trogon_sandbox_contract::StdinSeq::new(seq + 1),
            });
        }
        Ok(None)
    }

    async fn subscribe(&self, _subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        if self.fail_subscribe {
            return Err(BusError::Custom("subscribe refused".into()));
        }
        Ok(futures::stream::iter(self.frames.clone()).boxed())
    }
}

fn chunk_frame(execution: &str, stream: OutputStream, seq: u64, data: &[u8]) -> Result<BusMessage, serde_json::Error> {
    let chunk = CommandOutputChunk {
        execution_id: ExecutionId::new(execution),
        stream,
        seq,
        data: data.to_vec(),
    };
    Ok(BusMessage {
        subject: format!("sandbox.evidence.lease-1.command.{execution}"),
        payload: Bytes::from(serde_json::to_vec(&chunk)?),
    })
}

fn launcher(bus: ScriptBus) -> SandboxLauncher {
    SandboxLauncher::new(
        Arc::new(SandboxClient::from_bus(Box::new(bus))),
        LeaseId::new("lease-1"),
    )
}

fn command() -> Result<LaunchCommand, Box<dyn std::error::Error>> {
    Ok(LaunchCommand {
        command: "pyright-langserver".into(),
        args: vec!["--stdio".into()],
        cwd: project_dir()?,
        env: Default::default(),
    })
}

#[tokio::test]
async fn launch_runs_the_command_detached_with_interactive_stdin() -> TestResult {
    let (bus, sent) = ScriptBus::new(vec![]);
    launcher(bus).launch(&command()?).await?;
    let sent = lock(&sent).clone();
    let (subject, body) = &sent[0];
    assert_eq!(subject, "sandbox.lease.run_command.lease-1");
    assert_eq!(body["command"], json!(["pyright-langserver", "--stdio"]));
    assert_eq!(body["cwd"], "/workspace/project");
    assert_eq!(body["detached"], true);
    assert_eq!(body["stdin"], "interactive");
    assert_eq!(body["output_retention"], "stream_only");
    assert_eq!(body["deadline"], "lease");
    Ok(())
}

#[tokio::test]
async fn output_keeps_only_this_commands_frames_in_order() -> TestResult {
    let frames = vec![
        chunk_frame("exec-1", OutputStream::Stdout, 1, b"out")?,
        chunk_frame("exec-other", OutputStream::Stdout, 1, b"foreign")?,
        BusMessage {
            subject: "sandbox.evidence.lease-1.lifecycle".into(),
            payload: Bytes::from("noise"),
        },
        chunk_frame("exec-1", OutputStream::Stderr, 1, b"err")?,
    ];
    let (bus, _) = ScriptBus::new(frames);
    let transport = launcher(bus).launch(&command()?).await?;
    let chunks: Vec<_> = transport.output()?.collect().await;
    let chunks = chunks.into_iter().collect::<Result<Vec<_>, _>>()?;
    let seen: Vec<_> = chunks.iter().map(|c| (c.kind, c.seq, c.data.clone())).collect();
    assert_eq!(
        seen,
        vec![
            (OutputKind::Stdout, 1, b"out".to_vec()),
            (OutputKind::Stderr, 1, b"err".to_vec())
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_malformed_output_frame_is_a_protocol_error() -> TestResult {
    let frames = vec![BusMessage {
        subject: "sandbox.evidence.lease-1.command.exec-1".into(),
        payload: Bytes::from("not json"),
    }];
    let (bus, _) = ScriptBus::new(frames);
    let transport = launcher(bus).launch(&command()?).await?;
    let first = transport.output()?.next().await.ok_or("no item")?;
    assert!(matches!(first, Err(LspError::Protocol(m)) if m.contains("invalid command output frame")));
    Ok(())
}

#[tokio::test]
async fn output_can_only_be_taken_once() -> TestResult {
    let (bus, _) = ScriptBus::new(vec![]);
    let transport = launcher(bus).launch(&command()?).await?;
    let _first = transport.output()?;
    let second = transport.output().err().ok_or("second take succeeded")?;
    assert!(matches!(second, LspError::Transport(m) if m.contains("already taken")));
    Ok(())
}

#[tokio::test]
async fn writes_and_close_go_through_write_stdin() -> TestResult {
    let (bus, sent) = ScriptBus::new(vec![]);
    let transport = launcher(bus).launch(&command()?).await?;
    transport.write(b"hello", StdinEof::Keep).await?;
    transport.write(&[], StdinEof::Close).await?;
    let writes: Vec<_> = lock(&sent)
        .iter()
        .filter(|(subject, _)| subject == "sandbox.lease.write_stdin.lease-1")
        .map(|(_, body)| (body["seq"].clone(), body["eof"].clone()))
        .collect();
    assert_eq!(writes, vec![(json!(0), json!(false)), (json!(1), json!(true))]);
    Ok(())
}

#[tokio::test]
async fn a_write_that_closes_with_data_is_sent_as_one_chunk() -> TestResult {
    let (bus, sent) = ScriptBus::new(vec![]);
    let transport = launcher(bus).launch(&command()?).await?;
    transport.write(b"bye", StdinEof::Close).await?;
    let writes = lock(&sent)
        .iter()
        .filter(|(subject, _)| subject == "sandbox.lease.write_stdin.lease-1")
        .count();
    assert_eq!(writes, 1);
    Ok(())
}

#[tokio::test]
async fn poll_exit_maps_every_outcome() -> TestResult {
    let completed = PollCommandResponse::Completed(CommandOutput {
        exit_code: 7,
        ..Default::default()
    });
    let cases = [
        (PollCommandResponse::Running, ExitState::Running),
        (completed, ExitState::Exited(Some(7))),
        (PollCommandResponse::NotFound, ExitState::Exited(None)),
    ];
    for (poll, expected) in cases {
        let (mut bus, _) = ScriptBus::new(vec![]);
        bus.poll = poll;
        let transport = launcher(bus).launch(&command()?).await?;
        assert_eq!(transport.poll_exit().await?, expected);
    }
    Ok(())
}

#[tokio::test]
async fn kill_sends_the_matching_signal() -> TestResult {
    let (bus, sent) = ScriptBus::new(vec![]);
    let transport = launcher(bus).launch(&command()?).await?;
    transport.kill(Signal::Term).await?;
    transport.kill(Signal::Kill).await?;
    let signals: Vec<_> = lock(&sent)
        .iter()
        .filter(|(subject, _)| subject == "sandbox.lease.kill_command.lease-1")
        .map(|(_, body)| body["signal"].clone())
        .collect();
    assert_eq!(signals, vec![json!("term"), json!("kill")]);
    Ok(())
}

#[tokio::test]
async fn launch_failures_surface_as_errors() -> TestResult {
    let (mut bus, _) = ScriptBus::new(vec![]);
    bus.fail_subscribe = true;
    assert!(launcher(bus).launch(&command()?).await.is_err());

    let (mut bus, _) = ScriptBus::new(vec![]);
    bus.fail_run = true;
    assert!(launcher(bus).launch(&command()?).await.is_err());
    Ok(())
}

#[tokio::test]
async fn in_sandbox_builds_a_server_that_launches_through_the_lease() -> TestResult {
    let (bus, sent) = ScriptBus::new(vec![]);
    let client = Arc::new(SandboxClient::from_bus(Box::new(bus)));
    let server = LspServer::in_sandbox(
        client,
        LeaseId::new("lease-1"),
        LspPresetName::Python,
        project_dir()?,
        LspServerOptions::default(),
    );
    // No initialize reply ever arrives, so the start fails, but only after launching.
    tokio::time::timeout(Duration::from_millis(200), server.start())
        .await
        .ok();
    let launched = lock(&sent)
        .iter()
        .any(|(subject, _)| subject == "sandbox.lease.run_command.lease-1");
    assert!(launched);
    Ok(())
}
