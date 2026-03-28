//! Integration tests for `KvSessionStore` against a real NATS server.
//!
//! These tests require a NATS server with JetStream enabled.
//! Set `NATS_TEST_URL` to run them, e.g.:
//!
//!   NATS_TEST_URL=nats://localhost:4222 cargo test -p trogon-xai-runner \
//!     --features test-helpers --test kv_integration
//!
//! Each test creates its own uniquely named KV bucket and deletes it on exit,
//! so tests can run in parallel safely.

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    Agent, CloseSessionRequest, LoadSessionRequest, NewSessionRequest, SetSessionModelRequest,
    ForkSessionRequest,
};
use trogon_xai_runner::XaiAgent;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Returns the NATS URL from `NATS_TEST_URL`, or `None` to skip the test.
fn nats_url() -> Option<String> {
    std::env::var("NATS_TEST_URL").ok()
}

/// Connect to a real NATS server.
async fn real_nats(url: &str) -> async_nats::Client {
    async_nats::connect(url).await.expect("connect to NATS")
}

/// A unique bucket name for this test run.
fn unique_bucket() -> String {
    format!("XAITEST{}", uuid::Uuid::new_v4().simple())
}

/// Delete the KV bucket at the end of a test.
async fn cleanup(url: &str, bucket: &str) {
    if let Ok(nats) = async_nats::connect(url).await {
        let js = async_nats::jetstream::new(nats);
        js.delete_key_value(bucket).await.ok();
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Session created by one agent instance is visible to a second instance
/// sharing the same KV bucket.
#[tokio::test]
async fn kv_session_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(
        nats_a, prefix.clone(), "grok-3", "key", &bucket,
    ).await.unwrap();

    let sess = agent_a.new_session(NewSessionRequest::new("/workspace")).await.unwrap();
    let session_id = sess.session_id.clone();

    // Second agent, same bucket — should see the session.
    let agent_b = XaiAgent::new_with_kv_bucket(
        nats_b, prefix, "grok-3", "key", &bucket,
    ).await.unwrap();

    agent_b.load_session(LoadSessionRequest::new(session_id, "/workspace")).await.unwrap();

    cleanup(&url, &bucket).await;
}

/// Model change made through one agent is visible to another.
#[tokio::test]
async fn kv_model_change_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(
        nats_a, prefix.clone(), "grok-3", "key", &bucket,
    ).await.unwrap();

    let sess = agent_a.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.clone();

    agent_a
        .set_session_model(SetSessionModelRequest::new(session_id.to_string(), "grok-3-mini"))
        .await
        .unwrap();

    let agent_b = XaiAgent::new_with_kv_bucket(
        nats_b, prefix, "grok-3", "key", &bucket,
    ).await.unwrap();

    let model = agent_b.test_session_model(&session_id.to_string()).await;
    assert_eq!(model.as_deref(), Some("grok-3-mini"));

    cleanup(&url, &bucket).await;
}

/// Closing a session on one agent removes it from KV — the other can no
/// longer load it.
#[tokio::test]
async fn kv_close_session_removes_from_kv() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(
        nats_a, prefix.clone(), "grok-3", "key", &bucket,
    ).await.unwrap();

    let sess = agent_a.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
    let session_id = sess.session_id.clone();

    agent_a
        .close_session(CloseSessionRequest::new(session_id.to_string()))
        .await
        .unwrap();

    let agent_b = XaiAgent::new_with_kv_bucket(
        nats_b, prefix, "grok-3", "key", &bucket,
    ).await.unwrap();

    agent_b
        .load_session(LoadSessionRequest::new(session_id, "/tmp"))
        .await
        .unwrap_err(); // session should be gone

    cleanup(&url, &bucket).await;
}

/// `list_sessions` across two agents sharing a bucket returns all sessions.
#[tokio::test]
async fn kv_list_sessions_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(
        nats_a, prefix.clone(), "grok-3", "key", &bucket,
    ).await.unwrap();
    let agent_b = XaiAgent::new_with_kv_bucket(
        nats_b, prefix, "grok-3", "key", &bucket,
    ).await.unwrap();

    agent_a.new_session(NewSessionRequest::new("/a")).await.unwrap();
    agent_b.new_session(NewSessionRequest::new("/b")).await.unwrap();

    // Each agent should see both sessions when listing.
    let list_a = agent_a
        .list_sessions(agent_client_protocol::ListSessionsRequest::new())
        .await
        .unwrap();
    let list_b = agent_b
        .list_sessions(agent_client_protocol::ListSessionsRequest::new())
        .await
        .unwrap();

    assert_eq!(list_a.sessions.len(), 2);
    assert_eq!(list_b.sessions.len(), 2);

    cleanup(&url, &bucket).await;
}

/// A forked session is persisted in KV and visible to a second agent.
#[tokio::test]
async fn kv_forked_session_persists_across_agent_instances() {
    let Some(url) = nats_url() else { return };
    let bucket = unique_bucket();

    let nats_a = real_nats(&url).await;
    let nats_b = real_nats(&url).await;
    let prefix = AcpPrefix::new("test").unwrap();

    let agent_a = XaiAgent::new_with_kv_bucket(
        nats_a, prefix.clone(), "grok-3", "key", &bucket,
    ).await.unwrap();

    let sess = agent_a.new_session(NewSessionRequest::new("/src")).await.unwrap();
    agent_a
        .set_session_model(SetSessionModelRequest::new(sess.session_id.to_string(), "grok-3-mini"))
        .await
        .unwrap();

    let fork = agent_a
        .fork_session(ForkSessionRequest::new(sess.session_id.clone(), "/fork"))
        .await
        .unwrap();
    let fork_id = fork.session_id.to_string();

    let agent_b = XaiAgent::new_with_kv_bucket(
        nats_b, prefix, "grok-3", "key", &bucket,
    ).await.unwrap();

    // Fork should exist and inherit the model.
    let model = agent_b.test_session_model(&fork_id).await;
    assert_eq!(model.as_deref(), Some("grok-3-mini"));

    cleanup(&url, &bucket).await;
}
