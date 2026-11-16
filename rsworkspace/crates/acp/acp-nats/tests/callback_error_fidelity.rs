use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::{
    AcpPrefix, Bridge, ClientHandler, Config, FlushClient, NatsConfig, PublishClient, RequestClient, SubscribeClient,
};
use agent_client_protocol::v1::schema::{
    CreateElicitationRequest, CreateElicitationResponse, CreateTerminalRequest, CreateTerminalResponse, ExtRequest,
    ExtResponse, KillTerminalRequest, KillTerminalResponse, ReadTextFileRequest, ReadTextFileResponse,
    ReleaseTerminalRequest, ReleaseTerminalResponse, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, TerminalOutputRequest, TerminalOutputResponse, WaitForTerminalExitRequest,
    WaitForTerminalExitResponse, WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::{Error, ErrorCode, Result};
use async_nats::subject::ToSubject;
use async_nats::{HeaderMap, Message};
use bytes::Bytes;
use futures::StreamExt;
use futures::channel::mpsc;
use jsonrpc_nats::{Direction, RequestId};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::time::timeout;
use trogon_std::time::SystemClock;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone)]
struct CallbackTransport {
    incoming: Arc<Mutex<Option<Message>>>,
    replies: mpsc::UnboundedSender<Message>,
    flushed: Arc<Notify>,
}

impl SubscribeClient for CallbackTransport {
    type SubscribeError = std::io::Error;
    type Subscription = futures::stream::Iter<std::option::IntoIter<Message>>;

    async fn subscribe<S: ToSubject + Send>(
        &self,
        subject: S,
    ) -> std::result::Result<Self::Subscription, Self::SubscribeError> {
        assert_eq!(subject.to_subject().as_str(), "callback_errors.v1.session.*.client.>");
        let message = self
            .incoming
            .lock()
            .map_err(|_| std::io::Error::other("incoming lock poisoned"))?
            .take();
        Ok(futures::stream::iter(message))
    }
}

impl RequestClient for CallbackTransport {
    type RequestError = std::io::Error;

    async fn request_with_headers<S: ToSubject + Send>(
        &self,
        _subject: S,
        _headers: HeaderMap,
        _payload: Bytes,
    ) -> std::result::Result<Message, Self::RequestError> {
        Err(std::io::Error::other(
            "callback dispatch must not issue a broker request",
        ))
    }
}

impl PublishClient for CallbackTransport {
    type PublishError = std::io::Error;

    async fn publish_with_headers<S: ToSubject + Send>(
        &self,
        subject: S,
        headers: HeaderMap,
        payload: Bytes,
    ) -> std::result::Result<(), Self::PublishError> {
        self.replies
            .unbounded_send(message(subject.to_subject().as_str(), None, headers, payload))
            .map_err(|_| std::io::Error::other("reply receiver closed"))
    }
}

impl FlushClient for CallbackTransport {
    type FlushError = std::io::Error;

    async fn flush(&self) -> std::result::Result<(), Self::FlushError> {
        self.flushed.notify_one();
        Ok(())
    }
}

fn message(subject: &str, reply: Option<&str>, headers: HeaderMap, payload: Bytes) -> Message {
    Message {
        subject: subject.to_owned().into(),
        reply: reply.map(|value| value.to_owned().into()),
        length: payload.len(),
        payload,
        headers: Some(headers),
        status: None,
        description: None,
    }
}

struct RejectingClient {
    error: Error,
    calls: AtomicUsize,
}

impl RejectingClient {
    fn reject<T>(&self) -> Result<T> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Err(self.error.clone())
    }
}

#[async_trait::async_trait]
impl ClientHandler for RejectingClient {
    async fn session_notification(&self, _args: SessionNotification) -> Result<()> {
        self.reject()
    }

    async fn request_permission(&self, _args: RequestPermissionRequest) -> Result<RequestPermissionResponse> {
        self.reject()
    }

    async fn read_text_file(&self, _args: ReadTextFileRequest) -> Result<ReadTextFileResponse> {
        self.reject()
    }

    async fn write_text_file(&self, _args: WriteTextFileRequest) -> Result<WriteTextFileResponse> {
        self.reject()
    }

    async fn create_terminal(&self, _args: CreateTerminalRequest) -> Result<CreateTerminalResponse> {
        self.reject()
    }

