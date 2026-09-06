use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use acp_nats::nats::{AcpStream, commands, global, responses};
use acp_nats::{AcpPrefix, AcpSessionId, NatsConfig};
use agent_client_protocol::schema::v1::{CancelNotification, ContentBlock, NewSessionRequest, PromptRequest};
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_channel::{
    AgentId, AgentPort, AgentPortError, AgentSessionId, ConversationRecord, Endpoint, InboundEvent, MessageRef,
    PlatformUserId, PrincipalId, PromptOutcome, ReleaseReason, ReleaseStep, Sender,
};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::env::InMemoryEnv;

use super::super::{AcpBridge, AcpPort, AcpPortError, SessionMethods};

struct Fixture {
    _server: JetStreamTestServer,
    client: async_nats::Client,
    js: async_nats::jetstream::Context,
    prefix: AcpPrefix,
    bridge: Arc<AcpBridge>,
    port: AcpPort,
    cwd: PathBuf,
}

impl Fixture {
    async fn new(close: bool) -> Self {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let js = async_nats::jetstream::new(client.clone());
        let prefix = AcpPrefix::new("channeltest").expect("ACP prefix");
        // Global calls use core request/reply; capturing them would let a storage
        // acknowledgement answer the request before the agent does.
        for stream in [AcpStream::Commands, AcpStream::Responses] {
            js.get_or_create_stream(stream.config(&prefix))
                .await
                .expect("ACP stream");
        }
        let config = acp_nats::Config::new(prefix.clone(), NatsConfig::from_env(&InMemoryEnv::new()))
            .with_operation_timeout(Duration::from_secs(2))
            .with_prompt_timeout(Duration::from_secs(2));
        let bridge = Arc::new(AcpBridge::new(
            client.clone(),
            NatsJetStreamClient::new(js.clone()),
            trogon_std::time::SystemClock,
            &trogon_telemetry::meter("channel-port-test"),
            config,
        ));
        let capabilities = if close {
            json!({"sessionCapabilities": {"close": {}}})
        } else {
            json!({})
        };
        let methods = SessionMethods::advertised(&super::initialized(capabilities));
        let cwd = std::env::temp_dir().join("channel-port-fixture");
        let port = AcpPort::new(bridge.clone(), cwd.clone(), methods);
        Self {
            _server: server,
            client,
            js,
            prefix,
            bridge,
            port,
            cwd,
        }
    }

    async fn subscribe(&self, subject: String) -> async_nats::Subscriber {
        let subscriber = self.client.subscribe(subject).await.expect("agent subscription");
        self.client.flush().await.expect("agent subscription ready");
        subscriber
    }

    async fn reply_core(&self, request: &async_nats::Message, result: Result<Value, agent_client_protocol::Error>) {
        let id = acp_nats::wire::response_id_from_request_headers(request.headers.as_ref().expect("request headers"));
        let encoded = match result {
            Ok(value) => acp_nats::wire::encode_success(id, &value),
            Err(error) => acp_nats::wire::encode_agent_error(id, &error),
        }
        .expect("agent response");
        self.client
            .publish_with_headers(
                request.reply.clone().expect("reply subject"),
                encoded.headers,
                encoded.body,
            )
            .await
            .expect("agent reply");
    }

    async fn reply_session(
        &self,
        request: &async_nats::Message,
        session: &AcpSessionId,
        result: Result<Value, agent_client_protocol::Error>,
    ) {
        let id = acp_nats::wire::response_id_from_request_headers(request.headers.as_ref().expect("request headers"));
        let encoded = match result {
            Ok(value) => acp_nats::wire::encode_success(id, &value),
            Err(error) => acp_nats::wire::encode_agent_error(id, &error),
        }
        .expect("agent response");
        self.js
            .publish_with_headers(
                responses::ResponseSubject::new(&self.prefix, session).to_string(),
                encoded.headers,
                encoded.body,
            )
            .await
            .expect("session response publish")
            .await
            .expect("session response stored");
    }
}

async fn next(subscriber: &mut async_nats::Subscriber) -> async_nats::Message {
    tokio::time::timeout(Duration::from_secs(3), subscriber.next())
        .await
        .expect("agent request deadline")
        .expect("agent request")
}

