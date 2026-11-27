use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LifecycleEvent, SandboxId};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn a_subject_no_stream_captures_is_logged_and_never_fails() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let event = LifecycleEvent::Started {
        sandbox_id: SandboxId::new("sandbox-1")?,
    };
    publish_lifecycle(&server.jetstream().await, "sandbox.lifecycle.nobody-captures", &event).await;
    Ok(())
}

#[tokio::test]
async fn a_captured_event_is_stored() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    context
        .create_stream(async_nats::jetstream::stream::Config {
            name: "lifecycle_fixture".into(),
            subjects: vec!["sandbox.lifecycle.captured".into()],
            ..Default::default()
        })
        .await?;
    let event = LifecycleEvent::Started {
        sandbox_id: SandboxId::new("sandbox-1")?,
    };
    publish_lifecycle(&context, "sandbox.lifecycle.captured", &event).await;
    let mut stream = context.get_stream("lifecycle_fixture").await?;
    assert_eq!(stream.info().await?.state.messages, 1);
    Ok(())
}

#[tokio::test]
async fn an_event_too_large_to_publish_is_logged_and_never_fails() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let event = LifecycleEvent::Failed {
        sandbox_id: SandboxId::new("sandbox-1")?,
        error: "x".repeat(8 * 1024 * 1024),
    };
    publish_lifecycle(&server.jetstream().await, "sandbox.lifecycle.too-large", &event).await;
    Ok(())
}
