use super::*;
use acp_nats::Config;
use axum::body::{Body, to_bytes};
use axum::http::Request as HttpRequest;
use axum::http::header::{HOST, ORIGIN};
use serde_json::{Value, json};
use std::error::Error as _;
use std::net::{IpAddr, Ipv4Addr};
use tokio::sync::{mpsc, oneshot, watch};
use trogon_nats::AdvancedMockNatsClient;

#[derive(Clone)]
struct MockJs {
    publisher: trogon_nats::jetstream::MockJetStreamPublisher,
    consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory,
}

impl MockJs {
    fn new() -> Self {
        Self {
            publisher: trogon_nats::jetstream::MockJetStreamPublisher::new(),
            consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory::new(),
        }
    }
}

impl trogon_nats::jetstream::JetStreamPublisher for MockJs {
    type PublishError = trogon_nats::mocks::MockError;
    type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: bytes::Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.publisher.publish_with_headers(subject, headers, payload).await
    }
}

impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
    type Error = async_nats::jetstream::context::GetStreamError;
    type Stream = trogon_nats::jetstream::MockJetStreamStream;

    async fn get_stream<T: AsRef<str> + Send>(&self, stream_name: T) -> Result<Self::Stream, Self::Error> {
        self.consumer_factory.get_stream(stream_name).await
    }
}

fn test_config() -> Config {
    Config::new(
        acp_nats::AcpPrefix::new("acp").unwrap(),
        acp_nats::NatsConfig {
            servers: vec!["localhost:4222".to_string()],
            auth: trogon_nats::NatsAuth::None,
        },
    )
}

fn test_state() -> (AppState, mpsc::UnboundedReceiver<ManagerRequest>) {
    let (manager_tx, manager_rx) = mpsc::unbounded_channel();
    let (shutdown_tx, _) = watch::channel(false);
    (
        AppState {
            bind_host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            manager_tx,
            shutdown_tx,
        },
        manager_rx,
    )
}

async fn json_event_body(response: Response) -> Vec<Value> {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|json| !json.is_empty())
        .map(|json| serde_json::from_str(json).unwrap())
        .collect()
}

fn post_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/event-stream"));
    headers
}

fn get_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    headers
}

fn session_id() -> acp_nats::AcpSessionId {
    acp_nats::AcpSessionId::new("session-1").unwrap()
}

#[test]
fn http_transport_error_into_response_maps_status_codes() {
    let cases = [
        (HttpTransportError::bad_request("bad"), StatusCode::BAD_REQUEST),
        (HttpTransportError::not_found("missing"), StatusCode::NOT_FOUND),
        (HttpTransportError::conflict("conflict"), StatusCode::CONFLICT),
        (HttpTransportError::forbidden("forbidden"), StatusCode::FORBIDDEN),
        (
            HttpTransportError::unsupported_media_type("unsupported"),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        (
            HttpTransportError::not_acceptable("not-acceptable"),
            StatusCode::NOT_ACCEPTABLE,
        ),
        (
            HttpTransportError::internal("internal"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];

    for (error, status) in cases {
        assert_eq!(error.into_response().status(), status);
    }

    let sourced = IncomingHttpMessage::parse("{".to_string()).unwrap_err();
    assert_eq!(sourced.into_response().status(), StatusCode::BAD_REQUEST);
}

#[test]
fn http_transport_error_display_and_source_chain_return_expected_values() {
    assert_eq!(HttpTransportError::bad_request("bad").to_string(), "bad");
    assert!(HttpTransportError::bad_request("bad").source().is_none());
    assert!(HttpTransportError::not_found("missing").source().is_none());
    assert_eq!(HttpTransportError::conflict("conflict").to_string(), "conflict");
    assert!(HttpTransportError::conflict("conflict").source().is_none());
    assert_eq!(HttpTransportError::forbidden("forbidden").to_string(), "forbidden");
    assert!(HttpTransportError::forbidden("forbidden").source().is_none());
    assert!(
        HttpTransportError::unsupported_media_type("unsupported")
            .source()
            .is_none()
    );
    assert!(HttpTransportError::not_acceptable("not-acceptable").source().is_none());
    assert_eq!(HttpTransportError::internal("internal").to_string(), "internal");
    assert!(HttpTransportError::internal("internal").source().is_none());

    let invalid_json = IncomingHttpMessage::parse("{".to_string()).unwrap_err();
    assert_eq!(invalid_json.to_string(), "invalid JSON-RPC payload");
    assert!(invalid_json.source().is_some());
}

#[test]
fn incoming_http_message_parses_and_classifies_shapes() {
    let request = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":".","mcpServers":[]}}"#.to_string(),
    )
    .unwrap();
    assert!(request.is_request());
    assert!(request.creates_session());
    assert!(!request.requires_session_id());

    for raw in [
        r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"session-1","cwd":"."}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"session/resume","params":{"sessionId":"session-1","cwd":"."}}"#,
    ] {
        let attach = IncomingHttpMessage::parse(raw.to_string()).unwrap();
        assert!(attach.attaches_session());
        assert!(attach.requires_session_id());
    }

    let fork = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":4,"method":"session/fork","params":{"sessionId":"session-1","cwd":"."}}"#.to_string(),
    )
    .unwrap();
    assert!(fork.creates_session());
    assert!(fork.requires_session_id());

    let notification = IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string()).unwrap();
    assert!(notification.is_notification());
    assert!(!notification.requires_session_id());

    let response = IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":99,"result":{"ok":true}}"#.to_string()).unwrap();
    assert!(response.is_response());
    assert!(response.requires_session_id());

    let null_response = IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":100,"result":null}"#.to_string()).unwrap();
    assert!(null_response.is_response());
    assert!(null_response.requires_session_id());
}

