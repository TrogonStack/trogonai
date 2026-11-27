use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, bail};
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, header};
use axum::response::Response as AxumResponse;
use axum::routing::{get, post};
use futures::StreamExt;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use trogon_sandbox_actor_agent::{GuestEntropyReseedReceipt, GuestEntropySeed};
use trogon_sandbox_contract::{CommandOutput, OutputStream, StdinChunk, StdinEof, StdinSeq};

use super::*;
use crate::config::VsockPort;

#[derive(Default)]
struct RecordingSink {
    chunks: Mutex<Vec<CommandOutputChunk>>,
    begun: Mutex<Vec<ExecutionId>>,
}

#[async_trait]
impl ChunkSink for RecordingSink {
    async fn publish(&self, chunk: CommandOutputChunk) {
        self.chunks.lock().await.push(chunk);
    }

    async fn begin(&self, execution_id: ExecutionId) {
        self.begun.lock().await.push(execution_id);
    }
}

pub(crate) use super::test_support::{serve, serve_unversioned, stamp_version};

pub(crate) fn ndjson(body: &'static str) -> AxumResponse {
    let mut response = AxumResponse::new(Body::from(body));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/x-ndjson"));
    response
}

pub(crate) fn request() -> anyhow::Result<RunCommandRequest> {
    Ok(serde_json::from_value(serde_json::json!({
        "lease_id": "01J0",
        "command": ["true"],
        "timeout_seconds": 5,
        "stream_output": true,
    }))?)
}

fn bind_request(lease: &str, entropy_seed: Option<GuestEntropySeed>, working_dir: Option<&str>) -> BindLeaseRequest {
    BindLeaseRequest {
        lease_id: LeaseId::new(lease),
        entropy_seed,
        env: BTreeMap::new(),
        working_dir: working_dir.map(str::to_string),
        exposed_port: None,
    }
}

fn restored_seed() -> GuestEntropySeed {
    GuestEntropySeed::new([7; GuestEntropySeed::LEN])
}

#[tokio::test]
async fn write_stdin_posts_the_request_and_returns_the_agents_verdict() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(
        EXEC_WRITE_STDIN_PATH,
        post(|axum::Json(request): axum::Json<WriteStdinRequest>| async move {
            assert_eq!(request.data().as_slice(), b"hi");
            axum::Json(WriteStdinResponse::Written {
                next_seq: StdinSeq::new(request.seq().get() + 1),
            })
        }),
    ))
    .await?;

    let response = agent
        .write_stdin(&WriteStdinRequest::new(
            LeaseId::new("01J0"),
            ExecutionId::new("e1"),
            StdinSeq::new(4),
            StdinChunk::new(b"hi".to_vec())?,
            StdinEof::Keep,
        )?)
        .await?;
    assert_eq!(
        response,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(5)
        }
    );
    Ok(())
}

#[tokio::test]
async fn restored_bind_refuses_an_agent_without_kernel_reseed_acknowledgement() -> anyhow::Result<()> {
    let (agent, _tmp) =
        serve(Router::new().route(BIND_PATH, post(|| async { axum::http::StatusCode::NO_CONTENT }))).await?;
    let response = agent.bind(&bind_request("restored", Some(restored_seed()), None)).await;
    assert!(
        response.is_err(),
        "a legacy success response cannot prove the restored kernel RNG was reseeded"
    );
    Ok(())
}

#[tokio::test]
async fn restored_bind_requires_the_exact_lease_seed_and_reseed_method() -> anyhow::Result<()> {
    let receipt = |lease: &str, seed: &[u8]| -> anyhow::Result<serde_json::Value> {
        Ok(serde_json::to_value(BindLeaseResponse {
            lease_id: LeaseId::new(lease),
            entropy_reseed: GuestEntropyReseedReceipt {
                seed_digest: seed_digest(seed)?,
                method: GuestEntropyReseedMethod::LinuxKernelV1,
            },
        })?)
    };
    let mut weak_method = receipt("restored", &[7; 32])?;
    weak_method["entropy_reseed"]["method"] = "write_only".into();
    for (receipt, success) in [
        (receipt("restored", &[7; 32])?, true),
        (receipt("another-lease", &[7; 32])?, false),
        (receipt("restored", &[8; 32])?, false),
        (weak_method, false),
        (serde_json::json!({}), false),
    ] {
        let body = receipt.to_string();
        let (agent, _tmp) = serve(Router::new().route(
            BIND_PATH,
            post(move || {
                let body = body.clone();
                async move { body }
            }),
        ))
        .await?;
        let response = agent.bind(&bind_request("restored", Some(restored_seed()), None)).await;
        assert_eq!(response.is_ok(), success, "{response:?}");
    }
    Ok(())
}

