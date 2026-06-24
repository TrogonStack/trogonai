use super::*;
use crate::acp_prefix::AcpPrefix;
use std::error::Error;
use trogon_nats::jetstream::MockJetStreamContext;

fn p(s: &str) -> AcpPrefix {
    AcpPrefix::new(s).expect("test prefix")
}

#[tokio::test]
async fn provision_creates_six_streams() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("acp")).await.unwrap();
    assert_eq!(ctx.created_streams().len(), 6);
}

#[tokio::test]
async fn provision_creates_correct_stream_names() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("acp")).await.unwrap();
    let names: Vec<String> = ctx.created_streams().iter().map(|c| c.name.clone()).collect();
    assert!(names.contains(&"ACP_COMMANDS".to_string()));
    assert!(names.contains(&"ACP_RESPONSES".to_string()));
    assert!(names.contains(&"ACP_CLIENT_OPS".to_string()));
    assert!(names.contains(&"ACP_NOTIFICATIONS".to_string()));
    assert!(names.contains(&"ACP_GLOBAL".to_string()));
    assert!(names.contains(&"ACP_GLOBAL_EXT".to_string()));
}

#[tokio::test]
async fn provision_with_custom_prefix() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("myapp")).await.unwrap();
    let names: Vec<String> = ctx.created_streams().iter().map(|c| c.name.clone()).collect();
    assert!(names.contains(&"MYAPP_COMMANDS".to_string()));
}

#[tokio::test]
async fn provision_returns_error_on_failure() {
    let ctx = MockJetStreamContext::new();
    ctx.fail_next();
    let result = provision_streams(&ctx, &p("acp")).await;
    let error = result.unwrap_err();
    assert_eq!(error.to_string(), "stream provisioning failed for ACP_COMMANDS");
    assert!(error.source().is_some());
}

#[tokio::test]
async fn provision_is_idempotent() {
    let ctx = MockJetStreamContext::new();
    provision_streams(&ctx, &p("acp")).await.unwrap();
    provision_streams(&ctx, &p("acp")).await.unwrap();
    assert_eq!(ctx.created_streams().len(), 12);
}
