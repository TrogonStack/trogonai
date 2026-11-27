use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use tokio::sync::Mutex;
use trogon_sandbox_actor_agent::EXEC_PATH;
use trogon_sandbox_contract::{
    CommandOutput, CommandOutputChunk, LeaseError, LeaseId, OutputStream, RunCommandResponse,
};

use crate::agent::tests::{ndjson, request, serve, target};
use crate::agent::{AgentError, ChunkSink, DiscardChunks};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct RecordingSink {
    chunks: Mutex<Vec<CommandOutputChunk>>,
    begun: Mutex<Vec<String>>,
}

#[async_trait]
impl ChunkSink for RecordingSink {
    async fn publish(&self, chunk: CommandOutputChunk) {
        self.chunks.lock().await.push(chunk);
    }

    async fn begin(&self, execution_id: String) {
        self.begun.lock().await.push(execution_id);
    }
}

struct PublishOnlySink;

#[async_trait]
impl ChunkSink for PublishOnlySink {
    async fn publish(&self, _chunk: CommandOutputChunk) {}
}

fn exec_router(body: &'static str) -> Router {
    Router::new().route(EXEC_PATH, post(move || async move { ndjson(body) }))
}

#[tokio::test]
async fn a_sink_that_ignores_the_begin_frame_still_completes() -> TestResult {
    let agent = serve(
        target()?,
        exec_router(concat!(
            r#"{"begin_execution_id":"e1"}"#,
            "\n",
            r#"{"outcome":"completed","exit_code":0,"stdout":[],"stderr":[]}"#,
            "\n"
        )),
    )
    .await?;

    let outcome = agent.exec(&request()?, &PublishOnlySink).await?;

    assert!(matches!(outcome, RunCommandResponse::Completed(_)));
    Ok(())
}

#[tokio::test]
async fn a_begin_frame_names_a_silent_execution() -> TestResult {
    let agent = serve(
        target()?,
        exec_router(concat!(
            r#"{"begin_execution_id":"e1"}"#,
            "\n",
            r#"{"outcome":"completed","exit_code":0,"stdout":[],"stderr":[]}"#,
            "\n"
        )),
    )
    .await?;

    let sink = Arc::new(RecordingSink::default());
    let outcome = agent.exec(&request()?, sink.as_ref()).await?;

    assert!(matches!(outcome, RunCommandResponse::Completed(_)));
    assert!(sink.chunks.lock().await.is_empty());
    assert_eq!(*sink.begun.lock().await, vec!["e1".to_string()]);
    Ok(())
}

#[tokio::test]
async fn chunks_reach_the_sink_and_the_final_frame_is_returned() -> TestResult {
    let agent = serve(
        target()?,
        exec_router(concat!(
            r#"{"execution_id":"e1","stream":"stdout","seq":0,"data":[104,105]}"#,
            "\n",
            r#"{"execution_id":"e1","stream":"stderr","seq":1,"data":[33]}"#,
            "\n",
            r#"{"outcome":"completed","exit_code":0,"stdout":[104,105],"stderr":[33]}"#,
            "\n"
        )),
    )
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
    assert_eq!(chunks[0].data, b"hi".to_vec());
    assert_eq!(chunks[1].stream, OutputStream::Stderr);
    Ok(())
}

#[tokio::test]
async fn a_response_frame_without_a_trailing_newline_is_still_read() -> TestResult {
    let agent = serve(
        target()?,
        exec_router(r#"{"outcome":"completed","exit_code":7,"stdout":[],"stderr":[]}"#),
    )
    .await?;

    let outcome = agent.exec(&request()?, &DiscardChunks).await?;
    assert_eq!(
        outcome,
        RunCommandResponse::Completed(CommandOutput {
            exit_code: 7,
            ..Default::default()
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_stream_that_dies_before_the_response_frame_is_not_reported_as_success() -> TestResult {
    let agent = serve(
        target()?,
        exec_router(concat!(
            r#"{"execution_id":"e1","stream":"stdout","seq":0,"data":[104]}"#,
            "\n"
        )),
    )
    .await?;

    let error = agent
        .exec(&request()?, &DiscardChunks)
        .await
        .err()
        .ok_or("the exec should fail")?;
    assert!(matches!(error, AgentError::Malformed { .. }), "got {error:?}");
    Ok(())
}

#[tokio::test]
async fn an_unreadable_chunk_does_not_lose_the_command_outcome() -> TestResult {
    let agent = serve(
        target()?,
        exec_router(concat!(
            "{not json at all}\n",
            "\n",
            r#"{"outcome":"completed","exit_code":0,"stdout":[],"stderr":[]}"#,
            "\n"
        )),
    )
    .await?;

    let outcome = agent.exec(&request()?, &DiscardChunks).await?;
    assert!(matches!(outcome, RunCommandResponse::Completed(_)));
    Ok(())
}

#[tokio::test]
async fn a_denial_from_the_agent_is_forwarded_rather_than_translated() -> TestResult {
    let agent = serve(
        target()?,
        exec_router(concat!(
            r#"{"outcome":"denied","kind":"lease_not_found","lease_id":"01J0"}"#,
            "\n"
        )),
    )
    .await?;

    let outcome = agent.exec(&request()?, &DiscardChunks).await?;
    assert_eq!(
        outcome,
        RunCommandResponse::Denied(LeaseError::LeaseNotFound {
            lease_id: LeaseId::new("01J0")
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_non_success_status_is_an_error_and_keeps_the_body() -> TestResult {
    let agent = serve(
        target()?,
        Router::new().route(
            EXEC_PATH,
            post(|| async { (StatusCode::BAD_GATEWAY, "router has no backend").into_response() }),
        ),
    )
    .await?;

    let error = agent
        .exec(&request()?, &DiscardChunks)
        .await
        .err()
        .ok_or("the exec should fail")?;
    assert!(
        matches!(&error, AgentError::Http { status: 502, body, .. } if body.contains("no backend")),
        "got {error:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_body_that_breaks_mid_stream_is_unreachable_not_success() -> TestResult {
    let agent = serve(
        target()?,
        Router::new().route(
            EXEC_PATH,
            post(|| async {
                let frames = futures::stream::iter(vec![
                    Ok(bytes::Bytes::from_static(b"{\"begin_execution_id\":\"e1\"}\n")),
                    Err(std::io::Error::other("the tunnel broke")),
                ]);
                axum::body::Body::from_stream(frames)
            }),
        ),
    )
    .await?;

    let error = agent
        .exec(&request()?, &DiscardChunks)
        .await
        .err()
        .ok_or("the exec should fail")?;
    assert!(matches!(error, AgentError::Unreachable { .. }), "got {error:?}");
    Ok(())
}
