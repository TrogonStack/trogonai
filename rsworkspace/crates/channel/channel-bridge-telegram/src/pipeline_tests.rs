use super::*;
use crate::outbound::Outbound;
use acp_nats::ClientHandler;
use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, SessionNotification, SessionUpdate, TextContent};
use futures::StreamExt;
use std::cell::RefCell;
use std::rc::Rc;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_channel::store::PrincipalRecord;
use trogon_channel::{
    AgentPortError, AgentSessionId, Endpoint, InboundEvent, PrincipalId, PromptOutcome, ReleaseStep, SessionRelease,
};
use trogon_nats::jetstream::{
    ClaimCheckPublisher, ClaimRetention, DEFAULT_CLAIM_BUCKET, MaxPayload, NatsJetStreamClient, NatsObjectStore,
};
use trogon_std::UuidV7Generator;

struct NatsServer {
    _container: ContainerAsync<Nats>,
    url: String,
}

impl NatsServer {
    async fn start() -> Self {
        let cmd = NatsServerCmd::default().with_jetstream();
        let container = Nats::default()
            .with_cmd(&cmd)
            .start()
            .await
            .expect("start NATS testcontainer");
        let host = container.get_host().await.expect("get host");
        let port = container.get_host_port_ipv4(4222).await.expect("get port");
        Self {
            _container: container,
            url: format!("{host}:{port}"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("fake agent failure")]
struct FakeError;

impl AgentPortError for FakeError {
    fn is_session_lost(&self) -> bool {
        false
    }
}

/// Simulates the agent side: mints sessions, records prompts, and streams a
/// reply into the renderer the way real session notifications would.
struct FakePort {
    renderer: Rc<TelegramRenderClient>,
    reply: String,
    sessions_created: RefCell<u32>,
    prompted: RefCell<Vec<(String, String)>>,
    released: RefCell<Vec<String>>,
}

impl trogon_channel::AgentPort for FakePort {
    type Error = FakeError;

    async fn create_session(
        &self,
        _conversation: &trogon_channel::ConversationRecord,
    ) -> Result<AgentSessionId, Self::Error> {
        *self.sessions_created.borrow_mut() += 1;
        Ok(AgentSessionId::new(format!("sess-{}", self.sessions_created.borrow())))
    }

    async fn prompt(&self, session: &AgentSessionId, event: &InboundEvent) -> Result<PromptOutcome, Self::Error> {
        self.prompted
            .borrow_mut()
            .push((session.as_str().to_string(), event.text.clone().unwrap_or_default()));
        let notification = SessionNotification::new(
            session.as_str().to_string(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
                self.reply.clone(),
            )))),
        );
        self.renderer
            .session_notification(notification)
            .await
            .expect("renderer accepts notification");
        Ok(PromptOutcome::Completed)
    }

    async fn cancel(&self, _session: &AgentSessionId) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn release_session(
        &self,
        session: &AgentSessionId,
        _reason: trogon_channel::ReleaseReason,
    ) -> SessionRelease {
        self.released.borrow_mut().push(session.as_str().to_string());
        SessionRelease {
            cancelled: ReleaseStep::Done,
            closed: ReleaseStep::Done,
        }
    }
}

#[derive(Default)]
struct FakeOutbound {
    typing: RefCell<u32>,
    sent: RefCell<Vec<(i64, String)>>,
}

impl Outbound for FakeOutbound {
    async fn typing(&self, _chat_id: i64) -> anyhow::Result<()> {
        *self.typing.borrow_mut() += 1;
        Ok(())
    }

    async fn send_text(&self, chat_id: i64, text: String) -> anyhow::Result<()> {
        self.sent.borrow_mut().push((chat_id, text));
        Ok(())
    }
}

fn raw_update(update_id: u64, chat_id: i64, user_id: u64, text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id,
            "date": 1_700_000_000,
            "chat": { "id": chat_id, "type": "private", "first_name": "Test" },
            "from": { "id": user_id, "is_bot": false, "first_name": "Test" },
            "text": text,
        }
    }))
    .expect("serialize update")
}