    async fn terminal_output(&self, _args: TerminalOutputRequest) -> Result<TerminalOutputResponse> {
        self.reject()
    }

    async fn release_terminal(&self, _args: ReleaseTerminalRequest) -> Result<ReleaseTerminalResponse> {
        self.reject()
    }

    async fn wait_for_terminal_exit(&self, _args: WaitForTerminalExitRequest) -> Result<WaitForTerminalExitResponse> {
        self.reject()
    }

    async fn kill_terminal(&self, _args: KillTerminalRequest) -> Result<KillTerminalResponse> {
        self.reject()
    }

    async fn elicitation_create(&self, _args: CreateElicitationRequest) -> Result<CreateElicitationResponse> {
        self.reject()
    }

    async fn ext_method(&self, _args: ExtRequest) -> Result<ExtResponse> {
        self.reject()
    }
}

fn requests() -> [(&'static str, &'static str, Value); 10] {
    [
        (
            "elicitation/create",
            "elicitation.create",
            json!({
                "mode": "form", "sessionId": "session-1", "message": "Select a configuration",
                "requestedSchema": {"type": "object", "properties": {}}
            }),
        ),
        (
            "session/request_permission",
            "session.request_permission",
            json!({
                "sessionId": "session-1", "toolCall": {"toolCallId": "call-1"}, "options": []
            }),
        ),
        (
            "fs/read_text_file",
            "fs.read_text_file",
            json!({"sessionId": "session-1", "path": "/workspace/config.txt"}),
        ),
        (
            "fs/write_text_file",
            "fs.write_text_file",
            json!({
                "sessionId": "session-1", "path": "/workspace/config.txt", "content": "enabled = true"
            }),
        ),
        (
            "terminal/create",
            "terminal.create",
            json!({"sessionId": "session-1", "command": "printf", "args": ["ready"]}),
        ),
        (
            "terminal/output",
            "terminal.output",
            json!({"sessionId": "session-1", "terminalId": "terminal-1"}),
        ),
        (
            "terminal/release",
            "terminal.release",
            json!({"sessionId": "session-1", "terminalId": "terminal-1"}),
        ),
        (
            "terminal/wait_for_exit",
            "terminal.wait_for_exit",
            json!({"sessionId": "session-1", "terminalId": "terminal-1"}),
        ),
        (
            "terminal/kill",
            "terminal.kill",
            json!({"sessionId": "session-1", "terminalId": "terminal-1"}),
        ),
        (
            "_editor_selection",
            "ext.editor_selection",
            json!({"document": "config.txt"}),
        ),
    ]
}

async fn dispatch(suffix: &str, headers: HeaderMap, payload: Bytes, peer_error: Error) -> TestResult<(Message, usize)> {
    let (replies, mut receiver) = mpsc::unbounded();
    let nats = CallbackTransport {
        incoming: Arc::new(Mutex::new(Some(message(
            &format!("callback_errors.v1.session.session-1.client.{suffix}"),
            Some("_INBOX.callback-errors"),
            headers,
            payload,
        )))),
        replies,
        flushed: Arc::new(Notify::new()),
    };
    let client = Arc::new(RejectingClient {
        error: peer_error,
        calls: AtomicUsize::new(0),
    });
    let bridge = Arc::new(Bridge::new(
        nats.clone(),
        (),
        SystemClock,
        &opentelemetry::global::meter("callback-error-contract"),
        Config::new(
            AcpPrefix::new("callback_errors")?,
            NatsConfig::from_url("nats://127.0.0.1:4222"),
        ),
    ));
    acp_nats::client::run(nats.clone(), client.clone(), bridge).await;
    timeout(Duration::from_secs(5), nats.flushed.notified()).await?;
    let reply = timeout(Duration::from_secs(5), receiver.next())
        .await?
        .ok_or_else(|| std::io::Error::other("callback did not publish a reply"))?;
    assert_eq!(reply.subject.as_str(), "_INBOX.callback-errors");
    assert!(receiver.try_recv().is_err(), "callback must publish exactly one reply");
    Ok((reply, client.calls.load(Ordering::Relaxed)))
}

