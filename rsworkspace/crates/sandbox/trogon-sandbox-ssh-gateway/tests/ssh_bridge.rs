mod support;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use support::{FakeHost, Server, TestResult, eventually, json, next_frame, start_nats};
use trogon_sandbox_contract::{LeaseId, TerminalArgument, TerminalDimension, TerminalId, TerminalOp, TerminalOutput};
use trogon_sandbox_protocol::subjects::terminal_output;
use trogon_sandbox_protocol::terminal_data_frame;
use trogon_sandbox_ssh_gateway::{
    BridgeLimits, NatsSshTunnelBackend, SshAttachRequest, SshChannelKind, SshDetachRequest, SshSessionId,
    SshStreamBridge, SshStreamControl, SshStreamFrame, SshTunnelBackend,
};

const TO_WORKER: &str = "sandbox.ssh.stream.s1.to_worker";
const FROM_WORKER: &str = "sandbox.ssh.stream.s1.from_worker";

fn session() -> TestResult<SshSessionId> {
    Ok("s1".parse()?)
}

fn attach(session: &str, lease: &str, ttl: u32) -> TestResult<SshAttachRequest> {
    Ok(SshAttachRequest {
        session_id: session.parse()?,
        lease_id: LeaseId::new(lease),
        public_key: "ssh-ed25519 AAAA".to_string(),
        ttl_seconds: std::num::NonZeroU32::new(ttl).ok_or("zero ttl")?,
    })
}

fn detach(session: &str, lease: &str) -> TestResult<SshDetachRequest> {
    Ok(SshDetachRequest {
        session_id: session.parse()?,
        lease_id: LeaseId::new(lease),
        reason: "test".to_string(),
    })
}

fn limits() -> BridgeLimits {
    BridgeLimits::default()
}

fn dim(value: u16) -> TestResult<TerminalDimension> {
    Ok(TerminalDimension::new(value)?)
}

fn open_frame(channel: u32, kind: SshChannelKind, term: Option<&str>) -> TestResult<SshStreamFrame> {
    Ok(SshStreamFrame::control(
        &session()?,
        channel,
        SshStreamControl::Open {
            kind,
            cols: dim(80)?,
            rows: dim(24)?,
            term: term.map(str::to_string),
        },
    ))
}

struct Harness {
    _server: Server,
    client: async_nats::Client,
    host: Arc<FakeHost>,
    bridge: Arc<SshStreamBridge>,
    from_worker: async_nats::Subscriber,
}

async fn harness(limits: BridgeLimits) -> TestResult<Harness> {
    let server = Server::start().await;
    harness_on(server, limits).await
}

async fn harness_on(server: Server, limits: BridgeLimits) -> TestResult<Harness> {
    let client = server.client().await?;
    let host = FakeHost::holding(&["lease-1", "lease-2"]);
    let bridge = SshStreamBridge::new(host.clone(), client.clone(), limits);
    let from_worker = client.subscribe(FROM_WORKER).await?;
    client.flush().await?;
    Ok(Harness {
        _server: server,
        client,
        host,
        bridge,
        from_worker,
    })
}

impl Harness {
    async fn attached(limits: BridgeLimits) -> TestResult<Self> {
        let harness = harness(limits).await?;
        let response = harness
            .bridge
            .attach(&attach("s1", "lease-1", 600)?)
            .await
            .ok_or("not held")?;
        assert!(response.ok);
        Ok(harness)
    }

    async fn send(&self, frame: &SshStreamFrame) -> TestResult {
        self.client.publish(TO_WORKER, json(frame)?).await?;
        self.client.flush().await?;
        Ok(())
    }

    /// Opens a shell on channel 1 and waits until the host saw the open.
    async fn open_shell(&self) -> TestResult<TerminalId> {
        self.send(&open_frame(1, SshChannelKind::Shell, Some("xterm"))?).await?;
        eventually(|| !self.host.ops().is_empty()).await?;
        self.host.first_terminal().ok_or_else(|| "no terminal".into())
    }