#[test]
fn incoming_http_message_parse_all_unbundles_batch_and_rejects_empty_batch() {
    let batch = IncomingHttpMessage::parse_all(r#"[{"jsonrpc":"2.0","method":"initialized"}]"#.to_string()).unwrap();
    assert_eq!(batch.len(), 1);
    assert!(batch[0].is_notification());

    let empty = IncomingHttpMessage::parse_all("[]".to_string()).unwrap_err();
    assert!(matches!(
        empty,
        HttpTransportError::BadRequest {
            message: "empty JSON-RPC batch",
            source: None,
        }
    ));

    let invalid = IncomingHttpMessage::parse("{".to_string()).unwrap_err();
    assert!(matches!(
        invalid,
        HttpTransportError::BadRequest {
            message: "invalid JSON-RPC payload",
            source: Some(_),
        }
    ));
}

#[test]
fn incoming_http_message_session_id_helpers_handle_valid_and_invalid_values() {
    let message = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{"sessionId":"session-1"}}"#.to_string(),
    )
    .unwrap();
    assert_eq!(message.params_session_id().unwrap(), Some(session_id()));

    let invalid = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{"sessionId":"bad.session"}}"#.to_string(),
    )
    .unwrap();
    assert!(matches!(
        invalid.params_session_id(),
        Err(HttpTransportError::BadRequest {
            message: "invalid sessionId in request body",
            source: Some(_),
        })
    ));
}

#[test]
fn outgoing_http_message_extracts_session_ids() {
    let request =
        OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":1,"method":"prompt","params":{"text":"hi"}}"#).unwrap();
    assert!(!request.is_response());

    let response =
        OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"session-1"}}"#).unwrap();
    assert!(response.is_response());
    assert_eq!(response.result_session_id(), Some(session_id()));
    assert!(response.is_success_response());

    let null_result = OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":3,"result":null}"#).unwrap();
    assert!(null_result.is_response());
    assert_eq!(null_result.result_session_id(), None);
    assert!(null_result.is_success_response());

    let empty_success = OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":4}"#).unwrap();
    assert!(empty_success.is_response());
    assert!(empty_success.is_success_response());

    let error_response =
        OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32000,"message":"boom"}}"#).unwrap();
    assert!(error_response.is_response());
    assert!(!error_response.is_success_response());
}

#[test]
fn activate_attached_session_on_success_tracks_matching_success_responses() {
    let request_id = RequestId::Number(2);
    let session_id = session_id();
    let mut sessions = std::collections::HashSet::new();
    let response = OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":2,"result":null}"#).unwrap();

    activate_attached_session_on_success(&mut sessions, &request_id, Some(&session_id), Some(&response));

    assert!(sessions.contains(&session_id));
}

#[test]
fn activate_attached_session_on_success_ignores_errors_and_other_request_ids() {
    let request_id = RequestId::Number(2);
    let session_id = session_id();
    let mut sessions = std::collections::HashSet::new();
    let other_response = OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":3}"#).unwrap();
    let error_response =
        OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"boom"}}"#).unwrap();

    activate_attached_session_on_success(&mut sessions, &request_id, Some(&session_id), Some(&other_response));
    activate_attached_session_on_success(&mut sessions, &request_id, Some(&session_id), Some(&error_response));

    assert!(!sessions.contains(&session_id));
}

#[test]
fn pending_response_context_only_injects_session_id_for_attach_requests() {
    let session_id = session_id();
    let attach = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"session-1","cwd":"."}}"#.to_string(),
    )
    .unwrap();
    let attach_context = pending_response_context(&attach, session_id.clone()).unwrap();
    assert!(attach_context.activate_session_on_success);
    assert!(attach_context.inject_session_id);

    let authenticate =
        IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":3,"method":"authenticate"}"#.to_string()).unwrap();
    let authenticate_context = pending_response_context(&authenticate, session_id.clone()).unwrap();
    assert!(!authenticate_context.activate_session_on_success);
    assert!(!authenticate_context.inject_session_id);

    let create = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":4,"method":"session/fork","params":{"sessionId":"session-1","cwd":"."}}"#.to_string(),
    )
    .unwrap();
    assert!(pending_response_context(&create, session_id).is_none());
}