#[tokio::test]
async fn callback_errors_preserve_code_message_data_and_wire_ids() -> TestResult {
    let mut lost_data = Vec::new();
    for (method, suffix, params) in requests() {
        for id in [RequestId::Number(42), RequestId::String("request-\n\"é".into())] {
            for data in [
                None,
                Some(json!({"retryAfterMs": 250, "details": {"retryable": true, "values": [null, 42, "text"]}})),
            ] {
                let encoded = acp_nats::wire::encode_request(method, id.clone(), &params)?;
                let mut peer = Error::new(-32050, "Client temporarily unavailable\n\"retry\"");
                peer.data = data.clone();
                let (reply, calls) = dispatch(suffix, encoded.headers, encoded.body, peer.clone()).await?;
                assert_eq!(calls, 1, "{method} must reach the client handler");
                let headers = reply.headers.as_ref().ok_or("callback reply has no headers")?;
                let body: Value = serde_json::from_slice(&reply.payload)?;
                assert_eq!(
                    jsonrpc_nats::decode_value(Direction::Response, None, headers, &reply.payload)?,
                    body
                );
                assert_eq!(body["id"], id.to_json(), "{method} response ID");
                let projected_id = id.to_json().to_string();
                assert_eq!(
                    headers.get(jsonrpc_nats::HEADER_ID).map(|v| v.as_str()),
                    Some(projected_id.as_str())
                );
                assert_eq!(
                    headers.get(jsonrpc_nats::HEADER_ERROR_CODE).map(|v| v.as_str()),
                    Some("-32050")
                );
                assert_eq!(body["error"]["code"], i32::from(peer.code), "{method} error code");
                assert_eq!(body["error"]["message"], peer.message, "{method} error message");
                if body["error"].get("data") != data.as_ref() {
                    lost_data.push((method, id.clone(), data.clone(), body["error"].get("data").cloned()));
                }
            }
        }
    }
    assert!(lost_data.is_empty(), "callback error data changed: {lost_data:?}");
    Ok(())
}

#[tokio::test]
async fn peer_invalid_params_errors_retain_their_structured_data() -> TestResult {
    let data = json!({"reason": "unsupported operation", "supported": ["read", "inspect"]});
    let mut lost_data = Vec::new();
    for (method, suffix, params) in requests() {
        let encoded = acp_nats::wire::encode_request(method, RequestId::String("peer-invalid-params".into()), &params)?;
        let peer = Error::new(ErrorCode::INVALID_PARAMS, "The client refused these parameters").data(data.clone());
        let (reply, calls) = dispatch(suffix, encoded.headers, encoded.body, peer.clone()).await?;
        assert_eq!(calls, 1, "{method} must reach the peer");
        let headers = reply.headers.as_ref().ok_or("callback reply has no headers")?;
        let body = jsonrpc_nats::decode_value(Direction::Response, None, headers, &reply.payload)?;
        assert_eq!(body["id"], "peer-invalid-params");
        assert_eq!(body["error"]["code"], i32::from(ErrorCode::INVALID_PARAMS));
        assert_eq!(body["error"]["message"], peer.message);
        if body["error"].get("data") != Some(&data) {
            lost_data.push(method);
        }
    }
    assert!(
        lost_data.is_empty(),
        "peer invalid-params errors lost data: {lost_data:?}"
    );
    Ok(())
}

#[tokio::test]
async fn malformed_callbacks_remain_local_errors_without_peer_data() -> TestResult {
    for (method, suffix, params) in requests() {
        let encoded = acp_nats::wire::encode_request(method, RequestId::Number(7), &params)?;
        let peer = Error::new(-32050, "must not reach client").data(json!({"peer": true}));
        let (reply, calls) = dispatch(suffix, encoded.headers, Bytes::from_static(b"{"), peer).await?;
        let headers = reply.headers.as_ref().ok_or("callback reply has no headers")?;
        let body = jsonrpc_nats::decode_value(Direction::Response, None, headers, &reply.payload)?;
        assert_eq!(calls, 0, "{method} malformed request must not reach peer");
        assert_eq!(body["id"], 7);
        let expected = if method.starts_with('_') {
            ErrorCode::PARSE_ERROR
        } else {
            ErrorCode::INVALID_PARAMS
        };
        assert_eq!(
            body["error"]["code"],
            i32::from(expected),
            "{method} local error category"
        );
        assert!(
            body["error"].get("data").is_none(),
            "{method} local error acquired peer data"
        );
    }
    Ok(())
}
