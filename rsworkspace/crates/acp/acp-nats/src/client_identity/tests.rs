use super::*;
use trogon_nats::AdvancedMockNatsClient;

fn stamped(name: Option<&str>) -> (AdvancedMockNatsClient, ClientStamped<AdvancedMockNatsClient>) {
    let mock = AdvancedMockNatsClient::new();
    let identity = ClientIdentity::default();
    identity.set(name);
    (mock.clone(), ClientStamped::new(mock, identity))
}

#[tokio::test]
async fn a_named_client_is_stamped_on_publishes_and_requests() {
    let (mock, client) = stamped(Some("discord"));
    mock.set_response("subject.request", Bytes::new());

    client
        .publish_with_headers("subject.publish", HeaderMap::new(), Bytes::new())
        .await
        .expect("publish");
    client
        .request_with_headers("subject.request", HeaderMap::new(), Bytes::new())
        .await
        .expect("request");

    let published = mock.published_headers();
    let requested = mock.requested_headers();
    assert_eq!(requesting_client(published.first()), Some("discord"));
    assert_eq!(requesting_client(requested.first()), Some("discord"));
}

#[tokio::test]
async fn an_unnamed_client_sends_no_header() {
    let (mock, client) = stamped(None);

    client
        .publish_with_headers("subject.publish", HeaderMap::new(), Bytes::new())
        .await
        .expect("publish");

    assert_eq!(requesting_client(mock.published_headers().first()), None);
}

#[test]
fn a_name_that_cannot_travel_as_a_header_value_is_dropped() {
    for name in ["", " padded", "padded ", "line\r\nbreak", "tab\there"] {
        let identity = ClientIdentity::default();
        identity.set(Some(name));
        assert!(
            identity.stamp(HeaderMap::new()).get(ACP_CLIENT_HEADER).is_none(),
            "{name:?}"
        );
    }
}

#[test]
fn a_later_initialize_replaces_the_name() {
    let identity = ClientIdentity::default();
    identity.set(Some("discord"));
    identity.set(Some("zed"));
    let headers = identity.stamp(HeaderMap::new());
    assert_eq!(requesting_client(Some(&headers)), Some("zed"));

    identity.set(None);
    assert_eq!(requesting_client(Some(&identity.stamp(HeaderMap::new()))), None);
}

#[test]
fn requests_without_headers_name_no_client() {
    assert_eq!(requesting_client(None), None);
    assert_eq!(requesting_client(Some(&HeaderMap::new())), None);
}