#[test]
fn targets_unknown_session_requires_a_known_session_header() {
    let session_id = session_id();
    let prompt = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"session-1"}}"#.to_string(),
    )
    .unwrap();
    let initialize = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.to_string(),
    )
    .unwrap();
    let mut sessions = std::collections::HashSet::new();

    assert!(targets_unknown_session(&prompt, Some(&session_id), &sessions));
    assert!(!targets_unknown_session(&prompt, None, &sessions));
    assert!(!targets_unknown_session(&initialize, Some(&session_id), &sessions));

    sessions.insert(session_id.clone());
    assert!(!targets_unknown_session(&prompt, Some(&session_id), &sessions));
}

#[test]
fn require_initialized_rejects_uninitialized_connections() {
    assert!(require_initialized(true).is_ok());
    assert!(matches!(
        require_initialized(false),
        Err(HttpTransportError::BadRequest {
            message: "ACP connection has not been initialized",
            source: None,
        })
    ));
}

#[test]
fn dispatch_to_get_listeners_returns_missing_when_no_listeners() {
    let frame = SseFrame::json(r#"{"jsonrpc":"2.0","id":2,"result":null}"#.to_string());
    let outcome = dispatch_to_get_listeners(&frame, &mut Vec::new());

    assert!(matches!(outcome, ListenerDispatch::Missing));
}

#[test]
fn dispatch_to_get_listeners_drops_full_listener() {
    let mut get_listeners = Vec::new();
    let (listener_tx, mut listener_rx) = mpsc::channel(1);
    listener_tx
        .try_send(SseFrame::json(
            r#"{"jsonrpc":"2.0","method":"session/update"}"#.to_string(),
        ))
        .unwrap();
    get_listeners.push(listener_tx);

    let frame = SseFrame::json(r#"{"jsonrpc":"2.0","method":"session/update"}"#.to_string());
    let outcome = dispatch_to_get_listeners(&frame, &mut get_listeners);

    assert!(matches!(outcome, ListenerDispatch::Dropped));
    assert!(get_listeners.is_empty());
    assert!(matches!(listener_rx.try_recv(), Ok(SseFrame::Json { .. })));
    assert!(matches!(
        listener_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

#[test]
fn dispatch_to_get_listeners_broadcasts_each_message_to_all_streams() {
    let mut get_listeners = Vec::new();
    let (first_tx, mut first_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
    let (second_tx, mut second_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
    get_listeners.push(first_tx);
    get_listeners.push(second_tx);

    let frame =
        SseFrame::json(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1"}}"#.to_string());
    let outcome = dispatch_to_get_listeners(&frame, &mut get_listeners);

    assert!(matches!(outcome, ListenerDispatch::Delivered));
    assert!(matches!(first_rx.try_recv(), Ok(SseFrame::Json { .. })));
    assert!(matches!(second_rx.try_recv(), Ok(SseFrame::Json { .. })));
}

#[test]
fn dispatch_to_get_listeners_removes_closed_listener() {
    let mut get_listeners = Vec::new();
    let (listener_tx, listener_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
    drop(listener_rx);
    get_listeners.push(listener_tx);

    let frame =
        SseFrame::json(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1"}}"#.to_string());
    let outcome = dispatch_to_get_listeners(&frame, &mut get_listeners);

    assert!(matches!(outcome, ListenerDispatch::Dropped));
    assert!(get_listeners.is_empty());
}

#[test]
fn inject_connection_id_into_initialize_result_preserves_existing_payload() {
    let connection_id = AcpConnectionId::default();
    let updated = inject_connection_id_into_initialize_result(
        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":0}}"#,
        &connection_id,
    )
    .unwrap();

    let json: Value = serde_json::from_str(&updated).unwrap();
    assert_eq!(json["result"]["protocolVersion"], 0);
    assert_eq!(json["result"]["connectionId"], connection_id.to_string());
}

#[test]
fn inject_session_id_into_response_result_adds_session_id_to_null_result() {
    let session_id = session_id();
    let updated =
        inject_session_id_into_response_result(r#"{"jsonrpc":"2.0","id":2,"result":null}"#, &session_id).unwrap();

    let json: Value = serde_json::from_str(&updated).unwrap();
    assert_eq!(json["result"]["sessionId"], session_id.as_str());
}

#[test]
fn inject_session_id_into_response_result_or_preserve_keeps_original_on_error() {
    let connection_id = AcpConnectionId::default();
    let session_id = session_id();
    let outbound = "{".to_string();

    let updated = inject_session_id_into_response_result_or_preserve(outbound.clone(), &session_id, &connection_id);

    assert_eq!(updated, outbound);
}

#[test]
fn undelivered_response_outcome_only_flags_missing_or_dropped_responses() {
    let response = OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","id":2,"result":null}"#).unwrap();
    let notification =
        OutgoingHttpMessage::parse(r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session-1"}}"#)
            .unwrap();

    assert_eq!(
        undelivered_response_outcome(Some(&response), ListenerDispatch::Missing),
        Some(ListenerDispatch::Missing)
    );
    assert_eq!(
        undelivered_response_outcome(Some(&response), ListenerDispatch::Dropped),
        Some(ListenerDispatch::Dropped)
    );
    assert_eq!(
        undelivered_response_outcome(Some(&response), ListenerDispatch::Delivered),
        None
    );
    assert_eq!(
        undelivered_response_outcome(Some(&notification), ListenerDispatch::Missing),
        None
    );
    assert_eq!(undelivered_response_outcome(None, ListenerDispatch::Missing), None);
}

#[test]
fn header_validators_enforce_content_negotiation() {
    let valid_post = post_headers();
    assert!(validate_post_headers(&valid_post).is_ok());

    let mut valid_post_with_charset = post_headers();
    valid_post_with_charset.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    assert!(validate_post_headers(&valid_post_with_charset).is_ok());

    let mut bad_content_type = valid_post.clone();
    bad_content_type.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    assert!(matches!(
        validate_post_headers(&bad_content_type),
        Err(HttpTransportError::UnsupportedMediaType {
            message: "Content-Type must be application/json",
            source: None,
        })
    ));

    let valid_get = get_headers();
    assert!(validate_get_headers(&valid_get).is_ok());

    let mut valid_post_with_accept = HeaderMap::new();
    valid_post_with_accept.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    valid_post_with_accept.insert(
        ACCEPT,
        HeaderValue::from_static("application/json;q=0.9, text/event-stream"),
    );
    assert!(validate_post_headers(&valid_post_with_accept).is_ok());

    let mut valid_get_with_q = HeaderMap::new();
    valid_get_with_q.insert(ACCEPT, HeaderValue::from_static("text/event-stream; q=0.5"));
    assert!(validate_get_headers(&valid_get_with_q).is_ok());

    let mut invalid_get = HeaderMap::new();
    invalid_get.insert(ACCEPT, HeaderValue::from_static("application/json"));
    assert!(matches!(
        validate_get_headers(&invalid_get),
        Err(HttpTransportError::NotAcceptable {
            message: "Accept must include text/event-stream",
            source: None,
        })
    ));
}

#[test]
fn validate_origin_allows_loopback_and_matching_hosts() {
    let mut loopback = HeaderMap::new();
    loopback.insert(ORIGIN, HeaderValue::from_static("http://localhost:3000"));
    assert!(validate_origin(&loopback, IpAddr::V4(Ipv4Addr::LOCALHOST)).is_ok());

    let mut proxied = HeaderMap::new();
    proxied.insert(ORIGIN, HeaderValue::from_static("https://example.com"));
    proxied.insert(HOST, HeaderValue::from_static("example.com"));
    assert!(validate_origin(&proxied, IpAddr::V4(Ipv4Addr::UNSPECIFIED)).is_ok());
}

#[test]
fn validate_origin_rejects_invalid_and_remote_origins() {
    let mut invalid = HeaderMap::new();
    invalid.insert(ORIGIN, HeaderValue::from_static("not a uri"));
    assert!(matches!(
        validate_origin(&invalid, IpAddr::V4(Ipv4Addr::LOCALHOST)),
        Err(HttpTransportError::Forbidden {
            message: "invalid Origin header",
            source: Some(_),
        })
    ));

    let mut remote = HeaderMap::new();
    remote.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));
    assert!(matches!(
        validate_origin(&remote, IpAddr::V4(Ipv4Addr::LOCALHOST)),
        Err(HttpTransportError::Forbidden {
            message: "Origin is not allowed",
            source: None,
        })
    ));
}

#[test]
fn validate_http_context_enforces_connection_and_session_rules() {
    let initialize = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.to_string(),
    )
    .unwrap();
    let connection_id = AcpConnectionId::default();
    let session_id = session_id();

    assert!(validate_http_context(&initialize, None, None).is_ok());
    assert!(matches!(
        validate_http_context(&initialize, Some(&connection_id), None),
        Err(HttpTransportError::BadRequest {
            message: "initialize must not include Acp-Connection-Id",
            source: None,
        })
    ));

    let initialized = IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string()).unwrap();
    assert!(matches!(
        validate_http_context(&initialized, None, None),
        Err(HttpTransportError::BadRequest {
            message: "missing Acp-Connection-Id header",
            source: None,
        })
    ));

    let prompt = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"session-1"}}"#.to_string(),
    )
    .unwrap();
    assert!(matches!(
        validate_http_context(&prompt, Some(&connection_id), None),
        Err(HttpTransportError::BadRequest {
            message: "missing Acp-Session-Id header",
            source: None,
        })
    ));

    let mismatched = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"session-2"}}"#.to_string(),
    )
    .unwrap();
    assert!(matches!(
        validate_http_context(&mismatched, Some(&connection_id), Some(&session_id)),
        Err(HttpTransportError::BadRequest {
            message: "Acp-Session-Id header does not match request body sessionId",
            source: None,
        })
    ));

    let load = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"session-1","cwd":"."}}"#.to_string(),
    )
    .unwrap();
    assert!(matches!(
        validate_http_context(&load, Some(&connection_id), None),
        Err(HttpTransportError::BadRequest {
            message: "missing Acp-Session-Id header",
            source: None,
        })
    ));
    assert!(validate_http_context(&load, Some(&connection_id), Some(&session_id)).is_ok());

    let resume = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/resume","params":{"sessionId":"session-1","cwd":"."}}"#
            .to_string(),
    )
    .unwrap();
    assert!(matches!(
        validate_http_context(&resume, Some(&connection_id), None),
        Err(HttpTransportError::BadRequest {
            message: "missing Acp-Session-Id header",
            source: None,
        })
    ));
    assert!(validate_http_context(&resume, Some(&connection_id), Some(&session_id)).is_ok());

    let fork = IncomingHttpMessage::parse(
        r#"{"jsonrpc":"2.0","id":2,"method":"session/fork","params":{"sessionId":"session-1","cwd":"."}}"#.to_string(),
    )
    .unwrap();
    assert!(matches!(
        validate_http_context(&fork, Some(&connection_id), None),
        Err(HttpTransportError::BadRequest {
            message: "missing Acp-Session-Id header",
            source: None,
        })
    ));
    assert!(validate_http_context(&fork, Some(&connection_id), Some(&session_id)).is_ok());
}

