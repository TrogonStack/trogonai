use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::routing::post;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_actor_agent::{EXEC_PATH, EXEC_POLL_PATH};
use trogon_sandbox_contract::CommandExitMarker;

use super::*;
use crate::agent::tests::{ndjson, request, serve};

/// Runs `request` against an agent answering with `router` and returns the subjects
/// published on the lease's evidence stream, in order, with each exit marker.
async fn evidence(
    router: Router,
    request: RunCommandRequest,
) -> anyhow::Result<Vec<(String, Option<CommandExitMarker>)>> {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let mut evidence = client
        .subscribe(format!("sandbox.evidence.{}.>", request.lease_id))
        .await?;
    client.flush().await?;

    let (agent, _socket) = serve(router).await?;
    exec_streamed(&agent, js, &request).await;

    // Wait on the event itself, the exit marker, which every run ends with. The ceiling
    // only turns a lost marker into a failure instead of a hang; a loaded machine that
    // delivers late still passes.
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(msg) = evidence.next().await {
            let subject = msg.subject.to_string();
            let marker = subject
                .contains(".command_exit.")
                .then(|| serde_json::from_slice(&msg.payload))
                .transpose()?;
            let done = marker.is_some();
            seen.push((subject, marker));
            if done {
                break;
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .map_err(|_| anyhow::anyhow!("no exit marker arrived; saw {seen:?}"))??;
    Ok(seen)
}

#[tokio::test]
async fn the_exit_marker_follows_the_last_chunk() -> anyhow::Result<()> {
    let router = Router::new().route(
        EXEC_PATH,
        post(|| async {
            ndjson(concat!(
                r#"{"begin_execution_id":"e1"}"#,
                "\n",
                r#"{"execution_id":"e1","stream":"stdout","seq":0,"data":[104]}"#,
                "\n",
                r#"{"outcome":"completed","exit_code":3,"stdout":[104],"stderr":[]}"#,
                "\n"
            ))
        }),
    );

    let seen = evidence(router, request()?).await?;

    let subjects: Vec<&str> = seen.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(
        subjects,
        [
            "sandbox.evidence.01J0.command.e1",
            "sandbox.evidence.01J0.command_exit.e1",
        ]
    );
    assert_eq!(
        seen[1].1,
        Some(CommandExitMarker {
            execution_id: ExecutionId::new("e1"),
            exit_code: Some(3),
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_command_that_prints_nothing_still_announces_its_exit() -> anyhow::Result<()> {
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
        [(
            "sandbox.evidence.01J0.command_exit.e1".to_string(),
            Some(CommandExitMarker {
                execution_id: ExecutionId::new("e1"),
                exit_code: Some(0),
            })
        )]
    );
    Ok(())
}

#[tokio::test]
async fn a_detached_command_announces_its_exit_once_it_exits() -> anyhow::Result<()> {
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
                        axum::Json(PollCommandResponse::Completed(trogon_sandbox_contract::CommandOutput {
                            exit_code: 7,
                            ..Default::default()
                        }))
                    }
                }
            }),
        );
    let detached: RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id": "01J0",
        "command": ["true"],
        "timeout_seconds": 5,
        "stream_output": true,
        "detached": true,
    }))?;

    let seen = evidence(router, detached).await?;

    assert_eq!(
        seen,
        [(
            "sandbox.evidence.01J0.command_exit.e9".to_string(),
            Some(CommandExitMarker {
                execution_id: ExecutionId::new("e9"),
                exit_code: Some(7),
            })
        )]
    );
    Ok(())
}
