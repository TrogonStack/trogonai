//! Port publishing (`SandboxSpec::exposed_ports` /
//! `get_sandbox_domain_for_lease`) and command output streaming
//! (`RunCommandRequest::stream_output`) integration tests.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker port_and_streaming
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_id, fresh_lease_id, lease_request_exposing};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_protocol::RedactionSet;

use std::collections::BTreeMap;
use std::time::Duration;

use futures::StreamExt as _;
use trogon_sandbox_contract::{
    BootMode, CommandDeadline, CommandEnv, CommandOutputChunk, ExecutionId, GetSandboxDomainResponse, LeaseRequest,
    OutputRetention, OutputStream, StdinMode, StreamedOutput,
};
use trogon_sandbox_protocol::Provider as _;
use trogon_sandbox_protocol::subjects;

/// `bare_lease_request` asks for `exposed_ports: vec![8080]`, so a fresh
/// lease publishes that port at container-create time and
/// `get_sandbox_domain_for_lease` must resolve it to a reachable
/// `127.0.0.1:{host_port}` endpoint, mirroring Vercel Sandbox's
/// `sandbox.domain(port)`.
fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request_exposing("port-and-streaming-test", BootMode::Bare, &[8080])
}

#[tokio::test(flavor = "multi_thread")]
async fn get_sandbox_domain_reports_available_for_exposed_port() -> TestResult {
    let provider = build_provider("port_and_streaming").await?;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let response = provider.get_sandbox_domain_for_lease(&lease_id, 8080).await;
    assert!(
        matches!(response, GetSandboxDomainResponse::Available { .. }),
        "expected Available, got {response:?}"
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}

/// A port that was not in `exposed_ports` still resolves after create: the
/// worker forwards a loopback listener through the container, which also
/// works under `network_mode: none` (the bare lease has no egress).
#[tokio::test(flavor = "multi_thread")]
async fn get_sandbox_domain_forwards_a_port_opened_after_create() -> TestResult {
    let provider = build_provider("port_and_streaming").await?;

    let req = lease_request_exposing("port-and-streaming-test", BootMode::Bare, &[])?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    provider
        .run_command_for_lease_detached(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "while true; do echo pong | nc -l -p 9000; done".to_string(),
            ],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    let response = provider.get_sandbox_domain_for_lease(&lease_id, 9000).await;
    let GetSandboxDomainResponse::Available { endpoint, .. } = response else {
        return Err(format!("expected Available, got {response:?}").into());
    };
    let again = provider.get_sandbox_domain_for_lease(&lease_id, 9000).await;
    assert!(
        matches!(&again, GetSandboxDomainResponse::Available { endpoint: e, .. } if *e == endpoint),
        "a second lookup reuses the forward, got {again:?}"
    );

    let reply = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let mut stream = tokio::net::TcpStream::connect(&endpoint).await?;
            let _ = stream.write_all(b"ping\n").await;
            // Busybox nc exits on stdin EOF, not on the remote closing.
            let _ = stream.shutdown().await;
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf).await;
            if !buf.is_empty() {
                return Ok::<String, std::io::Error>(String::from_utf8_lossy(&buf).into_owned());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await??;
    assert_eq!(reply.trim(), "pong");

    provider.cancel(&lease_id).await?;
    assert!(
        tokio::net::TcpStream::connect(&endpoint).await.is_err(),
        "the forward stops listening once the lease is released"
    );
    Ok(())
}

/// Streams a multi-line command's stdout and asserts the published
/// `CommandOutputChunk`s carry a single, strictly increasing `seq` across the
/// whole run (there is only one stream in play here; `multi_user_exec.rs`-style
/// stdout/stderr interleaving is covered by the pure `ChunkSequencer` unit
/// tests in `adapter.rs`, which do not need a live container).
#[tokio::test(flavor = "multi_thread")]
async fn streamed_command_publishes_chunks_with_increasing_seq() -> TestResult {
    let provider = build_provider("port_and_streaming").await?;
    let nats = provider.server.client().await;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    // Only output chunks; the lease also publishes lifecycle events and exit
    // markers under its evidence subject.
    let chunk_subject = format!(
        "sandbox.evidence.{}.{}",
        lease_id.as_str(),
        subjects::evidence_command_suffix(&ExecutionId::new("*"))
    );
    let mut sub = nats.subscribe(chunk_subject).await?;

    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo one; sleep 0.3; echo two; sleep 0.3; echo three".to_string(),
            ],
            None,
            &CommandEnv::default(),
            30,
            None,
            StreamedOutput::Enabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    let chunks = tokio::time::timeout(Duration::from_secs(10), async {
        // The worker may coalesce lines into fewer chunks, so wait for the
        // content rather than a chunk count.
        let mut collected: Vec<CommandOutputChunk> = Vec::new();
        while !collected
            .iter()
            .flat_map(|c| c.data.iter().copied())
            .collect::<Vec<u8>>()
            .ends_with(b"three\n")
        {
            let msg = sub.next().await.ok_or("the chunk subscription closed")?;
            let chunk: CommandOutputChunk = serde_json::from_slice(&msg.payload)?;
            collected.push(chunk);
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(collected)
    })
    .await??;

    for chunk in &chunks {
        assert_eq!(chunk.stream, OutputStream::Stdout);
    }
    let data: Vec<u8> = chunks.iter().flat_map(|c| c.data.iter().copied()).collect();
    assert_eq!(data, b"one\ntwo\nthree\n");
    let seqs: Vec<u64> = chunks.iter().map(|c| c.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    assert_eq!(seqs, sorted, "seq must be non-decreasing as chunks arrive");
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "seq must strictly increase across chunks, got {seqs:?}"
    );

    provider.cancel(&lease_id).await?;
    let _ = execution_id;
    Ok(())
}

/// A literal registered for redaction never reaches the buffered output or the
/// streamed chunks, even though the command echoes it from its environment.
#[tokio::test(flavor = "multi_thread")]
async fn registered_values_are_redacted_from_buffered_and_streamed_output() -> TestResult {
    let provider = build_provider("port_and_streaming").await?;
    let nats = provider.server.client().await;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let secret = format!("tok-{}", fresh_id());
    let env = CommandEnv::new(BTreeMap::from([("API_TOKEN".to_string(), secret.clone())]))?;
    let redaction = RedactionSet::new([secret.as_str()]);
    // Only output chunks; the lease also publishes lifecycle events and exit
    // markers under its evidence subject.
    let chunk_subject = format!(
        "sandbox.evidence.{}.{}",
        lease_id.as_str(),
        subjects::evidence_command_suffix(&ExecutionId::new("*"))
    );
    let mut sub = nats.subscribe(chunk_subject).await?;

    let output = provider
        .run_command_for_lease_redacting(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo token=$API_TOKEN; echo again=$API_TOKEN >&2".to_string(),
            ],
            None,
            &env,
            30,
            None,
            StreamedOutput::Enabled,
            usize::MAX,
            &redaction,
        )
        .await?;

    assert_eq!(output.stdout, b"token=[REDACTED]\n");
    assert_eq!(output.stderr, b"again=[REDACTED]\n");

    let mut streamed = Vec::new();
    while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_secs(2), sub.next()).await {
        let chunk: CommandOutputChunk = serde_json::from_slice(&msg.payload)?;
        streamed.extend_from_slice(&chunk.data);
    }
    let streamed = String::from_utf8_lossy(&streamed);
    assert!(streamed.contains("[REDACTED]"), "{streamed}");
    assert!(!streamed.contains(secret.as_str()), "{streamed}");

    provider.cancel(&lease_id).await?;
    Ok(())
}

