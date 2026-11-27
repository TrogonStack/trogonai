use super::*;
use crate::client_claim::ClientClaim;
use crate::client_handshake::ClientHandshake;
use acp_nats::wire::{decode_response, encode_notification, encode_request};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    AuthenticateRequest, AuthenticateResponse, CancelNotification, Implementation, InitializeRequest,
    InitializeResponse, ListSessionsRequest, ListSessionsResponse, NewSessionRequest, NewSessionResponse,
    PromptRequest, PromptResponse, SetSessionModeRequest, SetSessionModeResponse, StopReason,
};
use agent_client_protocol::{Error, ErrorCode, Result};
use async_nats::{HeaderMap, Message};
use futures::channel::mpsc::UnboundedSender;
use jsonrpc_nats::RequestId;
use std::sync::{Arc, Mutex};
use trogon_nats::MockNatsClient;
use trogon_nats::jetstream::{MockJetStreamConsumer, MockJetStreamConsumerFactory, MockJsMessage};

/// The child, recording which calls reach it.
#[derive(Default)]
struct Child {
    calls: Mutex<Vec<String>>,
}

impl Child {
    fn record(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }
}

#[async_trait::async_trait]
impl acp_nats::AgentHandler for Child {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.record("initialize");
        Ok(InitializeResponse::new(args.protocol_version))
    }

    async fn authenticate(&self, _args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        self.record("authenticate");
        Ok(AuthenticateResponse::new())
    }

    async fn new_session(&self, _args: NewSessionRequest) -> Result<NewSessionResponse> {
        self.record("session/new");
        Ok(NewSessionResponse::new("s1"))
    }

    async fn list_sessions(&self, _args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        self.record("session/list");
        Ok(ListSessionsResponse::new(vec![]))
    }

    async fn prompt(&self, _args: PromptRequest) -> Result<PromptResponse> {
        self.record("session/prompt");
        Ok(PromptResponse::new(StopReason::EndTurn))
    }

    async fn set_session_mode(&self, _args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        self.record("session/set_mode");
        Ok(SetSessionModeResponse::new())
    }

    async fn cancel(&self, _args: CancelNotification) -> Result<()> {
        self.record("session/cancel");
        Ok(())
    }
}

/// A host serving [`Child`] on mock NATS: requests go in on the two core
/// subscriptions and the COMMANDS consumer, replies come out as publishes.
struct Host {
    nats: MockNatsClient,
    global: UnboundedSender<Message>,
    _ext: UnboundedSender<Message>,
    commands: UnboundedSender<std::result::Result<MockJsMessage, trogon_nats::mocks::MockError>>,
    child: Arc<Child>,
}

/// Shares the child's call log out of the agent the host owns.
struct ChildCalls(Arc<Child>);

#[async_trait::async_trait]
impl acp_nats::AgentHandler for ChildCalls {
    async fn initialize(&self, args: InitializeRequest) -> Result<InitializeResponse> {
        self.0.initialize(args).await
    }
    async fn authenticate(&self, args: AuthenticateRequest) -> Result<AuthenticateResponse> {
        self.0.authenticate(args).await
    }
    async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse> {
        self.0.new_session(args).await
    }
    async fn list_sessions(&self, args: ListSessionsRequest) -> Result<ListSessionsResponse> {
        self.0.list_sessions(args).await
    }
    async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
        self.0.prompt(args).await
    }
    async fn set_session_mode(&self, args: SetSessionModeRequest) -> Result<SetSessionModeResponse> {
        self.0.set_session_mode(args).await
    }
    async fn cancel(&self, args: CancelNotification) -> Result<()> {
        self.0.cancel(args).await
    }
}