#[test]
fn validate_protocol_version_header_allows_missing_and_rejects_mismatches() {
    assert!(validate_protocol_version_header(None, None).is_ok());
    assert!(validate_protocol_version_header(Some(&ProtocolVersion::V0), None).is_ok());
    assert!(validate_protocol_version_header(None, Some(&ProtocolVersion::V0)).is_ok());
    assert!(validate_protocol_version_header(Some(&ProtocolVersion::V0), Some(&ProtocolVersion::V0)).is_ok());

    assert!(matches!(
        validate_protocol_version_header(Some(&ProtocolVersion::V1), Some(&ProtocolVersion::V0)),
        Err(HttpTransportError::BadRequest {
            message: "Acp-Protocol-Version header does not match initialized protocol version",
            source: None,
        })
    ));
}

#[test]
fn header_parsers_and_websocket_detection_handle_valid_and_invalid_values() {
    let connection_id = AcpConnectionId::default();
    let session_id = session_id();
    let mut headers = HeaderMap::new();
    headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string()).unwrap(),
    );
    headers.insert(
        ACP_SESSION_ID_HEADER,
        HeaderValue::from_str(session_id.as_str()).unwrap(),
    );
    headers.insert(ACP_PROTOCOL_VERSION_HEADER, HeaderValue::from_static("0"));
    headers.insert("upgrade", HeaderValue::from_static("websocket"));

    assert_eq!(
        parse_connection_id_header(&headers).unwrap(),
        Some(connection_id.clone())
    );
    assert_eq!(parse_session_id_header(&headers).unwrap(), Some(session_id));
    assert_eq!(
        parse_protocol_version_header(&headers).unwrap(),
        Some(ProtocolVersion::V0)
    );
    assert!(is_websocket_request(&headers));

    headers.insert(ACP_CONNECTION_ID_HEADER, HeaderValue::from_static("not-a-uuid"));
    assert!(matches!(
        parse_connection_id_header(&headers),
        Err(HttpTransportError::BadRequest {
            message: "invalid Acp-Connection-Id header",
            source: Some(_),
        })
    ));

    headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string()).unwrap(),
    );
    headers.insert(ACP_PROTOCOL_VERSION_HEADER, HeaderValue::from_static("not-a-version"));
    assert!(matches!(
        parse_protocol_version_header(&headers),
        Err(HttpTransportError::BadRequest {
            message: "invalid Acp-Protocol-Version header",
            source: Some(_),
        })
    ));
}

