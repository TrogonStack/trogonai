use async_nats::jetstream::stream;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::OutputStream;

use super::*;
use crate::adapter::tests_support::TestResult;

fn event() -> LifecycleEvent {
    LifecycleEvent::Output {
        stream: OutputStream::Stdout,
        text: "hello".to_string(),
    }
}

#[tokio::test]
async fn an_event_is_published_to_the_leases_subject() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut stream = js
        .create_stream(stream::Config {
            name: "LIFECYCLE".to_string(),
            subjects: vec!["lifecycle.lease".to_string()],
            ..Default::default()
        })
        .await?;
    let emitter = NatsLifecycleEmitter::new(js, "lifecycle.lease".to_string());

    emitter.emit(event()).await;

    let mut messages = 0;
    for _ in 0..100 {
        messages = stream.info().await?.state.messages;
        if messages > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(messages, 1);
    Ok(())
}

#[tokio::test]
async fn a_publish_that_cannot_be_sent_is_dropped_without_failing_the_lease() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let emitter = NatsLifecycleEmitter::new(js, String::new());

    emitter.emit(event()).await;
    Ok(())
}