fn conversation() -> ConversationRecord {
    ConversationRecord {
        principal: PrincipalId::new("telegram-42").expect("principal"),
        agent_id: AgentId::new("fixture-agent").expect("agent"),
        current_session: None,
        created_at: 100,
        last_activity_at: 100,
    }
}

fn event(text: Option<&str>) -> InboundEvent {
    InboundEvent {
        endpoint: Endpoint::new("telegram", "fixturebot", "42").expect("endpoint"),
        sender: Sender {
            platform_user_id: PlatformUserId::new("42").expect("sender"),
            display_name: "Pat".to_owned(),
        },
        text: text.map(str::to_owned),
        command: None,
        attachments: Vec::new(),
        message_ref: MessageRef::from(7_i64),
        occurred_at: 100,
    }
}

#[tokio::test]
async fn session_creation_forwards_workspace_and_preserves_the_agent_session_id() {
    let fixture = Fixture::new(false).await;
    let mut requests = fixture
        .subscribe(global::SessionNewSubject::new(&fixture.prefix).to_string())
        .await;
    let respond = async {
        let request = next(&mut requests).await;
        let params: NewSessionRequest = acp_nats::wire::decode_request_params(
            "session/new",
            request.headers.as_ref().expect("headers"),
            &request.payload,
        )
        .expect("session request");
        assert_eq!(params.cwd, fixture.cwd);
        assert!(params.mcp_servers.is_empty());
        fixture
            .reply_core(&request, Ok(json!({"sessionId": "session-a"})))
            .await;
    };
    let record = conversation();
    let (session, ()) = tokio::join!(fixture.port.create_session(&record), respond);
    assert_eq!(session.expect("created session").as_str(), "session-a");
    fixture.bridge.drain_background_tasks().await;
}

#[tokio::test]
async fn session_creation_preserves_agent_errors_and_rejects_unusable_ids() {
    let fixture = Fixture::new(true).await;
    let mut requests = fixture
        .subscribe(global::SessionNewSubject::new(&fixture.prefix).to_string())
        .await;
    let record = conversation();
    let respond = async {
        let request = next(&mut requests).await;
        fixture
            .reply_core(&request, Err(agent_client_protocol::Error::auth_required()))
            .await;
    };
    let (result, ()) = tokio::join!(fixture.port.create_session(&record), respond);
    assert!(
        matches!(result, Err(AcpPortError::Rpc(error)) if error.code == agent_client_protocol::ErrorCode::AuthRequired)
    );
    let respond = async {
        let request = next(&mut requests).await;
        fixture
            .reply_core(&request, Ok(json!({"sessionId": "bad session"})))
            .await;
    };
    let (result, ()) = tokio::join!(fixture.port.create_session(&record), respond);
    assert!(matches!(result, Err(AcpPortError::SessionId(_))));
    fixture.bridge.drain_background_tasks().await;
}

#[tokio::test]
async fn prompt_forwards_chat_context_and_maps_agent_stop_reasons() {
    let fixture = Fixture::new(false).await;
    for (index, (reason, outcome, text)) in [
        ("end_turn", PromptOutcome::Completed, Some("hello")),
        ("cancelled", PromptOutcome::Cancelled, None),
        ("refusal", PromptOutcome::Refused, Some("hello")),
        ("max_tokens", PromptOutcome::Truncated, Some("hello")),
    ]
    .into_iter()
    .enumerate()
    {
        let raw_session = format!("session-{index}");
        let session = AgentSessionId::new(&raw_session).expect("session");
        let acp_session = AcpSessionId::new(raw_session).expect("ACP session");
        let mut requests = fixture
            .subscribe(commands::PromptSubject::new(&fixture.prefix, &acp_session).to_string())
            .await;
        let respond = async {
            let request = next(&mut requests).await;
            let params: PromptRequest = acp_nats::wire::decode_request_params(
                "session/prompt",
                request.headers.as_ref().expect("headers"),
                &request.payload,
            )
            .expect("prompt request");
            assert_eq!(params.session_id.to_string(), session.as_str());
            assert_eq!(params.prompt.len(), 1);
            let ContentBlock::Text(content) = &params.prompt[0] else {
                panic!("expected text prompt")
            };
            assert_eq!(
                content.text,
                format!("[telegram message from Pat]\n{}", text.unwrap_or_default())
            );
            assert_eq!(
                params.meta,
                Some(
                    serde_json::from_value(json!({"chat": {
                        "channel": "telegram", "endpoint": "telegram.fixturebot.42",
                        "sender": {"platform_user_id": "42", "display_name": "Pat"},
                        "message_ref": "7", "occurred_at": 100
                    }}))
                    .expect("expected metadata")
                )
            );
            fixture
                .reply_session(&request, &acp_session, Ok(json!({"stopReason": reason})))
                .await;
        };
        let inbound = event(text);
        let (result, ()) = tokio::join!(fixture.port.prompt(&session, &inbound), respond);
        assert_eq!(result.expect("prompt outcome"), outcome);
    }
}