#[tokio::test]
async fn http_post_returns_accepted_for_notifications() {
    let (state, mut manager_rx) = test_state();
    let connection_id = AcpConnectionId::default();
    let expected_connection_id = connection_id.clone();

    tokio::spawn(async move {
        match manager_rx.recv().await.unwrap() {
            ManagerRequest::HttpPost {
                connection_id: Some(actual_connection_id),
                session_id: None,
                message,
                response,
                ..
            } => {
                assert_eq!(actual_connection_id, expected_connection_id);
                assert!(message.is_notification());
                let _ = response.send(Ok(HttpPostOutcome::Accepted));
            }
            // TODO(coverage): defensive test guard; sweep when refreshing baseline.
            _ => panic!("unexpected manager request"),
        }
    });

    let mut headers = post_headers();
    headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string()).unwrap(),
    );

    let response = http_post(
        headers,
        state,
        r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn http_post_accepts_null_result_responses() {
    let (state, mut manager_rx) = test_state();
    let connection_id = AcpConnectionId::default();
    let session_id = session_id();
    let expected_connection_id = connection_id.clone();
    let expected_session_id = session_id.clone();

    tokio::spawn(async move {
        match manager_rx.recv().await.unwrap() {
            ManagerRequest::HttpPost {
                connection_id: Some(actual_connection_id),
                session_id: Some(actual_session_id),
                message,
                response,
                ..
            } => {
                assert_eq!(actual_connection_id, expected_connection_id);
                assert_eq!(actual_session_id, expected_session_id);
                assert!(message.is_response());
                let _ = response.send(Ok(HttpPostOutcome::Accepted));
            }
            // TODO(coverage): defensive test guard; sweep when refreshing baseline.
            _ => panic!("unexpected manager request"),
        }
    });

    let mut headers = post_headers();
    headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string()).unwrap(),
    );
    headers.insert(
        ACP_SESSION_ID_HEADER,
        HeaderValue::from_str(session_id.as_str()).unwrap(),
    );

    let response = http_post(headers, state, r#"{"jsonrpc":"2.0","id":1,"result":null}"#.to_string())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn http_post_returns_json_initialize_response_with_connection_header() {
    let (state, mut manager_rx) = test_state();
    let connection_id = AcpConnectionId::default();
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "connectionId": connection_id.to_string(),
            "protocolVersion": 0
        }
    });
    let expected_body = body.clone();

    let expected_connection_id = connection_id.clone();
    tokio::spawn(async move {
        match manager_rx.recv().await.unwrap() {
            ManagerRequest::HttpPost {
                connection_id: None,
                session_id: None,
                message,
                response,
                ..
            } => {
                assert!(message.is_initialize());
                let _ = response.send(Ok(HttpPostOutcome::Json {
                    connection_id: expected_connection_id,
                    protocol_version: Some(ProtocolVersion::V0),
                    body: body.to_string(),
                }));
            }
            // TODO(coverage): defensive test guard; sweep when refreshing baseline.
            _ => panic!("unexpected manager request"),
        }
    });

    let response = http_post(
        post_headers(),
        state,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.to_string(),
    )
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(ACP_CONNECTION_ID_HEADER).unwrap(),
        HeaderValue::from_str(&connection_id.to_string()).unwrap()
    );
    assert_eq!(
        response.headers().get(ACP_PROTOCOL_VERSION_HEADER).unwrap(),
        HeaderValue::from_static("0")
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected_body);
}

