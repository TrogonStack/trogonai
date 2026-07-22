use super::*;

#[test]
fn mock_client_default() {
    let mock = MockNatsClient::default();
    assert!(mock.published_messages().is_empty());
    assert!(mock.subscribed_to().is_empty());
}

#[tokio::test]
async fn mock_client_tracks_publish() {
    let mock = MockNatsClient::new();
    let _ = mock
        .publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("bar"))
        .await;
    assert_eq!(mock.published_messages(), vec!["foo"]);
    assert_eq!(mock.published_payloads(), vec![bytes::Bytes::from("bar")]);
}

#[tokio::test]
async fn mock_client_tracks_subscribe() {
    let mock = MockNatsClient::new();
    let _ = mock.subscribe("test.sub").await;
    assert_eq!(mock.subscribed_to(), vec!["test.sub"]);
}

#[tokio::test]
async fn mock_client_request_returns_err() {
    let mock = MockNatsClient::new();
    let result = mock
        .request_with_headers("any", async_nats::HeaderMap::new(), bytes::Bytes::from("x"))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("not implemented"));
}

#[test]
fn advanced_mock_default() {
    let mock = AdvancedMockNatsClient::default();
    assert!(mock.published_messages().is_empty());
    assert!(mock.subscribed_to().is_empty());
}

#[test]
fn advanced_mock_clear_responses() {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response("a", "b".into());
    mock.clear_responses();
    assert!(mock.request_responses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn advanced_mock_fail_next_publish_fails_once_then_succeeds() {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_next_publish();

    let first = mock
        .publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("x"))
        .await;
    assert!(first.is_err());

    let second = mock
        .publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("y"))
        .await;
    assert!(second.is_ok());
    assert_eq!(mock.published_messages(), vec!["foo"]);
    assert_eq!(mock.published_payloads(), vec![bytes::Bytes::from("y")]);
}

#[tokio::test]
async fn advanced_mock_hang_next_publish_hangs_once_then_succeeds() {
    let mock = AdvancedMockNatsClient::new();
    mock.hang_next_publish();

    let first = tokio::time::timeout(
        std::time::Duration::from_millis(10),
        mock.publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("x")),
    )
    .await;
    assert!(first.is_err());

    let second = mock
        .publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("y"))
        .await;
    assert!(second.is_ok());
    assert_eq!(mock.published_messages(), vec!["foo"]);
    assert_eq!(mock.published_payloads(), vec![bytes::Bytes::from("y")]);
}

#[tokio::test]
async fn advanced_mock_fail_publish_count_fails_n_times_then_succeeds() {
    let mock = AdvancedMockNatsClient::new();
    mock.fail_publish_count(2);

    assert!(
        mock.publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("1"))
            .await
            .is_err()
    );
    assert!(
        mock.publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("2"))
            .await
            .is_err()
    );
    assert!(
        mock.publish_with_headers("foo", async_nats::HeaderMap::new(), bytes::Bytes::from("3"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn advanced_mock_subscribe_delegates_to_base() {
    let mock = AdvancedMockNatsClient::new();
    let _ = mock.subscribe("test.sub").await;
    assert_eq!(mock.subscribed_to(), vec!["test.sub"]);
}

#[tokio::test]
async fn advanced_mock_request_no_response_configured() {
    let mock = AdvancedMockNatsClient::new();
    let result = mock
        .request_with_headers("missing", async_nats::HeaderMap::new(), bytes::Bytes::from("x"))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("no response configured"));
}

#[test]
fn advanced_mock_debug_format() {
    let mock = AdvancedMockNatsClient::new();
    mock.set_response("a", "b".into());
    let dbg = format!("{:?}", mock);
    assert!(dbg.contains("1 configured responses"));
}

#[test]
fn mock_error_display() {
    let err = MockError("test".into());
    assert_eq!(err.to_string(), "test");
    assert!(std::error::Error::source(&err).is_none());
}