#[tokio::test]
async fn wait_ready_and_bind_run_over_the_vsock_stream() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(
        Router::new()
            .route(READYZ_PATH, get(|| async { "ok" }))
            .route(BIND_PATH, post(|| async { "{}" })),
    )
    .await?;

    agent.wait_ready(Duration::from_secs(5)).await?;
    agent.bind(&bind_request("01J0", None, Some("/workspace"))).await?;
    Ok(())
}

#[tokio::test]
async fn wait_ready_gives_up_when_nothing_listens() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let agent = GuestAgent::new(VsockEndpoint::new(
        tmp.path().join("none.sock"),
        VsockPort::DEFAULT_AGENT,
    ));
    let result = agent.wait_ready(Duration::from_millis(200)).await;
    assert!(matches!(result, Err(AgentError::NotReady { .. })), "{result:?}");
    Ok(())
}

#[tokio::test]
async fn readiness_deadline_bounds_a_guest_that_never_answers() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(
        READYZ_PATH,
        get(|| async { std::future::pending::<&'static str>().await }),
    ))
    .await?;
    let result = tokio::time::timeout(Duration::from_millis(500), agent.wait_ready(Duration::from_millis(50)))
        .await
        .context("a stalled probe must respect the boot deadline")?;
    assert!(matches!(result, Err(AgentError::NotReady { .. })));
    Ok(())
}

fn stalled_body() -> AxumResponse {
    AxumResponse::new(Body::from_stream(futures::stream::pending::<
        Result<Bytes, std::io::Error>,
    >()))
}

#[tokio::test]
async fn a_stalled_control_body_is_bounded() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(IDENTITY_PATH, get(|| async { stalled_body() }))).await?;
    let result = tokio::time::timeout(Duration::from_secs(5), agent.with_timeouts(SHORT).identity())
        .await
        .context("the control deadline must include the body")?;
    assert!(matches!(result, Err(AgentError::Unresponsive { .. })));
    Ok(())
}

#[tokio::test]
async fn a_stalled_exec_body_is_bounded() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(EXEC_PATH, post(|| async { stalled_body() }))).await?;
    let mut request = request()?;
    request.timeout_seconds = 0;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        agent.with_timeouts(SHORT).exec(&request, &DiscardChunks),
    )
    .await
    .context("the exec deadline must include streamed output")?;
    assert!(matches!(result, Err(AgentError::Unresponsive { .. })));
    Ok(())
}

#[tokio::test]
async fn a_begin_frame_names_a_silent_execution() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(
        EXEC_PATH,
        post(|| async {
            ndjson(concat!(
                r#"{"begin_execution_id":"e1"}"#,
                "\n",
                r#"{"outcome":"completed","exit_code":0,"stdout":[],"stderr":[]}"#,
                "\n"
            ))
        }),
    ))
    .await?;

    let sink = Arc::new(RecordingSink::default());
    let outcome = agent.exec(&request()?, sink.as_ref()).await?;

    assert!(matches!(outcome, RunCommandResponse::Completed(_)));
    assert!(sink.chunks.lock().await.is_empty());
    assert_eq!(*sink.begun.lock().await, vec![ExecutionId::new("e1")]);
    Ok(())
}

