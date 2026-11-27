use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::routing::post;
use futures::StreamExt;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_actor_agent::{EXEC_PATH, EXEC_POLL_PATH};
use trogon_sandbox_contract::{CommandExitMarker, CommandOutput, ExecutionId, PollCommandResponse, RunCommandRequest};

use super::super::exec_streamed;
use super::support::{HELD, TestResult, create_evidence_stream};
use crate::agent::tests::{ndjson, parse, request, serve, target};

fn detached_request() -> Result<RunCommandRequest, Box<dyn std::error::Error>> {
    parse(r#"{"lease_id":"01J0","command":["true"],"timeout_seconds":5,"stream_output":true,"detached":true}"#)
}

/// Runs `request` against an agent answering with `router` and returns the subjects
/// published on the lease's evidence stream, in order, with each exit marker.
async fn evidence(
    router: Router,
    request: RunCommandRequest,
) -> Result<Vec<(String, Option<CommandExitMarker>)>, Box<dyn std::error::Error>> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = server.jetstream().await;
    create_evidence_stream(&js).await?;
    let mut evidence = client.subscribe(format!("sandbox.evidence.{HELD}.>")).await?;
    client.flush().await?;

    let agent = serve(target()?, router).await?;
    exec_streamed(&agent, js, &request).await;

    let mut seen = Vec::new();
    while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_secs(30), evidence.next()).await {
        let subject = msg.subject.to_string();
        let marker = if subject.contains(".command_exit.") {
            Some(serde_json::from_slice(&msg.payload)?)
        } else {
            None
        };
        let done = marker.is_some();
        seen.push((subject, marker));
        if done {
            break;
        }
    }
    Ok(seen)
}

fn marker(execution_id: &str, exit_code: i32) -> Option<CommandExitMarker> {
    Some(CommandExitMarker {
        execution_id: ExecutionId::new(execution_id),
        exit_code: Some(exit_code),
    })
}

#[tokio::test]
async fn the_exit_marker_follows_the_last_chunk() -> TestResult {
    let router = Router::new().route(
        EXEC_PATH,
        post(|| async {
            ndjson(concat!(
                r#"{"begin_execution_id":"e1"}"#,
                "\n",
                r#"{"execution_id":"e1","stream":"stdout","seq":0,"data":[104]}"#,
                "\n",
                r#"{"execution_id":"e1","stream":"stdout","seq":1,"data":[105]}"#,
                "\n",
                r#"{"outcome":"completed","exit_code":3,"stdout":[104,105],"stderr":[]}"#,
                "\n"
            ))
        }),
    );

    let seen = evidence(router, request()?).await?;

    let subjects: Vec<&str> = seen.iter().map(|(subject, _)| subject.as_str()).collect();
    assert_eq!(
        subjects,
        [
            "sandbox.evidence.01J0.command.e1",
            "sandbox.evidence.01J0.command.e1",
            "sandbox.evidence.01J0.command_exit.e1",
        ]
    );
    assert_eq!(seen[2].1, marker("e1", 3));
    Ok(())
}

#[tokio::test]
async fn a_command_that_prints_nothing_still_announces_its_exit() -> TestResult {
    let router = Router::new().route(
        EXEC_PATH,
        post(|| async {
            ndjson(concat!(
                r#"{"begin_execution_id":"e1"}"#,
                "\n",
                r#"{"outcome":"completed","exit_code":0,"stdout":[],"stderr":[]}"#,
                "\n"
            ))
        }),
    );

    let seen = evidence(router, request()?).await?;

    assert_eq!(
        seen,
        [("sandbox.evidence.01J0.command_exit.e1".to_string(), marker("e1", 0))]
    );
    Ok(())
}

#[tokio::test]
async fn a_stream_that_breaks_after_naming_the_command_releases_followers_with_an_unknown_exit() -> TestResult {
    let router = Router::new().route(
        EXEC_PATH,
        post(|| async { ndjson(concat!(r#"{"begin_execution_id":"e5"}"#, "\n")) }),
    );

    let seen = evidence(router, request()?).await?;

    assert_eq!(
        seen,
        [(
            "sandbox.evidence.01J0.command_exit.e5".to_string(),
            Some(CommandExitMarker {
                execution_id: ExecutionId::new("e5"),
                exit_code: None,
            })
        )]
    );
    Ok(())
}

#[tokio::test]
async fn a_detached_command_announces_its_exit_once_it_exits() -> TestResult {
    let polls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            EXEC_PATH,
            post(|| async { ndjson(concat!(r#"{"outcome":"started","execution_id":"e9"}"#, "\n")) }),
        )
        .route(
            EXEC_POLL_PATH,
            post(move || {
                let polls = Arc::clone(&polls);
                async move {
                    if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                        axum::Json(PollCommandResponse::Running)
                    } else {
                        axum::Json(PollCommandResponse::Completed(CommandOutput {
                            exit_code: 7,
                            ..Default::default()
                        }))
                    }
                }
            }),
        );

    let seen = evidence(router, detached_request()?).await?;

    assert_eq!(
        seen,
        [("sandbox.evidence.01J0.command_exit.e9".to_string(), marker("e9", 7))]
    );
    Ok(())
}

#[tokio::test]
async fn an_agent_without_a_begin_frame_is_named_by_its_first_chunk() -> TestResult {
    let router = Router::new().route(
        EXEC_PATH,
        post(|| async {
            ndjson(concat!(
                r#"{"execution_id":"e2","stream":"stdout","seq":0,"data":[104]}"#,
                "\n",
                r#"{"outcome":"completed","exit_code":0,"stdout":[104],"stderr":[]}"#,
                "\n"
            ))
        }),
    );

    let seen = evidence(router, request()?).await?;

    let (subject, exit) = seen.last().ok_or("nothing was announced")?;
    assert_eq!(subject, "sandbox.evidence.01J0.command_exit.e2");
    assert_eq!(*exit, marker("e2", 0));
    Ok(())
}