#[tokio::test]
async fn http_get_and_delete_round_trip_through_manager() {
    let (state, mut manager_rx) = test_state();
    let connection_id = AcpConnectionId::default();
    let expected_connection_id = connection_id.clone();

    tokio::spawn(async move {
        match manager_rx.recv().await.unwrap() {
            ManagerRequest::HttpGet {
                connection_id: actual_connection_id,
                protocol_version,
                response,
            } => {
                assert_eq!(actual_connection_id, expected_connection_id.clone());
                assert_eq!(protocol_version, None);
                let (stream_tx, stream_rx) = mpsc::channel(HTTP_CHANNEL_CAPACITY);
                let _ = stream_tx.try_send(SseFrame::json(
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": { "sessionId": "session-1" }
                    })
                    .to_string(),
                ));
                drop(stream_tx);
                let _ = response.send(Ok(HttpGetOutcome {
                    protocol_version: Some(ProtocolVersion::V0),
                    stream: stream_rx,
                }));
            }
            // TODO(coverage): defensive test guard; sweep when refreshing baseline.
            _ => panic!("unexpected manager request"),
        }
        match manager_rx.recv().await.unwrap() {
            ManagerRequest::HttpDelete {
                connection_id: actual_connection_id,
                protocol_version,
                response,
            } => {
                assert_eq!(actual_connection_id, expected_connection_id);
                assert_eq!(protocol_version, None);
                let _ = response.send(Ok(Some(ProtocolVersion::V0)));
            }
            // TODO(coverage): defensive test guard; sweep when refreshing baseline.
            _ => panic!("unexpected manager request"),
        }
    });

    let mut headers = get_headers();
    headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string()).unwrap(),
    );

    let response = http_get(headers, state.clone()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(ACP_CONNECTION_ID_HEADER).unwrap(),
        HeaderValue::from_str(&connection_id.to_string()).unwrap()
    );
    assert_eq!(
        response.headers().get(ACP_PROTOCOL_VERSION_HEADER).unwrap(),
        HeaderValue::from_static("0")
    );
    assert_eq!(
        response.headers().get(X_ACCEL_BUFFERING_HEADER).unwrap(),
        HeaderValue::from_static("no")
    );
    assert_eq!(
        json_event_body(response).await,
        vec![json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": "session-1" }
        })]
    );

    let mut delete_headers = HeaderMap::new();
    delete_headers.insert(
        ACP_CONNECTION_ID_HEADER,
        HeaderValue::from_str(&connection_id.to_string()).unwrap(),
    );

    let response = http_delete(delete_headers, state).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response.headers().get(ACP_PROTOCOL_VERSION_HEADER).unwrap(),
        HeaderValue::from_static("0")
    );
}

