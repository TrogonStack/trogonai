use super::*;
use trogon_nats::jetstream::MockJetStreamContext;

fn p(s: &str) -> A2aPrefix {
    A2aPrefix::new(s.to_string()).expect("test prefix")
}

#[tokio::test]
async fn provision_creates_both_streams() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("a2a")).await.unwrap();
    assert_eq!(ctx.created_streams().len(), 2);
}

#[tokio::test]
async fn provision_creates_correct_stream_names() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("a2a")).await.unwrap();
    let names: Vec<String> = ctx.created_streams().iter().map(|c| c.name.clone()).collect();
    assert!(names.contains(&"A2A_EVENTS".to_string()));
    assert!(names.contains(&"A2A_PUSH_DLQ".to_string()));
}

#[tokio::test]
async fn provision_with_custom_prefix() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("myapp")).await.unwrap();
    let names: Vec<String> = ctx.created_streams().iter().map(|c| c.name.clone()).collect();
    assert!(names.contains(&"MYAPP_EVENTS".to_string()));
}

#[tokio::test]
async fn provision_returns_error_on_failure() {
    let ctx = MockJetStreamContext::new();
    ctx.fail_next();
    let result = provision_streams(&ctx, &p("a2a")).await;
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().to_string(),
        "stream provisioning failed: A2A_EVENTS: simulated stream creation failure"
    );
}

#[tokio::test]
async fn provision_with_events_max_age_override() {
    use async_nats::jetstream::stream::RetentionPolicy;

    use crate::jetstream::stream_options::{EventsStreamMaxAge, PushDlqDuplicateWindow, StreamProvisionOptions};

    let ctx = MockJetStreamContext::new();
    let options = StreamProvisionOptions {
        events_max_age: EventsStreamMaxAge::from_secs(3600),
        push_dlq_duplicate_window: PushDlqDuplicateWindow::DEFAULT,
    };
    provision_streams_with_options(&ctx, &p("a2a"), &options).await.unwrap();
    let streams = ctx.created_streams();
    let events = streams.iter().find(|c| c.name == "A2A_EVENTS").expect("A2A_EVENTS");
    assert_eq!(events.retention, RetentionPolicy::Interest);
    assert_eq!(events.max_age, std::time::Duration::from_secs(3600));
}

#[tokio::test]
async fn provision_is_idempotent() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("a2a")).await.unwrap();
    provision_streams(&ctx, &p("a2a")).await.unwrap();
    assert_eq!(ctx.created_streams().len(), 4);
}