/// Consumer state once its acks have landed. `msg.ack()` does not wait for the
/// server, so a snapshot taken the instant the pipeline returns can still show
/// the last message pending.
async fn settled_consumer_info(
    stream: &async_nats::jetstream::stream::Stream,
    consumer: &str,
) -> async_nats::jetstream::consumer::Info {
    for _ in 0..40 {
        let info = stream.consumer_info(consumer).await.expect("consumer info");
        if info.num_ack_pending == 0 && info.num_pending == 0 {
            return info;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    stream.consumer_info(consumer).await.expect("consumer info")
}

/// The bucket the gateway offloads oversized bodies into, opened the way the
/// bridge opens it. Provisioned here because in a deployment the gateway has
/// already done so.
async fn claim_resolver(js: &async_nats::jetstream::Context) -> ClaimResolver<NatsObjectStore> {
    let store = NatsObjectStore::provision_claim_bucket(js, DEFAULT_CLAIM_BUCKET, ClaimRetention::EventSourced)
        .await
        .expect("provision claim bucket");
    ClaimResolver::new(store, DEFAULT_CLAIM_BUCKET)
}

/// End to end against a real NATS: gateway-shaped raw updates in, identity
/// gate, conversation + session KV, prompt, rendered reply out, and the reset
/// command that rotates the session under a stable conversation. One container
/// for the whole scenario.
#[tokio::test]
async fn pipeline_routes_gateway_updates_to_the_agent_and_back() {
    let server = NatsServer::start().await;
    let client = async_nats::connect(&server.url).await.expect("connect");
    let js = async_nats::jetstream::new(client);

    js.create_stream(async_nats::jetstream::stream::Config {
        name: "TELEGRAM".to_string(),
        subjects: vec!["telegram.>".to_string()],
        ..Default::default()
    })
    .await
    .expect("create TELEGRAM stream");

    let store = ChannelStore::ensure(&js, "test").await.expect("ensure buckets");
    let principal = PrincipalId::new("telegram-42").expect("principal");
    let endpoint = Endpoint::new("telegram", "mybot", "42").expect("endpoint");
    store
        .link_endpoint(&principal, &PrincipalRecord { display_name: None }, &endpoint)
        .await
        .expect("seed principal");

    for (update_id, chat, user, text) in [
        (1u64, 99i64, 99u64, "intruder"),
        (2, 42, 42, "hello"),
        (3, 42, 42, "again"),
        (4, 42, 42, "/new"),
        (5, 42, 42, "keep going"),
        (6, 42, 42, "/reset finish up"),
    ] {
        js.publish("telegram.message", raw_update(update_id, chat, user, text).into())
            .await
            .expect("publish")
            .await
            .expect("ack");
    }

    let stream = js.get_stream("TELEGRAM").await.expect("get stream");
    let consumer = stream
        .get_or_create_consumer(
            "bridge-test",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("bridge-test".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("consumer");
    let mut messages = consumer.messages().await.expect("messages");

    let renderer = Rc::new(TelegramRenderClient::new());
    let port = FakePort {
        renderer: renderer.clone(),
        reply: "hi there".to_string(),
        sessions_created: RefCell::new(0),
        prompted: RefCell::new(Vec::new()),
        released: RefCell::new(Vec::new()),
    };
    let outbound = FakeOutbound::default();
    let triggers = CommandTriggers::default();
    let claims = claim_resolver(&js).await;
    let pipeline = Pipeline {
        store: &store,
        port: &port,
        renderer: renderer.as_ref(),
        outbound: &outbound,
        claims: &claims,
        bot_account: "mybot",
        agent_id: "default",
        triggers: &triggers,
        ids: &UuidV7Generator,
    };

    for _ in 0..6 {
        let msg = messages.next().await.expect("stream yields").expect("message received");
        pipeline.handle_message(&msg).await.expect("handled");
    }

    // The unauthorized endpoint never reached the agent and got no reply.
    let intruder_endpoint = Endpoint::new("telegram", "mybot", "99").expect("endpoint");
    assert!(
        store
            .conversation_for(&intruder_endpoint)
            .await
            .expect("kv read")
            .is_none()
    );

    // Consecutive messages share a session; each reset mints the next one, and
    // the text after a trigger is prompted rather than forwarded verbatim.
    assert_eq!(*port.sessions_created.borrow(), 3);
    assert_eq!(
        *port.prompted.borrow(),
        vec![
            ("sess-1".to_string(), "hello".to_string()),
            ("sess-1".to_string(), "again".to_string()),
            ("sess-2".to_string(), "keep going".to_string()),
            ("sess-3".to_string(), "finish up".to_string()),
        ]
    );
    assert_eq!(
        *port.released.borrow(),
        vec!["sess-1".to_string(), "sess-2".to_string()]
    );

    // The conversation and its principal outlive every session rotation.
    let (_, record) = store
        .conversation_for(&endpoint)
        .await
        .expect("kv read")
        .expect("conversation exists");
    assert_eq!(record.principal, principal);
    assert_eq!(
        record.current_session.as_ref().map(AgentSessionId::as_str),
        Some("sess-3")
    );

    assert_eq!(*outbound.typing.borrow(), 4);
    assert_eq!(
        *outbound.sent.borrow(),
        vec![
            (42, "hi there".to_string()),
            (42, "hi there".to_string()),
            (42, "Started a new session.".to_string()),
            (42, "hi there".to_string()),
            (42, "hi there".to_string()),
        ]
    );

    // Everything acked: nothing left pending for redelivery.
    let info = settled_consumer_info(&stream, "bridge-test").await;
    assert_eq!(info.num_ack_pending, 0);
    assert_eq!(info.num_pending, 0);
}

/// An update the gateway had to offload still reaches the agent. The gateway
/// publishes through `ClaimCheckPublisher`, so a body over the NATS max payload
/// arrives on the stream as an empty payload plus claim headers, and a consumer
/// that deserializes the payload sees zero bytes. Published here through the
/// same publisher the gateway uses, with the threshold driven to zero so every
/// body takes that path.
#[tokio::test]
async fn pipeline_redeems_a_claim_checked_update() {
    let server = NatsServer::start().await;
    let client = async_nats::connect(&server.url).await.expect("connect");
    let js = async_nats::jetstream::new(client);

    js.create_stream(async_nats::jetstream::stream::Config {
        name: "TELEGRAM".to_string(),
        subjects: vec!["telegram.>".to_string()],
        ..Default::default()
    })
    .await
    .expect("create TELEGRAM stream");

    let store = ChannelStore::ensure(&js, "test").await.expect("ensure buckets");
    let principal = PrincipalId::new("telegram-42").expect("principal");
    let endpoint = Endpoint::new("telegram", "mybot", "42").expect("endpoint");
    store
        .link_endpoint(&principal, &PrincipalRecord { display_name: None }, &endpoint)
        .await
        .expect("seed principal");

    let claims = claim_resolver(&js).await;
    let gateway = ClaimCheckPublisher::new(
        NatsJetStreamClient::new(js.clone()),
        NatsObjectStore::bind(&js, DEFAULT_CLAIM_BUCKET)
            .await
            .expect("bind claim bucket"),
        DEFAULT_CLAIM_BUCKET.to_string(),
        MaxPayload::from_server_limit(0),
    );
    let outcome = gateway
        .publish_event(
            "telegram.message".to_string(),
            async_nats::HeaderMap::new(),
            raw_update(1, 42, 42, "hello from a big body").into(),
            std::time::Duration::from_secs(5),
        )
        .await;
    assert!(outcome.is_ok(), "gateway publish failed: {outcome:?}");

    let stream = js.get_stream("TELEGRAM").await.expect("get stream");
    let consumer = stream
        .get_or_create_consumer(
            "bridge-test",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("bridge-test".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("consumer");
    let mut messages = consumer.messages().await.expect("messages");

    let renderer = Rc::new(TelegramRenderClient::new());
    let port = FakePort {
        renderer: renderer.clone(),
        reply: "hi there".to_string(),
        sessions_created: RefCell::new(0),
        prompted: RefCell::new(Vec::new()),
        released: RefCell::new(Vec::new()),
    };
    let outbound = FakeOutbound::default();
    let triggers = CommandTriggers::default();
    let pipeline = Pipeline {
        store: &store,
        port: &port,
        renderer: renderer.as_ref(),
        outbound: &outbound,
        claims: &claims,
        bot_account: "mybot",
        agent_id: "default",
        triggers: &triggers,
        ids: &UuidV7Generator,
    };

    let msg = messages.next().await.expect("stream yields").expect("message received");
    // The premise of the test: parsing what arrived would have failed.
    assert!(msg.payload.is_empty());
    pipeline.handle_message(&msg).await.expect("handled");

    assert_eq!(
        *port.prompted.borrow(),
        vec![("sess-1".to_string(), "hello from a big body".to_string())]
    );
    assert_eq!(*outbound.sent.borrow(), vec![(42, "hi there".to_string())]);

    let info = settled_consumer_info(&stream, "bridge-test").await;
    assert_eq!(info.num_ack_pending, 0);
    assert_eq!(info.num_pending, 0);
}

/// A claim that cannot be redeemed is left for redelivery instead of acked.
/// Dropping it would be permanent, and the payload alone carries no sign that
/// anything was lost.
#[tokio::test]
async fn pipeline_leaves_an_unredeemable_claim_unacked() {
    let server = NatsServer::start().await;
    let client = async_nats::connect(&server.url).await.expect("connect");
    let js = async_nats::jetstream::new(client);

    js.create_stream(async_nats::jetstream::stream::Config {
        name: "TELEGRAM".to_string(),
        subjects: vec!["telegram.>".to_string()],
        ..Default::default()
    })
    .await
    .expect("create TELEGRAM stream");

    let store = ChannelStore::ensure(&js, "test").await.expect("ensure buckets");
    let claims = claim_resolver(&js).await;
    let gateway = ClaimCheckPublisher::new(
        NatsJetStreamClient::new(js.clone()),
        NatsObjectStore::bind(&js, DEFAULT_CLAIM_BUCKET)
            .await
            .expect("bind claim bucket"),
        DEFAULT_CLAIM_BUCKET.to_string(),
        MaxPayload::from_server_limit(0),
    );
    let outcome = gateway
        .publish_event(
            "telegram.message".to_string(),
            async_nats::HeaderMap::new(),
            raw_update(1, 42, 42, "hello").into(),
            std::time::Duration::from_secs(5),
        )
        .await;
    assert!(outcome.is_ok(), "gateway publish failed: {outcome:?}");

    // Simulates an object expired or never written: the claim survives, the
    // bytes do not.
    js.delete_object_store(DEFAULT_CLAIM_BUCKET)
        .await
        .expect("drop claim bucket");
    js.create_object_store(async_nats::jetstream::object_store::Config {
        bucket: DEFAULT_CLAIM_BUCKET.to_string(),
        ..Default::default()
    })
    .await
    .expect("recreate claim bucket");

    let stream = js.get_stream("TELEGRAM").await.expect("get stream");
    let consumer = stream
        .get_or_create_consumer(
            "bridge-test",
            async_nats::jetstream::consumer::pull::Config {
                durable_name: Some("bridge-test".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("consumer");
    let mut messages = consumer.messages().await.expect("messages");

    let renderer = Rc::new(TelegramRenderClient::new());
    let port = FakePort {
        renderer: renderer.clone(),
        reply: "hi there".to_string(),
        sessions_created: RefCell::new(0),
        prompted: RefCell::new(Vec::new()),
        released: RefCell::new(Vec::new()),
    };
    let outbound = FakeOutbound::default();
    let triggers = CommandTriggers::default();
    let pipeline = Pipeline {
        store: &store,
        port: &port,
        renderer: renderer.as_ref(),
        outbound: &outbound,
        claims: &claims,
        bot_account: "mybot",
        agent_id: "default",
        triggers: &triggers,
        ids: &UuidV7Generator,
    };

    let msg = messages.next().await.expect("stream yields").expect("message received");
    let error = pipeline.handle_message(&msg).await.expect_err("must not be acked");
    assert!(error.to_string().contains("failed to redeem claim-checked update"));

    assert!(port.prompted.borrow().is_empty());
    assert!(outbound.sent.borrow().is_empty());

    let info = stream.consumer_info("bridge-test").await.expect("consumer info");
    assert_eq!(info.num_ack_pending, 1);
}
