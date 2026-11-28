use super::*;
use agent_client_protocol::ErrorCode;
use agent_client_protocol::v1::schema::{ContentBlock, ContentChunk, SessionUpdate};
use trogon_nats::AdvancedMockNatsClient;

fn routed(nats: AdvancedMockNatsClient) -> SessionRoutedClient<AdvancedMockNatsClient> {
    SessionRoutedClient::new(nats, AcpPrefix::new("acp").expect("prefix"), Duration::from_secs(5))
}

fn chunk(session: &str) -> SessionNotification {
    SessionNotification::new(
        session.to_string(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from("hello"))),
    )
}

#[tokio::test]
async fn each_sessions_updates_land_on_that_sessions_subject() {
    let nats = AdvancedMockNatsClient::new();
    let client = routed(nats.clone());

    client.session_notification(chunk("s1")).await.expect("s1 update");
    client.session_notification(chunk("s2")).await.expect("s2 update");

    assert_eq!(
        nats.published_messages(),
        vec![
            "acp.v1.session.s1.client.session.update",
            "acp.v1.session.s2.client.session.update",
        ]
    );
}

#[tokio::test]
async fn a_session_id_that_cannot_be_a_subject_token_is_rejected_as_bad_params() {
    let nats = AdvancedMockNatsClient::new();
    let client = routed(nats.clone());

    let error = client
        .session_notification(chunk("not.a.token"))
        .await
        .expect_err("dotted id cannot be routed");

    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    assert!(nats.published_messages().is_empty());
}

#[tokio::test]
async fn a_failed_publish_reaches_the_agent_as_an_error() {
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let client = routed(nats);

    assert!(client.session_notification(chunk("s1")).await.is_err());
}

fn raw(json: &str) -> std::sync::Arc<RawValue> {
    RawValue::from_string(json.to_string()).expect("raw json").into()
}

#[tokio::test]
async fn an_extension_notification_lands_on_its_sessions_ext_subject() {
    let nats = AdvancedMockNatsClient::new();
    let client = routed(nats.clone());

    client
        .ext_notification(ExtNotification::new("_vendor/x", raw(r#"{"sessionId":"s1","n":1}"#)))
        .await
        .expect("routed");

    assert_eq!(
        nats.published_messages(),
        vec!["acp.v1.session.s1.client.ext._vendor/x"]
    );
}

#[tokio::test]
async fn an_extension_call_without_a_session_is_refused_as_bad_params() {
    let nats = AdvancedMockNatsClient::new();
    let client = routed(nats.clone());

    let request = client.ext_method(ExtRequest::new("_vendor/x", raw(r#"{"n":1}"#))).await;
    let notification = client
        .ext_notification(ExtNotification::new("_vendor/x", raw("[]")))
        .await;

    assert_eq!(request.expect_err("no session").code, ErrorCode::INVALID_PARAMS);
    assert_eq!(notification.expect_err("no session").code, ErrorCode::INVALID_PARAMS);
    assert!(nats.published_messages().is_empty());
}

mod extension_round_trip;