impl Host {
    fn start() -> Self {
        let nats = MockNatsClient::new();
        let global = nats.inject_messages();
        let ext = nats.inject_messages();
        let factory = MockJetStreamConsumerFactory::new();
        let (consumer, commands) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);
        let child = Arc::new(Child::default());
        let agent = InitializedAgent::new(
            ChildCalls(child.clone()),
            None,
            Arc::new(ClientClaim::default()),
            Arc::new(ClientHandshake::default()),
        );
        tokio::task::spawn_local(serve(
            agent,
            nats.clone(),
            factory,
            AcpPrefix::new("acp").unwrap(),
            std::future::pending(),
        ));
        Self {
            nats,
            global,
            _ext: ext,
            commands,
            child,
        }
    }

    fn calls(&self) -> Vec<String> {
        self.child.calls.lock().unwrap().clone()
    }

    /// Sends a core request as a bridge for `client` would, and waits for the
    /// reply on its own inbox.
    async fn request<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        client: Option<&str>,
        subject: &str,
        method: &str,
        args: &Req,
    ) -> Result<Res> {
        let inbox = format!("_INBOX.{}", uuid());
        let encoded = encode_request(method, RequestId::String(uuid()), args).unwrap();
        self.global
            .unbounded_send(message(
                subject,
                Some(&inbox),
                stamped(encoded.headers, client),
                encoded.body,
            ))
            .unwrap();
        self.reply_on(&inbox).await
    }

    /// Sends a session command through COMMANDS, as the bridge's JetStream
    /// path does, and waits for its reply on the session's response subject.
    async fn command<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        client: Option<&str>,
        subject: &str,
        method: &str,
        args: &Req,
    ) -> Result<Res> {
        let before = self.published_on("acp.v1.session.s1.agent.response").len();
        let encoded = encode_request(method, RequestId::String(uuid()), args).unwrap();
        self.commands
            .unbounded_send(Ok(MockJsMessage::new(message(
                subject,
                None,
                stamped(encoded.headers, client),
                encoded.body,
            ))))
            .unwrap();
        for _ in 0..1000 {
            let replies = self.published_on("acp.v1.session.s1.agent.response");
            if replies.len() > before {
                let (headers, body) = &replies[before];
                return decode_response(headers, body).unwrap();
            }
            tokio::task::yield_now().await;
        }
        panic!("no reply to {subject}");
    }

    async fn notify<Req: serde::Serialize>(&self, client: Option<&str>, subject: &str, method: &str, args: &Req) {
        let encoded = encode_notification(method, args).unwrap();
        self.commands
            .unbounded_send(Ok(MockJsMessage::new(message(
                subject,
                None,
                stamped(encoded.headers, client),
                encoded.body,
            ))))
            .unwrap();
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
    }

    async fn reply_on<Res: serde::de::DeserializeOwned>(&self, inbox: &str) -> Result<Res> {
        for _ in 0..1000 {
            if let Some((headers, body)) = self.published_on(inbox).first() {
                return decode_response(headers, body).unwrap();
            }
            tokio::task::yield_now().await;
        }
        panic!("no reply on {inbox}");
    }

    fn published_on(&self, subject: &str) -> Vec<(HeaderMap, bytes::Bytes)> {
        let subjects = self.nats.published_messages();
        let headers = self.nats.published_headers();
        let payloads = self.nats.published_payloads();
        subjects
            .into_iter()
            .zip(headers.into_iter().zip(payloads))
            .filter(|(published, _)| published == subject)
            .map(|(_, reply)| reply)
            .collect()
    }
}

fn uuid() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::SeqCst).to_string()
}

fn stamped(mut headers: HeaderMap, client: Option<&str>) -> HeaderMap {
    if let Some(client) = client {
        headers.insert(acp_nats::ACP_CLIENT_HEADER, client);
    }
    headers
}

fn message(subject: &str, reply: Option<&str>, headers: HeaderMap, body: bytes::Bytes) -> Message {
    Message {
        subject: subject.into(),
        reply: reply.map(Into::into),
        length: body.len(),
        payload: body,
        headers: Some(headers),
        status: None,
        description: None,
    }
}

fn initialize(name: &str) -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1).client_info(Implementation::new(name, "1"))
}

fn assert_refused<T: std::fmt::Debug>(result: Result<T>, what: &str) {
    let error: Error = result.expect_err(what);
    assert_eq!(error.code, ErrorCode::INVALID_REQUEST, "{what}");
    assert!(error.message.contains("serves one client"), "{what}: {}", error.message);
    assert!(error.message.contains("`zed`"), "{what}: {}", error.message);
}

#[tokio::test]
async fn a_second_client_is_refused_on_every_request_and_the_first_keeps_working() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let host = Host::start();
            let discord = Some("discord");
            let zed = Some("zed");

            let _: InitializeResponse = host
                .request(
                    discord,
                    "acp.v1.global.agent.initialize",
                    "initialize",
                    &initialize("discord"),
                )
                .await
                .expect("discord initializes");
            let session: NewSessionResponse = host
                .request(
                    discord,
                    "acp.v1.global.agent.session.new",
                    "session/new",
                    &NewSessionRequest::new("/tmp"),
                )
                .await
                .expect("discord opens a session");
            assert_eq!(session.session_id.to_string(), "s1");
            let calls_before_zed = host.calls();

            // zed shares the prefix by mistake. Its bridge stamps its name on
            // everything once it has tried to initialize.
            assert_refused(
                host.request::<_, InitializeResponse>(
                    zed,
                    "acp.v1.global.agent.initialize",
                    "initialize",
                    &initialize("zed"),
                )
                .await,
                "initialize",
            );
            assert_refused(
                host.request::<_, NewSessionResponse>(
                    zed,
                    "acp.v1.global.agent.session.new",
                    "session/new",
                    &NewSessionRequest::new("/tmp"),
                )
                .await,
                "session/new",
            );
            assert_refused(
                host.request::<_, ListSessionsResponse>(
                    zed,
                    "acp.v1.global.agent.session.list",
                    "session/list",
                    &ListSessionsRequest::new(),
                )
                .await,
                "session/list",
            );
            assert_refused(
                host.request::<_, AuthenticateResponse>(
                    zed,
                    "acp.v1.global.agent.authenticate",
                    "authenticate",
                    &AuthenticateRequest::new("key"),
                )
                .await,
                "authenticate",
            );
            assert_refused(
                host.command::<_, PromptResponse>(
                    zed,
                    "acp.v1.session.s1.agent.prompt",
                    "session/prompt",
                    &PromptRequest::new("s1", vec![]),
                )
                .await,
                "session/prompt",
            );
            assert_refused(
                host.command::<_, SetSessionModeResponse>(
                    zed,
                    "acp.v1.session.s1.agent.set_mode",
                    "session/set_mode",
                    &SetSessionModeRequest::new("s1", "plan"),
                )
                .await,
                "session/set_mode",
            );
            host.notify(
                zed,
                "acp.v1.session.s1.agent.cancel",
                "session/cancel",
                &CancelNotification::new("s1"),
            )
            .await;
            assert_eq!(host.calls(), calls_before_zed, "nothing from zed reached the child");

            let reply: PromptResponse = host
                .command(
                    discord,
                    "acp.v1.session.s1.agent.prompt",
                    "session/prompt",
                    &PromptRequest::new("s1", vec![]),
                )
                .await
                .expect("discord's session is still its own");
            assert_eq!(reply.stop_reason, StopReason::EndTurn);
        })
        .await;
}