    async fn output(&self, terminal: &TerminalId, frame: &TerminalOutput) -> TestResult {
        self.client.publish(terminal_output(terminal), json(frame)?).await?;
        self.client.flush().await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_lease_the_worker_does_not_hold_is_left_to_others() -> TestResult {
    let harness = harness(limits()).await?;
    assert!(harness.bridge.attach(&attach("s1", "other", 60)?).await.is_none());
    assert!(harness.bridge.detach(&detach("s1", "other")?).await.is_none());
    assert_eq!(harness.bridge.session_count(), 0);
    Ok(())
}

#[tokio::test]
async fn attach_is_idempotent_for_the_same_lease_and_refused_for_another() -> TestResult {
    let harness = Harness::attached(limits()).await?;
    assert_eq!(harness.bridge.session_count(), 1);
    let again = harness
        .bridge
        .attach(&attach("s1", "lease-1", 600)?)
        .await
        .ok_or("not held")?;
    assert!(again.ok);
    let stolen = harness
        .bridge
        .attach(&attach("s1", "lease-2", 600)?)
        .await
        .ok_or("not held")?;
    assert!(!stolen.ok);
    assert_eq!(stolen.message.as_deref(), Some("ssh session belongs to another lease"));
    assert_eq!(harness.bridge.session_count(), 1);
    Ok(())
}

#[tokio::test]
async fn sessions_are_capped() -> TestResult {
    let harness = harness(BridgeLimits {
        max_sessions: NonZeroUsize::MIN,
        ..limits()
    })
    .await?;
    assert!(
        harness
            .bridge
            .attach(&attach("s1", "lease-1", 600)?)
            .await
            .ok_or("not held")?
            .ok
    );
    let second = harness
        .bridge
        .attach(&attach("s2", "lease-1", 600)?)
        .await
        .ok_or("not held")?;
    assert!(!second.ok);
    assert_eq!(
        second.message.as_deref(),
        Some("worker is at its limit of 1 ssh sessions")
    );
    Ok(())
}

#[tokio::test]
async fn a_closed_connection_rejects_the_attach() -> TestResult {
    let harness = harness(limits()).await?;
    harness.client.drain().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let response = harness
        .bridge
        .attach(&attach("s1", "lease-1", 600)?)
        .await
        .ok_or("not held")?;
    assert!(!response.ok);
    assert!(
        response
            .message
            .is_some_and(|message| message.starts_with("stream subscribe failed"))
    );
    assert_eq!(harness.bridge.session_count(), 0);
    Ok(())
}

#[tokio::test]
async fn detach_ends_the_session_and_closes_its_terminals() -> TestResult {
    let mut harness = Harness::attached(limits()).await?;
    harness.open_shell().await?;
    assert!(
        !harness
            .bridge
            .detach(&detach("s1", "lease-2")?)
            .await
            .ok_or("not held")?
            .ok
    );
    assert!(
        harness
            .bridge
            .detach(&detach("s1", "lease-1")?)
            .await
            .ok_or("not held")?
            .ok
    );
    eventually(|| harness.bridge.session_count() == 0).await?;
    assert!(matches!(harness.host.ops().last(), Some(TerminalOp::Close)));
    // The client asked for the end, so it is not told the channel failed.
    assert!(
        tokio::time::timeout(Duration::from_millis(200), next_frame(&mut harness.from_worker))
            .await
            .is_err()
    );
    assert!(
        harness
            .bridge
            .detach(&detach("s1", "lease-1")?)
            .await
            .ok_or("not held")?
            .ok
    );
    Ok(())
}

#[tokio::test]
async fn a_shell_opens_a_terminal_and_relays_its_output() -> TestResult {
    let mut harness = Harness::attached(limits()).await?;
    let terminal = harness.open_shell().await?;
    assert_eq!(terminal.as_str(), "ssh-s1-1");
    let expected = TerminalOp::Open {
        cols: dim(80)?,
        rows: dim(24)?,
        cmd: vec![],
        env: BTreeMap::from([("TERM".to_string(), "xterm".to_string())]),
        cwd: None,
        resume_from: None,
    };
    assert_eq!(harness.host.ops()[0], expected);

    // The same channel opened twice is one terminal.
    harness.send(&open_frame(1, SshChannelKind::Shell, None)?).await?;

    let data = vec![b'o'; 20_000];
    harness.output(&terminal, &terminal_data_frame(0, &data)?).await?;
    let mut sizes = Vec::new();
    while sizes.iter().sum::<usize>() < 20_000 {
        let frame = next_frame(&mut harness.from_worker).await?;
        assert_eq!(frame.channel_id, 1);
        sizes.push(frame.data.len());
    }
    assert_eq!(sizes, [8192, 8192, 3616]);

    harness
        .client
        .publish(terminal_output(&terminal), Bytes::from_static(b"garbage"))
        .await?;
    harness
        .output(&terminal, &TerminalOutput::Resync { from_seq: 0 })
        .await?;
    harness
        .output(
            &terminal,
            &TerminalOutput::Exit {
                seq: 1,
                exit_code: Some(7),
            },
        )
        .await?;
    let exit = next_frame(&mut harness.from_worker).await?;
    assert_eq!(exit.ctl, Some(SshStreamControl::Exit { status: Some(7) }));
    assert!(next_frame(&mut harness.from_worker).await?.eof);
    assert_eq!(
        harness
            .host
            .ops()
            .iter()
            .filter(|op| matches!(op, TerminalOp::Open { .. }))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn an_exit_without_a_usable_code_has_no_status() -> TestResult {
    let mut harness = Harness::attached(limits()).await?;
    let terminal = harness.open_shell().await?;
    harness
        .output(
            &terminal,
            &TerminalOutput::Exit {
                seq: 0,
                exit_code: Some(-1),
            },
        )
        .await?;
    let exit = next_frame(&mut harness.from_worker).await?;
    assert_eq!(exit.ctl, Some(SshStreamControl::Exit { status: None }));
    Ok(())
}

#[tokio::test]
async fn exec_runs_the_command_through_a_shell() -> TestResult {
    let harness = Harness::attached(limits()).await?;
    let kind = SshChannelKind::Exec {
        command: "ls -l".to_string(),
    };
    harness.send(&open_frame(2, kind, Some("bad term!"))?).await?;
    eventually(|| !harness.host.ops().is_empty()).await?;
    let words = ["/bin/sh", "-c", "ls -l"];
    let expected = TerminalOp::Open {
        cols: dim(80)?,
        rows: dim(24)?,
        cmd: words.map(TerminalArgument::new).into_iter().collect::<Result<_, _>>()?,
        env: BTreeMap::new(),
        cwd: None,
        resume_from: None,
    };
    assert_eq!(harness.host.ops()[0], expected);
    Ok(())
}

#[tokio::test]
async fn a_command_with_nul_fails_the_channel() -> TestResult {
    let mut harness = Harness::attached(limits()).await?;
    let kind = SshChannelKind::Exec {
        command: "a\0b".to_string(),
    };
    harness.send(&open_frame(1, kind, None)?).await?;
    let error = next_frame(&mut harness.from_worker).await?;
    assert!(matches!(error.ctl, Some(SshStreamControl::Error { message }) if message.starts_with("invalid command")));
    assert!(next_frame(&mut harness.from_worker).await?.eof);
    assert!(harness.host.ops().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_host_that_does_not_open_fails_the_channel() -> TestResult {
    let mut harness = Harness::attached(limits()).await?;
    harness.host.refuse_open.store(true, Ordering::SeqCst);
    harness.send(&open_frame(1, SshChannelKind::Shell, None)?).await?;
    let error = next_frame(&mut harness.from_worker).await?;
    assert!(
        matches!(error.ctl, Some(SshStreamControl::Error { message }) if message.starts_with("terminal not opened"))
    );
    Ok(())
}

#[tokio::test]
async fn channels_are_capped() -> TestResult {
    let mut harness = Harness::attached(BridgeLimits {
        max_channels: NonZeroUsize::MIN,
        ..limits()
    })
    .await?;
    harness.open_shell().await?;
    harness.send(&open_frame(2, SshChannelKind::Shell, None)?).await?;
    let error = next_frame(&mut harness.from_worker).await?;
    assert_eq!(error.channel_id, 2);
    assert!(
        matches!(error.ctl, Some(SshStreamControl::Error { message }) if message == "session is at its limit of 1 channels")
    );
    Ok(())
}

#[tokio::test]
async fn input_and_resizes_reach_the_terminal() -> TestResult {
    let harness = Harness::attached(limits()).await?;
    harness.open_shell().await?;
    harness
        .send(&SshStreamFrame::data(&session()?, 1, b"hello".to_vec()))
        .await?;
    harness
        .send(&SshStreamFrame::control(
            &session()?,
            1,
            SshStreamControl::Resize {
                cols: dim(100)?,
                rows: dim(30)?,
            },
        ))
        .await?;
    harness
        .send(&SshStreamFrame::data(&session()?, 1, vec![b'z'; 70_000]))
        .await?;
    harness.send(&SshStreamFrame::eof(&session()?, 1)).await?;
    eventually(|| harness.host.ops().len() >= 6).await?;
    let ops = harness.host.ops();
    let inputs: Vec<Vec<u8>> = ops
        .iter()
        .filter_map(|op| match op {
            TerminalOp::Input { data } => STANDARD.decode(data.as_str()).ok(),
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 4);
    assert_eq!(inputs[0], b"hello");
    assert_eq!(inputs[1].len(), 65_536);
    assert_eq!(inputs[2].len(), 70_000 - 65_536);
    assert_eq!(inputs[3], [4u8]);
    assert!(
        ops.iter()
            .any(|op| matches!(op, TerminalOp::Resize { cols, rows } if cols.get() == 100 && rows.get() == 30))
    );
    Ok(())
}

#[tokio::test]
async fn frames_for_unknown_channels_and_echoes_are_ignored() -> TestResult {
    let harness = Harness::attached(limits()).await?;
    harness
        .send(&SshStreamFrame::data(&session()?, 9, b"x".to_vec()))
        .await?;
    harness
        .send(&SshStreamFrame::control(
            &session()?,
            9,
            SshStreamControl::Resize {
                cols: dim(1)?,
                rows: dim(1)?,
            },
        ))
        .await?;
    harness
        .send(&SshStreamFrame::control(
            &session()?,
            9,
            SshStreamControl::Exit { status: Some(0) },
        ))
        .await?;
    harness
        .send(&SshStreamFrame::control(
            &session()?,
            9,
            SshStreamControl::Error {
                message: "echo".to_string(),
            },
        ))
        .await?;
    harness
        .client
        .publish(TO_WORKER, Bytes::from_static(b"garbage"))
        .await?;
    let foreign = SshStreamFrame::data(&"other".parse()?, 1, b"x".to_vec());
    harness.send(&foreign).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(harness.host.ops().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_terminal_that_refuses_input_is_tolerated() -> TestResult {
    let harness = Harness::attached(limits()).await?;
    harness.open_shell().await?;
    harness.host.refuse_ops.store(true, Ordering::SeqCst);
    harness
        .send(&SshStreamFrame::data(&session()?, 1, b"x".to_vec()))
        .await?;
    eventually(|| harness.host.ops().len() >= 2).await?;
    Ok(())
}

#[tokio::test]
async fn an_idle_session_is_closed_and_told_why() -> TestResult {
    let mut harness = Harness::attached(BridgeLimits {
        idle_timeout: Duration::from_millis(400),
        ..limits()
    })
    .await?;
    harness.open_shell().await?;
    let error = next_frame(&mut harness.from_worker).await?;
    assert!(matches!(error.ctl, Some(SshStreamControl::Error { message }) if message == "ssh session idle timeout"));
    eventually(|| harness.bridge.session_count() == 0).await?;
    Ok(())
}

#[tokio::test]
async fn activity_defers_the_idle_timeout() -> TestResult {
    let harness = Harness::attached(BridgeLimits {
        idle_timeout: Duration::from_millis(600),
        ..limits()
    })
    .await?;
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        harness.send(&SshStreamFrame::data(&session()?, 9, Vec::new())).await?;
    }
    assert_eq!(harness.bridge.session_count(), 1);
    Ok(())
}

#[tokio::test]
async fn a_session_expires_after_its_ttl() -> TestResult {
    let mut harness = harness(limits()).await?;
    assert!(
        harness
            .bridge
            .attach(&attach("s1", "lease-1", 1)?)
            .await
            .ok_or("not held")?
            .ok
    );
    harness.open_shell().await?;
    let error = next_frame(&mut harness.from_worker).await?;
    assert!(matches!(error.ctl, Some(SshStreamControl::Error { message }) if message == "ssh session expired"));
    eventually(|| harness.bridge.session_count() == 0).await?;
    Ok(())
}

#[tokio::test]
async fn unwritten_input_over_the_budget_ends_the_session() -> TestResult {
    let mut harness = Harness::attached(BridgeLimits {
        max_buffered_bytes: 16,
        ..limits()
    })
    .await?;
    harness.open_shell().await?;
    harness
        .send(&SshStreamFrame::data(&session()?, 1, vec![b'x'; 4096]))
        .await?;
    let error = next_frame(&mut harness.from_worker).await?;
    assert!(
        matches!(error.ctl, Some(SshStreamControl::Error { message }) if message == "ssh session input buffer exceeded")
    );
    eventually(|| harness.bridge.session_count() == 0).await?;
    Ok(())
}

#[tokio::test]
async fn a_dropped_subscription_ends_the_session() -> TestResult {
    let harness = Harness::attached(limits()).await?;
    harness.client.drain().await?;
    eventually(|| harness.bridge.session_count() == 0).await?;
    Ok(())
}

#[tokio::test]
async fn frames_that_cannot_be_published_are_dropped() -> TestResult {
    let server = Server::start_with_config("max_payload: 1024\n").await;
    let mut harness = harness_on(server, limits()).await?;
    assert!(
        harness
            .bridge
            .attach(&attach("s1", "lease-1", 600)?)
            .await
            .ok_or("not held")?
            .ok
    );
    let terminal = harness.open_shell().await?;
    harness
        .output(&terminal, &terminal_data_frame(0, &[b'x'; 600])?)
        .await?;
    harness
        .output(
            &terminal,
            &TerminalOutput::Exit {
                seq: 1,
                exit_code: Some(0),
            },
        )
        .await?;
    let exit = next_frame(&mut harness.from_worker).await?;
    assert_eq!(exit.ctl, Some(SshStreamControl::Exit { status: Some(0) }));
    Ok(())
}

#[tokio::test]
async fn serve_answers_attach_and_detach_over_nats() -> TestResult {
    let (_server, client) = start_nats().await?;
    let host = FakeHost::holding(&["lease-1"]);
    let bridge = SshStreamBridge::new(host, client.clone(), limits());
    let cancel = tokio_util::sync::CancellationToken::new();
    let serving = tokio::spawn(Arc::clone(&bridge).serve(client.clone(), cancel.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let backend = NatsSshTunnelBackend::new(client.clone()).with_timeout(Duration::from_secs(5));
    let tenant = trogon_sandbox_contract::TenantId::new("tenant-a")?;
    assert!(backend.attach(&tenant, &attach("s1", "lease-1", 600)?).await?.ok);
    assert_eq!(bridge.session_count(), 1);
    assert!(backend.detach(&tenant, &detach("s1", "lease-1")?).await?.ok);
    eventually(|| bridge.session_count() == 0).await?;

    // Malformed requests and requests without a reply subject get no answer.
    client
        .publish("sandbox.ssh.attach.lease-1", Bytes::from_static(b"garbage"))
        .await?;
    client
        .publish("sandbox.ssh.attach.lease-1", json(&attach("s9", "lease-1", 60)?)?)
        .await?;
    client
        .publish("sandbox.ssh.detach.lease-1", json(&detach("s9", "lease-1")?)?)
        .await?;
    // With a reply subject, a malformed request or one for a lease this worker does not hold is still left unanswered.
    let mut replies = client.subscribe("replies.test").await?;
    client
        .publish_with_reply(
            "sandbox.ssh.attach.lease-1",
            "replies.test",
            Bytes::from_static(b"garbage"),
        )
        .await?;
    client
        .publish_with_reply(
            "sandbox.ssh.attach.other",
            "replies.test",
            json(&attach("s8", "other", 60)?)?,
        )
        .await?;
    client.flush().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), futures::StreamExt::next(&mut replies))
            .await
            .is_err()
    );

    cancel.cancel();
    serving.await??;
    Ok(())
}

#[tokio::test]
async fn serve_ends_when_the_connection_closes() -> TestResult {
    let (_server, client) = start_nats().await?;
    let bridge = SshStreamBridge::new(FakeHost::holding(&["lease-1"]), client.clone(), limits());
    let serving = tokio::spawn(Arc::clone(&bridge).serve(client.clone(), tokio_util::sync::CancellationToken::new()));
    tokio::time::sleep(Duration::from_millis(200)).await;
    client.drain().await?;
    tokio::time::timeout(Duration::from_secs(10), serving).await???;
    Ok(())
}

#[tokio::test]
async fn serve_fails_on_a_closed_connection() -> TestResult {
    let (_server, client) = start_nats().await?;
    let bridge = SshStreamBridge::new(FakeHost::holding(&["lease-1"]), client.clone(), limits());
    client.drain().await?;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let result = bridge.serve(client, tokio_util::sync::CancellationToken::new()).await;
    assert!(result.is_err());
    Ok(())
}
