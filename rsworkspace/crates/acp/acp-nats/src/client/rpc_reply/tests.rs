use super::*;
use agent_client_protocol::{Error, ErrorCode};
use async_nats::subject::ToSubject;
use bytes::Bytes;
use jsonrpc_nats::ResponseId;
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[test]
fn encode_success_sets_jsonrpc_id_header() {
    let encoded = encode_success_for_test(ResponseId::Number(42), &serde_json::json!({"ok": true})).unwrap();
    assert_eq!(encoded.headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(), "42");
    assert!(encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_none());
}

#[test]
fn encode_agent_error_sets_error_code_header() {
    let error = Error::new(ErrorCode::InvalidParams.into(), "test message");
    let encoded = encode_agent_error_for_test(ResponseId::Number(1), &error).unwrap();
    assert_eq!(
        encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).unwrap().as_str(),
        "-32602"
    );
    assert_eq!(encoded.headers.get(jsonrpc_nats::HEADER_ID).unwrap().as_str(), "1");
}

#[test]
fn encode_agent_error_null_id() {
    let error = Error::new(ErrorCode::InternalError.into(), "Internal error");
    let encoded = encode_agent_error_for_test(ResponseId::Null, &error).unwrap();
    assert!(encoded.headers.get(jsonrpc_nats::HEADER_ID).is_none());
    assert!(encoded.headers.get(jsonrpc_nats::HEADER_ERROR_CODE).is_some());
}

#[derive(Clone, Default)]
struct ReplyTransport {
    state: Arc<Mutex<ReplyTransportState>>,
    fail_publish: bool,
    fail_flush: bool,
}

#[derive(Default)]
struct ReplyTransportState {
    operations: Vec<&'static str>,
    published: Vec<(String, HeaderMap, Bytes)>,
}

impl PublishClient for ReplyTransport {
    type PublishError = std::io::Error;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> Result<(), Self::PublishError> {
        let mut state = self.state.lock().unwrap();
        state.operations.push("publish");
        state
            .published
            .push((subject.to_subject().to_string(), headers, payload));
        if self.fail_publish {
            Err(std::io::Error::other("transport unavailable"))
        } else {
            Ok(())
        }
    }
}

impl FlushClient for ReplyTransport {
    type FlushError = std::io::Error;

    async fn flush(&self) -> Result<(), Self::FlushError> {
        self.state.lock().unwrap().operations.push("flush");
        if self.fail_flush {
            Err(std::io::Error::other("flush disconnected"))
        } else {
            Ok(())
        }
    }
}

fn assert_published_reply(transport: &ReplyTransport, expected: Value, id: Option<&str>, code: Option<&str>) {
    let state = transport.state.lock().unwrap();
    assert_eq!(state.operations, ["publish", "flush"]);
    assert_eq!(state.published.len(), 1);
    let (subject, headers, payload) = &state.published[0];
    assert_eq!(subject, "_INBOX.reply-contract");
    assert_eq!(serde_json::from_slice::<Value>(payload).unwrap(), expected);
    assert_eq!(headers.get("Content-Type").unwrap().as_str(), CONTENT_TYPE_JSON);
    assert_eq!(headers.get(jsonrpc_nats::HEADER_ID).map(|value| value.as_str()), id);
    assert_eq!(
        headers.get(jsonrpc_nats::HEADER_ERROR_CODE).map(|value| value.as_str()),
        code
    );
    assert_eq!(
        jsonrpc_nats::decode_value(jsonrpc_nats::Direction::Response, None, headers, payload).unwrap(),
        expected
    );
}

struct UnserializableResult;

impl Serialize for UnserializableResult {
    fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("private implementation detail"))
    }
}

#[tokio::test]
async fn reply_publisher_preserves_canonical_success_and_flushes_once() {
    let transport = ReplyTransport::default();
    publish_success_reply(
        &transport,
        "_INBOX.reply-contract",
        ResponseId::String("request-42".into()),
        &json!({"content": [{"text": "ready"}]}),
        "success contract",
    )
    .await;
    assert_published_reply(
        &transport,
        json!({"jsonrpc": "2.0", "id": "request-42", "result": {"content": [{"text": "ready"}]}}),
        Some("\"request-42\""),
        None,
    );
}

#[tokio::test]
async fn failed_result_serialization_replies_with_correlated_internal_error() {
    let transport = ReplyTransport::default();
    publish_success_reply(
        &transport,
        "_INBOX.reply-contract",
        ResponseId::Number(42),
        &UnserializableResult,
        "serialization contract",
    )
    .await;
    assert_published_reply(
        &transport,
        json!({"jsonrpc": "2.0", "id": 42, "error": {"code": -32603, "message": "Internal error"}}),
        Some("42"),
        Some("-32603"),
    );
}

#[tokio::test]
async fn agent_error_publisher_preserves_structured_error_data() {
    let transport = ReplyTransport::default();
    let mut error = Error::new(ErrorCode::InvalidParams.into(), "Invalid path");
    error.data = Some(json!({"path": "cwd", "allowed": ["relative"]}));
    publish_agent_error_reply(
        &transport,
        "_INBOX.reply-contract",
        ResponseId::Number(9),
        &error,
        "agent error contract",
    )
    .await;
    assert_published_reply(
        &transport,
        json!({"jsonrpc": "2.0", "id": 9, "error": {
            "code": -32602, "message": "Invalid path", "data": {"path": "cwd", "allowed": ["relative"]}
        }}),
        Some("9"),
        Some("-32602"),
    );
}

#[tokio::test]
async fn transport_failures_still_flush_without_duplicate_reply_attempts() {
    for (fail_publish, fail_flush) in [(true, false), (false, true), (true, true)] {
        let transport = ReplyTransport {
            fail_publish,
            fail_flush,
            ..Default::default()
        };
        publish_error_reply(
            &transport,
            "_INBOX.reply-contract",
            ResponseId::Number(7),
            ErrorCode::MethodNotFound,
            "Unsupported method",
            "transport failure contract",
        )
        .await;
        assert_published_reply(
            &transport,
            json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32601, "message": "Unsupported method"}}),
            Some("7"),
            Some("-32601"),
        );
    }
}