#[tokio::test]
async fn a_lone_client_without_the_header_still_works() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let host = Host::start();

            let _: InitializeResponse = host
                .request(
                    None,
                    "acp.v1.global.agent.initialize",
                    "initialize",
                    &initialize("discord"),
                )
                .await
                .expect("initialize");
            let _: NewSessionResponse = host
                .request(
                    None,
                    "acp.v1.global.agent.session.new",
                    "session/new",
                    &NewSessionRequest::new("/tmp"),
                )
                .await
                .expect("session/new");
            let reply: PromptResponse = host
                .command(
                    None,
                    "acp.v1.session.s1.agent.prompt",
                    "session/prompt",
                    &PromptRequest::new("s1", vec![]),
                )
                .await
                .expect("session/prompt");

            assert_eq!(reply.stop_reason, StopReason::EndTurn);
            assert_eq!(host.calls(), vec!["initialize", "session/new", "session/prompt"]);
        })
        .await;
}

#[tokio::test]
async fn a_request_naming_a_client_before_any_initialize_does_not_lock_out_the_real_client() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let host = Host::start();
            let zed = Some("zed");

            // One stray request naming a client, and failing, arrives before
            // the real client initializes.
            let error = host
                .request::<_, InitializeResponse>(
                    zed,
                    "acp.v1.global.agent.initialize",
                    "initialize",
                    &serde_json::json!({"clientInfo": 7}),
                )
                .await
                .expect_err("not an initialize");
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);

            let _: InitializeResponse = host
                .request(
                    Some("discord"),
                    "acp.v1.global.agent.initialize",
                    "initialize",
                    &initialize("discord"),
                )
                .await
                .expect("the real client initializes");
            let _: NewSessionResponse = host
                .request(
                    Some("discord"),
                    "acp.v1.global.agent.session.new",
                    "session/new",
                    &NewSessionRequest::new("/tmp"),
                )
                .await
                .expect("and is served");
            assert_refused(
                host.request::<_, NewSessionResponse>(
                    zed,
                    "acp.v1.global.agent.session.new",
                    "session/new",
                    &NewSessionRequest::new("/tmp"),
                )
                .await,
                "the stray client, once the real one has claimed the host",
            );
        })
        .await;
}

fn unserved_agent() -> InitializedAgent<Child> {
    InitializedAgent::new(Child::default(), None, Arc::default(), Arc::default())
}

#[tokio::test]
async fn a_run_whose_serving_fails_re_creates_the_streams_for_the_next_run() {
    tokio::task::LocalSet::new()
        .run_until(async {
            // NATS restarted without its JetStream storage: no stream exists,
            // so serving fails on the missing COMMANDS stream.
            let js = trogon_nats::jetstream::MockJetStreamContext::new();
            let nats = MockNatsClient::new();
            let _global = nats.inject_messages();
            let _ext = nats.inject_messages();
            let prefix = AcpPrefix::new("acp").unwrap();

            let error = serve_and_reprovision(
                unserved_agent(),
                nats,
                js.clone(),
                prefix.clone(),
                std::future::pending(),
            )
            .await
            .expect_err("the stream is gone");

            assert!(matches!(error, ServeError::Connection(_)), "{error}");
            let names: Vec<String> = js.created_streams().into_iter().map(|config| config.name).collect();
            assert_eq!(
                names,
                [
                    acp_nats::nats::AcpStream::Commands.stream_name(&prefix),
                    acp_nats::nats::AcpStream::Responses.stream_name(&prefix),
                ]
            );
        })
        .await;
}

#[tokio::test]
async fn a_run_that_cannot_provision_again_reports_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let js = trogon_nats::jetstream::MockJetStreamContext::new();
            js.fail_next();
            let nats = MockNatsClient::new();
            let _global = nats.inject_messages();
            let _ext = nats.inject_messages();

            let error = serve_and_reprovision(
                unserved_agent(),
                nats,
                js,
                AcpPrefix::new("acp").unwrap(),
                std::future::pending(),
            )
            .await
            .expect_err("the stream is gone and cannot be re-created");

            assert!(matches!(error, ServeError::Provision(_)), "{error}");
        })
        .await;
}