/// Runs `command` as a streamed detached command on a fresh lease and returns
/// the first exit marker published for it.
async fn exit_marker_of_streamed(
    command: Vec<String>,
    timeout_seconds: u32,
) -> TestResult<trogon_sandbox_contract::CommandExitMarker> {
    let provider = build_provider("port_and_streaming").await?;
    let nats = provider.server.client().await;

    let req = bare_lease_request()?;
    let lease_id = fresh_lease_id();
    provider.lease(&req, &lease_id).await?;

    let mut sub = nats
        .subscribe(format!(
            "sandbox.evidence.{}.{}",
            lease_id.as_str(),
            subjects::evidence_command_exit_suffix(&ExecutionId::new("*"))
        ))
        .await?;
    nats.flush().await?;

    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            command,
            None,
            &CommandEnv::default(),
            timeout_seconds,
            None,
            StreamedOutput::Enabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    let msg = tokio::time::timeout(Duration::from_secs(30), sub.next())
        .await?
        .ok_or("the exit subscription closed")?;
    let marker: trogon_sandbox_contract::CommandExitMarker = serde_json::from_slice(&msg.payload)?;
    assert_eq!(marker.execution_id, execution_id);

    provider.cancel(&lease_id).await?;
    Ok(marker)
}

/// A command that outlives its timeout never reaches its own exit marker, so
/// the worker must announce the cut-short exit or a log follower waits forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_command_that_times_out_still_publishes_an_exit_marker() -> TestResult {
    let marker = exit_marker_of_streamed(vec!["sleep".to_string(), "60".to_string()], 1).await?;
    assert_eq!(marker.exit_code, Some(-1));
    Ok(())
}

/// A command the engine cannot start fails the exec instead of exiting, and
/// that failure must still release a follower with an exit marker.
#[tokio::test(flavor = "multi_thread")]
async fn a_streamed_command_that_errors_still_publishes_an_exit_marker() -> TestResult {
    let marker = exit_marker_of_streamed(vec!["/no/such/binary".to_string()], 30).await?;
    // The engine reports a missing binary as the shell convention for
    // "command not found", so the marker carries that code rather than the
    // -1 of an exec the worker abandoned.
    assert_eq!(marker.exit_code, Some(127), "{marker:?}");
    Ok(())
}