#[tokio::test]
async fn get_rejects_incomplete_websocket_upgrade() {
    let (state, _manager_rx) = test_state();
    let request = HttpRequest::builder()
        .method("GET")
        .uri("/acp")
        .header("upgrade", "websocket")
        .body(Body::empty())
        .unwrap();

    let response = get(State(state), request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_rejects_disallowed_origin_before_processing() {
    let (state, _manager_rx) = test_state();
    let mut headers = post_headers();
    headers.insert(ORIGIN, HeaderValue::from_static("https://evil.example"));

    let response = post(
        headers,
        State(state),
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.to_string(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), "Origin is not allowed");
}

#[tokio::test]
async fn get_rejects_websocket_upgrade_with_disallowed_origin() {
    let (state, _manager_rx) = test_state();
    let request = HttpRequest::builder()
        .method("GET")
        .uri("/acp")
        .header("upgrade", "websocket")
        .header(ORIGIN, "https://evil.example")
        .body(Body::empty())
        .unwrap();

    let response = get(State(state), request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn process_manager_request_rejects_invalid_or_unknown_http_targets() {
    let nats_client = AdvancedMockNatsClient::new();
    let js_client = MockJs::new();
    let config = test_config();
    let mut http_connections = HashMap::new();
    let mut websocket_handles = Vec::new();
    let mut http_connection_handles = Vec::new();
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    let (post_response_tx, post_response_rx) = oneshot::channel();
    let post_message = IncomingHttpMessage::parse(r#"{"jsonrpc":"2.0","method":"initialized"}"#.to_string()).unwrap();
    process_manager_request(
        ManagerRequest::HttpPost {
            connection_id: None,
            protocol_version: None,
            session_id: None,
            message: post_message,
            response: post_response_tx,
            shutdown_rx: shutdown_rx.clone(),
        },
        &mut http_connections,
        &mut websocket_handles,
        &mut http_connection_handles,
        &nats_client,
        &js_client,
        &config,
    )
    .await;
    assert!(matches!(
        post_response_rx.await.unwrap(),
        Err(HttpTransportError::BadRequest {
            message: "missing Acp-Connection-Id header",
            source: None,
        })
    ));

    let unknown_connection_id = AcpConnectionId::default();
    let (get_response_tx, get_response_rx) = oneshot::channel();
    process_manager_request(
        ManagerRequest::HttpGet {
            connection_id: unknown_connection_id.clone(),
            protocol_version: None,
            response: get_response_tx,
        },
        &mut http_connections,
        &mut websocket_handles,
        &mut http_connection_handles,
        &nats_client,
        &js_client,
        &config,
    )
    .await;
    assert!(matches!(
        get_response_rx.await.unwrap(),
        Err(HttpTransportError::NotFound {
            message: "unknown ACP connection",
            source: None,
        })
    ));

    let (delete_response_tx, delete_response_rx) = oneshot::channel();
    process_manager_request(
        ManagerRequest::HttpDelete {
            connection_id: unknown_connection_id,
            protocol_version: None,
            response: delete_response_tx,
        },
        &mut http_connections,
        &mut websocket_handles,
        &mut http_connection_handles,
        &nats_client,
        &js_client,
        &config,
    )
    .await;
    assert!(matches!(
        delete_response_rx.await.unwrap(),
        Err(HttpTransportError::NotFound {
            message: "unknown ACP connection",
            source: None,
        })
    ));
}

#[tokio::test]
async fn process_manager_request_prunes_closed_http_connections() {
    let nats_client = AdvancedMockNatsClient::new();
    let js_client = MockJs::new();
    let config = test_config();
    let mut http_connections = HashMap::new();
    let mut websocket_handles = Vec::new();
    let mut http_connection_handles = Vec::new();

    let stale_connection_id = AcpConnectionId::default();
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    drop(command_rx);
    http_connections.insert(stale_connection_id, HttpConnectionHandle { command_tx });

    let unknown_connection_id = AcpConnectionId::default();
    let (response_tx, response_rx) = oneshot::channel();
    process_manager_request(
        ManagerRequest::HttpGet {
            connection_id: unknown_connection_id,
            protocol_version: None,
            response: response_tx,
        },
        &mut http_connections,
        &mut websocket_handles,
        &mut http_connection_handles,
        &nats_client,
        &js_client,
        &config,
    )
    .await;

    assert!(http_connections.is_empty());
    assert!(matches!(
        response_rx.await.unwrap(),
        Err(HttpTransportError::NotFound {
            message: "unknown ACP connection",
            source: None,
        })
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn process_manager_request_tracks_http_connection_tasks() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let nats_client = AdvancedMockNatsClient::new();
            let _injector = nats_client.inject_messages();
            nats_client.hang_next_request();

            let js_client = MockJs::new();
            let config = test_config();
            let mut http_connections = HashMap::new();
            let mut websocket_handles = Vec::new();
            let mut http_connection_handles = Vec::new();
            let (shutdown_tx, shutdown_rx) = watch::channel(false);

            let (response_tx, mut response_rx) = oneshot::channel();
            let initialize = IncomingHttpMessage::parse(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#.to_string(),
            )
            .unwrap();

            process_manager_request(
                ManagerRequest::HttpPost {
                    connection_id: None,
                    protocol_version: None,
                    session_id: None,
                    message: initialize,
                    response: response_tx,
                    shutdown_rx,
                },
                &mut http_connections,
                &mut websocket_handles,
                &mut http_connection_handles,
                &nats_client,
                &js_client,
                &config,
            )
            .await;

            assert_eq!(http_connections.len(), 1);
            assert_eq!(http_connection_handles.len(), 1);
            assert!(!http_connection_handles[0].is_finished());
            assert!(matches!(
                response_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));

            let _ = shutdown_tx.send(true);
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while !http_connection_handles[0].is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("HTTP connection task did not finish after shutdown");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn http_connection_logs_response_without_get_listener() {
    let local = tokio::task::LocalSet::new();
    local
            .run_until(async {
                let nats_client = AdvancedMockNatsClient::new();
                let _injector = nats_client.inject_messages();
                nats_client.set_response(
                    "acp.agent.initialize",
                    bytes::Bytes::from_static(
                        br#"{"agentCapabilities":{"loadSession":false,"mcpCapabilities":{"http":false,"sse":false},"promptCapabilities":{"audio":false,"embeddedContext":false,"image":false},"sessionCapabilities":{}},"authMethods":[],"protocolVersion":0}"#,
                    ),
                );
                nats_client.set_response("acp.agent.authenticate", bytes::Bytes::from_static(b"{}"));

                let js_client = MockJs::new();
                let config = test_config();
                let connection_id = AcpConnectionId::default();
                let (command_tx, command_rx) = mpsc::unbounded_channel();
                let (shutdown_tx, shutdown_rx) = watch::channel(false);
                let task = tokio::task::spawn_local(run_http_connection(
                    connection_id.clone(),
                    nats_client,
                    js_client,
                    config,
                    command_rx,
                    shutdown_rx,
                ));

                let (initialize_tx, initialize_rx) = oneshot::channel();
                command_tx
                    .send(HttpConnectionCommand::Post {
                        protocol_version: None,
                        session_id: None,
                        message: IncomingHttpMessage::parse(
                            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":0}}"#
                                .to_string(),
                        )
                        .unwrap(),
                        response: initialize_tx,
                    })
                    .unwrap();
                assert!(matches!(
                    initialize_rx.await.unwrap().unwrap(),
                    HttpPostOutcome::Json {
                        protocol_version: Some(ProtocolVersion::V0),
                        ..
                    }
                ));

                let (authenticate_tx, authenticate_rx) = oneshot::channel();
                command_tx
                    .send(HttpConnectionCommand::Post {
                        protocol_version: Some(ProtocolVersion::V0),
                        session_id: None,
                        message: IncomingHttpMessage::parse(
                            r#"{"jsonrpc":"2.0","id":2,"method":"authenticate","params":{"methodId":"api-key"}}"#
                                .to_string(),
                        )
                        .unwrap(),
                        response: authenticate_tx,
                    })
                    .unwrap();
                assert!(matches!(
                    authenticate_rx.await.unwrap().unwrap(),
                    HttpPostOutcome::Accepted
                ));

                for _ in 0..20 {
                    tokio::task::yield_now().await;
                }

                let (close_tx, close_rx) = oneshot::channel();
                command_tx
                    .send(HttpConnectionCommand::Close {
                        protocol_version: Some(ProtocolVersion::V0),
                        response: close_tx,
                    })
                    .unwrap();
                assert_eq!(close_rx.await.unwrap().unwrap(), Some(ProtocolVersion::V0));

                let _ = shutdown_tx.send(true);
                tokio::time::timeout(std::time::Duration::from_secs(2), task)
                    .await
                    .expect("HTTP connection task did not finish")
                    .unwrap();
            })
            .await;
}

#[tokio::test]
async fn fail_pending_initialize_on_close_sends_internal_error() {
    let (response_tx, response_rx) = oneshot::channel();
    let mut pending_request = Some(PendingRequest::Initialize {
        request_id: RequestId::Number(1),
        response: response_tx,
    });

    fail_pending_initialize_on_close(&mut pending_request);

    assert!(pending_request.is_none());
    assert!(matches!(
        response_rx.await.unwrap(),
        Err(HttpTransportError::Internal {
            message: "HTTP connection closed before the request completed",
            source: None,
        })
    ));
}