#[tokio::test]
async fn chunks_reach_the_sink_and_the_final_frame_is_returned() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(
        EXEC_PATH,
        post(|| async {
            ndjson(concat!(
                r#"{"execution_id":"e1","stream":"stdout","seq":0,"data":[104,105]}"#,
                "\n",
                r#"{"execution_id":"e1","stream":"stderr","seq":1,"data":[33]}"#,
                "\n",
                r#"{"outcome":"completed","exit_code":0,"stdout":[104,105],"stderr":[33]}"#,
                "\n"
            ))
        }),
    ))
    .await?;

    let sink = Arc::new(RecordingSink::default());
    let outcome = agent.exec(&request()?, sink.as_ref()).await?;

    assert_eq!(
        outcome,
        RunCommandResponse::Completed(CommandOutput {
            exit_code: 0,
            stdout: b"hi".to_vec(),
            stderr: b"!".to_vec(),
            ..Default::default()
        })
    );
    let chunks = sink.chunks.lock().await;
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].stream, OutputStream::Stdout);
    assert_eq!(chunks[1].stream, OutputStream::Stderr);
    Ok(())
}

#[tokio::test]
async fn a_stream_that_dies_before_the_response_frame_is_not_reported_as_success() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(
        EXEC_PATH,
        post(|| async {
            ndjson(concat!(
                r#"{"execution_id":"e1","stream":"stdout","seq":0,"data":[104]}"#,
                "\n"
            ))
        }),
    ))
    .await?;

    let result = agent.exec(&request()?, &DiscardChunks).await;
    assert!(matches!(result, Err(AgentError::Malformed { .. })), "got {result:?}");
    Ok(())
}

#[tokio::test]
async fn an_http_error_status_is_surfaced_with_its_body() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(
        BIND_PATH,
        post(|| async {
            let mut response = AxumResponse::new(Body::from("already bound"));
            *response.status_mut() = axum::http::StatusCode::CONFLICT;
            response
        }),
    ))
    .await?;

    match agent.bind(&bind_request("01J0", None, None)).await {
        Err(AgentError::Http { status, body, .. }) => {
            assert_eq!(status, 409);
            assert_eq!(body, "already bound");
            Ok(())
        }
        other => bail!("got {other:?}"),
    }
}

const SHORT: AgentTimeouts = AgentTimeouts {
    control: Duration::from_millis(200),
    exec_margin: Duration::from_millis(200),
};

/// Sends the status line and the first bytes of a body, then goes quiet, like a guest
/// that was frozen in the middle of answering.
fn stalls_after(first: &'static [u8]) -> AxumResponse {
    let stream = futures::stream::once(async move { Ok::<_, std::io::Error>(Bytes::from_static(first)) })
        .chain(futures::stream::pending());
    AxumResponse::new(Body::from_stream(stream))
}

async fn assert_unresponsive_within<T: std::fmt::Debug>(
    work: impl Future<Output = Result<T, AgentError>>,
) -> anyhow::Result<()> {
    let result = tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .context("the call must be bounded by the agent's own timeout")?;
    assert!(matches!(result, Err(AgentError::Unresponsive { .. })), "got {result:?}");
    Ok(())
}

#[tokio::test]
async fn a_control_call_whose_body_never_finishes_is_bounded() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(EXEC_POLL_PATH, post(|| async { stalls_after(b"{\"out") }))).await?;
    let agent = agent.with_timeouts(SHORT);

    assert_unresponsive_within(agent.poll(&PollCommandRequest {
        lease_id: LeaseId::new("01J0"),
        execution_id: ExecutionId::new("e1"),
    }))
    .await
}

#[tokio::test]
async fn an_exec_stream_that_goes_quiet_is_bounded() -> anyhow::Result<()> {
    let (agent, _tmp) = serve(Router::new().route(
        EXEC_PATH,
        post(|| async { stalls_after(b"{\"execution_id\":\"e1\",\"stream\":\"stdout\",\"seq\":0,\"data\":[104]}\n") }),
    ))
    .await?;
    let agent = agent.with_timeouts(SHORT);
    let mut request = request()?;
    request.timeout_seconds = 0;

    let sink = Arc::new(RecordingSink::default());
    assert_unresponsive_within(agent.exec(&request, sink.as_ref())).await?;
    assert_eq!(sink.chunks.lock().await.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_guest_that_never_answers_the_vsock_handshake_is_bounded() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("v.sock");
    let listener = UnixListener::bind(&socket)?;
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    let agent = GuestAgent::new(VsockEndpoint::new(socket, VsockPort::new(8110)?)).with_timeouts(SHORT);

    assert_unresponsive_within(agent.bind(&bind_request("01J0", None, None))).await
}
