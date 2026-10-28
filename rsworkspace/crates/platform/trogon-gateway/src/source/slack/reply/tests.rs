use super::*;
use async_nats::HeaderMap;
use axum::body::to_bytes;

fn message(content_type: Option<&str>, body: &'static [u8]) -> Message {
    let headers = content_type.map(|value| {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_HEADER_CONTENT_TYPE, value);
        headers
    });
    Message {
        subject: "slack.respond.command.deploy".into(),
        reply: None,
        payload: Bytes::from_static(body),
        headers,
        status: None,
        description: None,
        length: body.len(),
    }
}

fn reply(content_type: Option<&str>, body: &'static [u8]) -> ConsumerReply {
    ConsumerReply::from_message(&message(content_type, body)).expect("non-empty reply")
}

#[test]
fn an_empty_message_is_no_reply() {
    assert_eq!(
        ConsumerReply::from_message(&message(Some("application/json"), b"")),
        None
    );
}

#[test]
fn the_content_type_header_is_optional() {
    assert_eq!(reply(None, b"x").content_type, None);
    assert_eq!(
        reply(Some("application/json"), b"{}").content_type.as_deref(),
        Some("application/json")
    );
}

#[test]
fn text_is_recognised_by_media_type_alone() {
    assert!(reply(Some("text/plain"), b"x").is_text());
    assert!(reply(Some(" Text/Plain ; charset=utf-8"), b"x").is_text());
    assert!(!reply(Some("application/json"), b"x").is_text());
    assert!(!reply(None, b"x").is_text());
}

#[test]
fn a_text_reply_becomes_the_text_field() {
    let payload = reply(Some("text/plain"), "caf\u{e9} \"quoted\"".as_bytes())
        .socket_payload()
        .unwrap();
    assert_eq!(payload.get(), "{\"text\":\"caf\u{e9} \\\"quoted\\\"\"}");
}

#[test]
fn text_that_is_not_utf8_has_no_socket_payload() {
    assert!(reply(Some("text/plain"), &[0xff]).socket_payload().is_none());
}

#[test]
fn a_json_reply_is_embedded_without_being_reserialised() {
    let payload = reply(Some("application/json"), br#"{ "options" : [ ] }"#)
        .socket_payload()
        .unwrap();
    assert_eq!(payload.get(), r#"{ "options" : [ ] }"#);
}

#[test]
fn a_reply_that_is_not_json_has_no_socket_payload() {
    assert!(reply(None, b"not json").socket_payload().is_none());
}

#[tokio::test]
async fn a_reply_is_the_http_body_with_its_own_content_type() {
    let response = Acknowledgement::Reply(reply(Some("application/json"), b"{\"a\":1}")).into_response();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "application/json");
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX).await.unwrap().as_ref(),
        b"{\"a\":1}"
    );
}

#[test]
fn a_reply_without_a_usable_content_type_is_json() {
    for content_type in [None, Some("bad\u{7f}type")] {
        let response = Acknowledgement::Reply(ConsumerReply {
            content_type: content_type.map(str::to_owned),
            body: Bytes::from_static(b"x"),
        })
        .into_response();
        assert_eq!(response.headers().get(CONTENT_TYPE).unwrap(), "application/json");
    }
}

#[tokio::test]
async fn a_status_acknowledgement_is_an_empty_response() {
    let response = Acknowledgement::from(StatusCode::NOT_FOUND).into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(to_bytes(response.into_body(), usize::MAX).await.unwrap().is_empty());
}