#[tokio::test]
async fn rejected_prompt_retains_the_session_lost_classification() {
    let fixture = Fixture::new(false).await;
    let session = AgentSessionId::new("lost-session").expect("session");
    let acp_session = AcpSessionId::new("lost-session").expect("ACP session");
    let mut requests = fixture
        .subscribe(commands::PromptSubject::new(&fixture.prefix, &acp_session).to_string())
        .await;
    let respond = async {
        let request = next(&mut requests).await;
        fixture
            .reply_session(
                &request,
                &acp_session,
                Err(agent_client_protocol::Error::invalid_params()),
            )
            .await;
    };
    let inbound = event(Some("hello"));
    let (result, ()) = tokio::join!(fixture.port.prompt(&session, &inbound), respond);
    assert!(result.expect_err("agent refused session").is_session_lost());
}

#[tokio::test]
async fn cancellation_and_unsupported_release_never_call_close() {
    let fixture = Fixture::new(false).await;
    let session = AgentSessionId::new("session-a").expect("session");
    let acp_session = AcpSessionId::new("session-a").expect("ACP session");
    let mut cancels = fixture
        .subscribe(commands::CancelSubject::new(&fixture.prefix, &acp_session).to_string())
        .await;
    let mut closes = fixture
        .subscribe(commands::CloseSubject::new(&fixture.prefix, &acp_session).to_string())
        .await;
    fixture.port.cancel(&session).await.expect("cancel");
    let message = next(&mut cancels).await;
    let headers = message.headers.unwrap_or_default();
    assert!(headers.get("Jsonrpc-Id").is_none());
    assert!(message.reply.is_none());
    let params: CancelNotification =
        acp_nats::wire::decode_notification_params("session/cancel", &headers, &message.payload)
            .expect("cancel notification");
    assert_eq!(params.session_id.to_string(), "session-a");
    let report = fixture.port.release_session(&session, ReleaseReason::NewSession).await;
    assert_eq!(report.cancelled, ReleaseStep::Done);
    assert_eq!(report.closed, ReleaseStep::Unsupported);
    next(&mut cancels).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(25), closes.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn release_cancels_before_close_and_keeps_close_failure_nonfatal() {
    let fixture = Fixture::new(true).await;
    for (name, succeeds) in [("released", true), ("refused", false)] {
        let session = AgentSessionId::new(name).expect("session");
        let acp_session = AcpSessionId::new(name).expect("ACP session");
        let mut requests = fixture
            .subscribe(format!("{}.v1.session.{name}.agent.*", fixture.prefix.as_str()))
            .await;
        let respond = async {
            assert_eq!(
                next(&mut requests).await.subject.as_str(),
                commands::CancelSubject::new(&fixture.prefix, &acp_session).to_string()
            );
            assert_eq!(
                next(&mut requests).await.subject.as_str(),
                responses::CancelledSubject::new(&fixture.prefix, &acp_session).to_string()
            );
            let request = next(&mut requests).await;
            assert_eq!(
                request.subject.as_str(),
                commands::CloseSubject::new(&fixture.prefix, &acp_session).to_string()
            );
            let response = if succeeds {
                Ok(json!({}))
            } else {
                Err(agent_client_protocol::Error::internal_error())
            };
            fixture.reply_session(&request, &acp_session, response).await;
        };
        let (report, ()) = tokio::join!(fixture.port.release_session(&session, ReleaseReason::Replaced), respond);
        assert_eq!(report.cancelled, ReleaseStep::Done);
        assert_eq!(
            report.closed,
            if succeeds {
                ReleaseStep::Done
            } else {
                ReleaseStep::Failed
            }
        );
    }
}
