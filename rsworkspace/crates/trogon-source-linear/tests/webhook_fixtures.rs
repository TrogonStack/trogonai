use axum::body::Body;
use axum::http::Request;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use tower::ServiceExt;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore, StreamMaxAge};
use trogon_source_linear::config::{LinearConfig, LinearWebhookSecret};
use trogon_std::NonZeroDuration;

type HmacSha256 = Hmac<Sha256>;

const TEST_SECRET: &str = "test-secret";

fn compute_sig(body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(TEST_SECRET.as_bytes()).expect("valid key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"))
}

fn make_app() -> (MockJetStreamPublisher, axum::Router) {
    let publisher = MockJetStreamPublisher::new();
    let cc = ClaimCheckPublisher::new(
        publisher.clone(),
        MockObjectStore::new(),
        "test-bucket".to_string(),
        MaxPayload::from_server_limit(usize::MAX),
    );
    let config = LinearConfig {
        webhook_secret: LinearWebhookSecret::new(TEST_SECRET).unwrap(),
        subject_prefix: NatsToken::new("linear").expect("valid token"),
        stream_name: NatsToken::new("LINEAR").expect("valid token"),
        stream_max_age: StreamMaxAge::from_secs(604800).unwrap(),
        // Disable timestamp check so fixtures with old timestamps are accepted.
        timestamp_tolerance: None,
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
    };
    let app = trogon_source_linear::router(cc, &config);
    (publisher, app)
}

fn webhook_request(body: Vec<u8>) -> Request<Body> {
    let sig = compute_sig(&body);
    Request::builder()
        .method("POST")
        .uri("/webhook")
        .header("linear-signature", sig)
        .body(Body::from(body))
        .expect("valid request")
}

// ── Comment.create ────────────────────────────────────────────────────────────

#[tokio::test]
async fn comment_create_routes_to_correct_subject() {
    let body = fixture("linear.Comment.create__comment_created_on_issue.json");
    let (publisher, app) = make_app();

    let resp = app
        .oneshot(webhook_request(body))
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "linear.Comment.create");
}

#[tokio::test]
async fn comment_create_preserves_full_payload() {
    let body = fixture("linear.Comment.create__comment_created_on_issue.json");
    let (publisher, app) = make_app();

    app.oneshot(webhook_request(body.clone()))
        .await
        .expect("request should succeed");

    let messages = publisher.published_messages();
    let published: serde_json::Value = serde_json::from_slice(&messages[0].payload).expect("valid JSON");

    assert_eq!(published["type"], "Comment");
    assert_eq!(published["action"], "create");
    assert_eq!(published["data"]["id"], "00000000-0000-4000-8000-000000000030");
    assert_eq!(published["data"]["issue"]["identifier"], "STM-14");
}

#[tokio::test]
async fn comment_create_sets_dedup_header() {
    let body = fixture("linear.Comment.create__comment_created_on_issue.json");
    let (publisher, app) = make_app();

    app.oneshot(webhook_request(body))
        .await
        .expect("request should succeed");

    let messages = publisher.published_messages();
    assert_eq!(
        messages[0].headers.get("Nats-Msg-Id").map(|v| v.as_str()),
        Some("00000000-0000-4000-8000-000000000002"),
    );
}

// ── Issue.create ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn issue_create_routes_to_correct_subject() {
    let body = fixture("linear.Issue.create__issue_created_in_backlog.json");
    let (publisher, app) = make_app();

    let resp = app
        .oneshot(webhook_request(body))
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "linear.Issue.create");
}

#[tokio::test]
async fn issue_create_preserves_full_payload() {
    let body = fixture("linear.Issue.create__issue_created_in_backlog.json");
    let (publisher, app) = make_app();

    app.oneshot(webhook_request(body.clone()))
        .await
        .expect("request should succeed");

    let messages = publisher.published_messages();
    let published: serde_json::Value = serde_json::from_slice(&messages[0].payload).expect("valid JSON");

    assert_eq!(published["type"], "Issue");
    assert_eq!(published["action"], "create");
    assert_eq!(published["data"]["identifier"], "STM-14");
    assert_eq!(published["data"]["title"], "Demo 2");
}

// ── Issue.update ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn issue_update_routes_to_correct_subject() {
    let body = fixture("linear.Issue.update__issue_moved_to_canceled.json");
    let (publisher, app) = make_app();

    let resp = app
        .oneshot(webhook_request(body))
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "linear.Issue.update");
}

#[tokio::test]
async fn issue_update_with_label_change_preserves_updated_from() {
    // This fixture has updatedFrom showing what changed
    let body = fixture("linear.Issue.update__issue_reopened_to_todo.json");
    let (publisher, app) = make_app();

    app.oneshot(webhook_request(body))
        .await
        .expect("request should succeed");

    let messages = publisher.published_messages();
    let published: serde_json::Value = serde_json::from_slice(&messages[0].payload).expect("valid JSON");

    assert_eq!(published["type"], "Issue");
    assert_eq!(published["action"], "update");
    // updatedFrom tracks what fields changed
    assert!(published["updatedFrom"].is_object());
}

// ── IssueLabel.update ─────────────────────────────────────────────────────────

#[tokio::test]
async fn issue_label_update_routes_to_correct_subject() {
    let body = fixture("linear.IssueLabel.update__label_applied.json");
    let (publisher, app) = make_app();

    let resp = app
        .oneshot(webhook_request(body))
        .await
        .expect("request should succeed");

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "linear.IssueLabel.update");
}

#[tokio::test]
async fn issue_label_update_preserves_full_payload() {
    let body = fixture("linear.IssueLabel.update__label_applied.json");
    let (publisher, app) = make_app();

    app.oneshot(webhook_request(body))
        .await
        .expect("request should succeed");

    let messages = publisher.published_messages();
    let published: serde_json::Value = serde_json::from_slice(&messages[0].payload).expect("valid JSON");

    assert_eq!(published["type"], "IssueLabel");
    assert_eq!(published["action"], "update");
    assert_eq!(published["data"]["name"], "Bug");
    assert_eq!(published["data"]["color"], "#EB5757");
    assert!(published["updatedFrom"]["lastAppliedAt"].is_null());
}
