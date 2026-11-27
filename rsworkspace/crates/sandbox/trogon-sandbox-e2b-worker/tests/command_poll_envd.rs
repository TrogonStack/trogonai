#![cfg(unix)]

mod support;

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use serde_json::{Value, json};
use support::{TestResult, outcome, sandbox_at};
use trogon_sandbox_contract::{CommandOutput, LeaseError, PollCommandResponse, RunCommandStartedResponse};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};
use trogon_sandbox_protocol::DEFAULT_MAX_OUTPUT_BYTES;

const MARKER_CHUNKS: usize = 1024;

struct Guest {
    marker: Marker,
    sent: AtomicUsize,
}

enum Marker {
    Valid,
    Invalid,
    Oversized,
}

fn start(request: &[u8]) -> io::Result<Response> {
    let request: Value = serde_json::from_slice(request.get(5..).unwrap_or_default())?;
    let probe = request["process"]["args"].as_array().is_some_and(|args| {
        args.iter().any(|arg| {
            arg.as_str()
                .is_some_and(|arg| arg.starts_with("p=$1\nif IFS= read -r line"))
        })
    });
    let stdout: &[u8] = if probe { b"" } else { b"123 456\n" };
    let mut body = Vec::new();
    for (flag, event) in [
        (0u8, json!({"event":{"start":{"pid":1}}})),
        (
            0,
            json!({"event":{"data":{"stdout":base64::engine::general_purpose::STANDARD.encode(stdout)}}}),
        ),
        (
            0,
            json!({"event":{"end":{"exited":true,"exitCode":if probe {44} else {0}}}}),
        ),
        (2, json!({})),
    ] {
        let bytes = serde_json::to_vec(&event)?;
        body.push(flag);
        body.extend_from_slice(&u32::try_from(bytes.len()).map_err(io::Error::other)?.to_be_bytes());
        body.extend(bytes);
    }
    Ok(([("content-type", "application/connect+json")], body).into_response())
}

async fn file(State(guest): State<Arc<Guest>>, Query(query): Query<BTreeMap<String, String>>) -> Response {
    let path = query.get("path").map(String::as_str).unwrap_or_default();
    if path.ends_with("/exit") {
        return match guest.marker {
            Marker::Valid => "0\n".into_response(),
            Marker::Invalid => "not an exit code".into_response(),
            Marker::Oversized => {
                let stream = futures::stream::unfold((0, guest), |(index, guest)| async move {
                    if index == MARKER_CHUNKS {
                        return None;
                    }
                    guest.sent.fetch_add(1, Ordering::SeqCst);
                    let mut chunk = vec![b' '; 64 * 1024];
                    chunk[0] = if index == 0 { b'0' } else { b' ' };
                    Some((Ok::<_, Infallible>(Bytes::from(chunk)), (index + 1, guest)))
                });
                Body::from_stream(stream).into_response()
            }
        };
    }
    if path.ends_with("/out") {
        let mut data = vec![b'h'; DEFAULT_MAX_OUTPUT_BYTES];
        data.extend(vec![b'm'; DEFAULT_MAX_OUTPUT_BYTES]);
        data.extend(vec![b't'; DEFAULT_MAX_OUTPUT_BYTES]);
        return data.into_response();
    }
    Bytes::new().into_response()
}

async fn poll(
    marker: Marker,
    max_output_bytes: Option<u64>,
) -> Result<(PollCommandResponse, Arc<Guest>), support::Boxed> {
    let guest = Arc::new(Guest {
        marker,
        sent: AtomicUsize::new(0),
    });
    let router = Router::new()
        .route(
            "/process.Process/Start",
            post(|body: Bytes| async move { start(&body).unwrap_or_else(|error| error.to_string().into_response()) }),
        )
        .route("/files", get(file))
        .with_state(guest.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let sandbox = sandbox_at(&format!("http://{}", listener.local_addr()?))?;
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let request = serde_json::from_value(
        json!({"lease_id":"lease-poll","command":["sleep","5"],"detached":true,"max_output_bytes":max_output_bytes}),
    )?;
    let started: RunCommandStartedResponse = outcome(&exec.handle_run_command(&sandbox, &request).await)?;
    let result = exec
        .handle_poll_command(&sandbox, "lease-poll", started.execution_id.as_str())
        .await;
    server.abort();
    Ok((result, guest))
}

#[tokio::test]
async fn malformed_exit_markers_are_refused_instead_of_reporting_the_process_running() -> TestResult {
    let (reply, _) = poll(Marker::Invalid, None).await?;
    assert!(
        matches!(reply, PollCommandResponse::Denied(LeaseError::Backend { .. })),
        "expected a denied poll outcome for the invalid exit marker"
    );
    Ok(())
}

#[tokio::test]
async fn oversized_exit_markers_abort_the_response_before_buffering_the_full_body() -> TestResult {
    let (reply, guest) = poll(Marker::Oversized, None).await?;
    assert!(
        matches!(reply, PollCommandResponse::Denied(LeaseError::Backend { .. })),
        "expected a denied poll outcome for the oversized exit marker"
    );
    assert!(
        guest.sent.load(Ordering::SeqCst) < MARKER_CHUNKS,
        "marker response was fully drained"
    );
    Ok(())
}

#[tokio::test]
async fn detached_output_keeps_the_default_head_and_tail_and_reports_truncation() -> TestResult {
    let (reply, _) = poll(Marker::Valid, None).await?;
    let output: CommandOutput = outcome(&reply)?;
    assert!(output.truncated.is_truncated());
    let marker = format!("\n[... {} bytes truncated ...]\n", DEFAULT_MAX_OUTPUT_BYTES * 2);
    assert_eq!(output.stdout.len(), DEFAULT_MAX_OUTPUT_BYTES + marker.len());
    assert!(
        output.stdout[..DEFAULT_MAX_OUTPUT_BYTES / 2]
            .iter()
            .all(|byte| *byte == b'h')
    );
    assert!(
        output.stdout[output.stdout.len() - DEFAULT_MAX_OUTPUT_BYTES / 2..]
            .iter()
            .all(|byte| *byte == b't')
    );
    assert_eq!(
        &output.stdout[DEFAULT_MAX_OUTPUT_BYTES / 2..output.stdout.len() - DEFAULT_MAX_OUTPUT_BYTES / 2],
        marker.as_bytes()
    );
    Ok(())
}

#[tokio::test]
async fn detached_poll_retains_the_launch_output_budget() -> TestResult {
    let (reply, _) = poll(Marker::Valid, Some(16)).await?;
    let output: CommandOutput = outcome(&reply)?;
    let marker = format!("\n[... {} bytes truncated ...]\n", DEFAULT_MAX_OUTPUT_BYTES * 3 - 16);
    assert_eq!(output.stdout.len(), 16 + marker.len());
    assert_eq!(&output.stdout[..8], b"hhhhhhhh");
    assert_eq!(&output.stdout[8..8 + marker.len()], marker.as_bytes());
    assert_eq!(&output.stdout[8 + marker.len()..], b"tttttttt");
    assert!(output.truncated.is_truncated());
    Ok(())
}
