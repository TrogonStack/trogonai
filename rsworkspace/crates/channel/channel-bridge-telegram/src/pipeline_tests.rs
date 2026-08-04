use super::*;
use crate::outbound::{SendText, SendTyping};
use acp_nats::ClientHandler;
use agent_client_protocol::schema::v1::{ContentBlock, ContentChunk, SessionNotification, SessionUpdate, TextContent};
use futures::StreamExt;
use std::cell::RefCell;
use std::rc::Rc;
use testcontainers_modules::nats::{Nats, NatsServerCmd};
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_channel::store::PrincipalRecord;
use trogon_channel::{
    AgentPortError, AgentSessionId, Endpoint, InboundEvent, MessageRef, PlatformUserId, PrincipalId, PromptOutcome,
    ReleaseReason, ReleaseStep, Sender, SessionRelease,
};
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding, MockObjectStore};
use trogon_std::UuidV7Generator;

// The claim-check scenarios below need the real object store and publisher, which
// the coverage build leaves out; the scenarios that carry no claim do not.
#[cfg(not(coverage))]
use trogon_nats::jetstream::{
    ClaimCheckPublisher, ClaimRetention, DEFAULT_CLAIM_BUCKET, MaxPayload, NatsJetStreamClient, NatsObjectStore,
};

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
#[error("fake agent failure (session_lost={session_lost})")]
struct FakeError {
    session_lost: bool,
}

impl AgentPortError for FakeError {
    fn is_session_lost(&self) -> bool {
        self.session_lost
    }
}

/// Simulates the agent side: mints sessions, records prompts, and streams a
/// reply into the renderer the way real session notifications would.
struct FakePort {
    renderer: Rc<TelegramRenderClient>,
    reply: String,
    sessions_created: RefCell<u32>,
    prompted: RefCell<Vec<(String, String)>>,
    released: RefCell<Vec<(String, ReleaseReason)>>,
    /// How many upcoming prompts fail with an error classified as a lost
    /// session. One models a session the agent really did forget, so the fresh
    /// session succeeds; two makes the fresh session fail the same way, which is
    /// the misclassification, where the prompt itself is being rejected and no
    /// fresh session helps.
    rejections: RefCell<u32>,
    /// How many upcoming prompts fail with an error that is *not* a lost
    /// session, which is every ordinary agent failure. No fresh session is
    /// opened for these: redelivery retries on the session the conversation has.
    refusals: RefCell<u32>,
    /// How many upcoming session creations fail. Models an agent that has
    /// stopped issuing sessions, which is what turns a suspected lost session
    /// into a dead end rather than a repair.
    creation_failures: RefCell<u32>,
    /// How many upcoming turns end without streaming any text. A real agent
    /// does this when it acts only through tool calls.
    silent_turns: RefCell<u32>,
}

/// Consume one use of a scripted behaviour.
fn scripted(counter: &RefCell<u32>) -> bool {
    let mut left = counter.borrow_mut();
    let scripted = *left > 0;
    *left = left.saturating_sub(1);
    scripted
}

impl FakePort {
    fn new(renderer: Rc<TelegramRenderClient>, reply: &str) -> Self {
        Self {
            renderer,
            reply: reply.to_string(),
            sessions_created: RefCell::new(0),
            prompted: RefCell::new(Vec::new()),
            released: RefCell::new(Vec::new()),
            rejections: RefCell::new(0),
            refusals: RefCell::new(0),
            creation_failures: RefCell::new(0),
            silent_turns: RefCell::new(0),
        }
    }

    fn reject_next_prompts(&self, count: u32) {
        *self.rejections.borrow_mut() = count;
    }

    fn refuse_next_prompts(&self, count: u32) {
        *self.refusals.borrow_mut() = count;
    }

    fn fail_next_session_creations(&self, count: u32) {
        *self.creation_failures.borrow_mut() = count;
    }

    fn stay_silent_for_next_turns(&self, count: u32) {
        *self.silent_turns.borrow_mut() = count;
    }

    async fn stream(&self, session: &AgentSessionId, text: &str) {
        let notification = SessionNotification::new(
            session.as_str().to_string(),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
                text.to_string(),
            )))),
        );
        self.renderer
            .session_notification(notification)
            .await
            .expect("renderer accepts notification");
    }
}

impl trogon_channel::AgentPort for FakePort {
    type Error = FakeError;

    async fn create_session(
        &self,
        _conversation: &trogon_channel::ConversationRecord,
    ) -> Result<AgentSessionId, Self::Error> {
        if scripted(&self.creation_failures) {
            return Err(FakeError { session_lost: false });
        }
        *self.sessions_created.borrow_mut() += 1;
        Ok(AgentSessionId::new(format!("sess-{}", self.sessions_created.borrow())).expect("session id"))
    }

    async fn prompt(&self, session: &AgentSessionId, event: &InboundEvent) -> Result<PromptOutcome, Self::Error> {
        self.prompted
            .borrow_mut()
            .push((session.as_str().to_string(), event.text.clone().unwrap_or_default()));

        // A real agent streams some text before the turn fails, so pipeline
        // tests catch a leftover buffer surviving into redelivery.
        if scripted(&self.rejections) {
            self.stream(session, "partial-").await;
            return Err(FakeError { session_lost: true });
        }
        if scripted(&self.refusals) {
            self.stream(session, "partial-").await;
            return Err(FakeError { session_lost: false });
        }
        if scripted(&self.silent_turns) {
            return Ok(PromptOutcome::Completed);
        }

        self.stream(session, &self.reply).await;
        Ok(PromptOutcome::Completed)
    }

    async fn cancel(&self, _session: &AgentSessionId) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn release_session(&self, session: &AgentSessionId, reason: ReleaseReason) -> SessionRelease {
        self.released.borrow_mut().push((session.as_str().to_string(), reason));
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

impl SendTyping for FakeOutbound {
    type Error = std::convert::Infallible;
    type Output = ();

    async fn typing(&self, _chat_id: i64) -> Result<Self::Output, Self::Error> {
        *self.typing.borrow_mut() += 1;
        Ok(())
    }
}

impl SendText for FakeOutbound {
    type Error = std::convert::Infallible;
    type Message = ();

    async fn send_text(&self, chat_id: i64, text: String) -> Result<Self::Message, Self::Error> {
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

/// A group message, where the chat and the speaker are two different endpoints.
/// That split is the whole reason `sender_is_authorized` exists: authorizing a
/// group chat authorizes everyone who can post in it.
fn raw_group_update(update_id: u64, chat_id: i64, user_id: u64, text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "update_id": update_id,
        "message": {
            "message_id": update_id,
            "date": 1_700_000_000,
            "chat": { "id": chat_id, "type": "group", "title": "Team" },
            "from": { "id": user_id, "is_bot": false, "first_name": "Test" },
            "text": text,
        }
    }))
    .expect("serialize update")
}

/// An update kind the bridge does not carry. Kept whole on the raw stream, but
/// nothing downstream of `parse` ever sees it.
fn raw_edit(update_id: u64, chat_id: i64, user_id: u64, text: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "update_id": update_id,
        "edited_message": {
            "message_id": update_id,
            "date": 1_700_000_000,
            "edit_date": 1_700_000_001,
            "chat": { "id": chat_id, "type": "private", "first_name": "Test" },
            "from": { "id": user_id, "is_bot": false, "first_name": "Test" },
            "text": text,
        }
    }))
    .expect("serialize update")
}

/// The next message off the stream. Taken one at a time rather than in a loop
/// so a scenario can change the agent's behaviour between messages.
async fn next_message<S, E>(messages: &mut S) -> async_nats::jetstream::Message
where
    S: futures::Stream<Item = Result<async_nats::jetstream::Message, E>> + Unpin,
    E: std::fmt::Debug,
{
    messages.next().await.expect("stream yields").expect("message received")
}

/// Consumer state once its acks have landed. `msg.ack()` does not wait for the
/// server, so a snapshot taken the instant the pipeline returns can still show
/// the last message pending. `ack_pending` is what the caller expects to remain
/// outstanding: zero when everything was acked, and one per message the pipeline
/// deliberately left for redelivery.
async fn settled_consumer_info(
    stream: &async_nats::jetstream::stream::Stream,
    consumer: &str,
    ack_pending: u64,
) -> async_nats::jetstream::consumer::Info {
    for _ in 0..40 {
        let info = stream.consumer_info(consumer).await.expect("consumer info");
        if info.num_ack_pending as u64 == ack_pending && info.num_pending == 0 {
            return info;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    stream.consumer_info(consumer).await.expect("consumer info")
}

/// A resolver for scenarios whose updates carry no claim headers, so nothing is
/// ever redeemed through it. Mock-backed rather than bucket-backed to keep those
/// scenarios off the object store.
fn unclaimed_resolver() -> ClaimResolver<MockObjectStore> {
    ClaimResolver::new(ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()))
}

/// The bucket the gateway offloads oversized bodies into, opened the way the
/// bridge opens it. Provisioned here because in a deployment the gateway has
/// already done so.
#[cfg(not(coverage))]
async fn claim_resolver(js: &async_nats::jetstream::Context) -> ClaimResolver<NatsObjectStore> {
    NatsObjectStore::provision_claim_bucket(js, &ClaimBucket::default(), ClaimRetention::EventSourced)
        .await
        .expect("provision claim bucket");
    ClaimResolver::new(
        NatsObjectStore::bind_claim_bucket(js, ClaimBucket::default())
            .await
            .expect("bind claim bucket"),
    )
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
    let port = FakePort::new(renderer.clone(), "hi there");
    let outbound = FakeOutbound::default();
    let triggers = CommandTriggers::default();
    let claims = unclaimed_resolver();
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
        let msg = next_message(&mut messages).await;
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
        vec![
            ("sess-1".to_string(), ReleaseReason::NewSession),
            ("sess-2".to_string(), ReleaseReason::NewSession),
        ]
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
    let info = settled_consumer_info(&stream, "bridge-test", 0).await;
    assert_eq!(info.num_ack_pending, 0);
    assert_eq!(info.num_pending, 0);
}

/// An update the gateway had to offload still reaches the agent. The gateway
/// publishes through `ClaimCheckPublisher`, so a body over the NATS max payload
/// arrives on the stream as an empty payload plus claim headers, and a consumer
/// that deserializes the payload sees zero bytes. Published here through the
/// same publisher the gateway uses, with the threshold driven to zero so every
/// body takes that path.
#[cfg(not(coverage))]
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
        NatsObjectStore::bind_claim_bucket(&js, ClaimBucket::default())
            .await
            .expect("bind claim bucket")
            .into_store(),
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
    let port = FakePort::new(renderer.clone(), "hi there");
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

    let msg = next_message(&mut messages).await;
    // The premise of the test: parsing what arrived would have failed.
    assert!(msg.payload.is_empty());
    pipeline.handle_message(&msg).await.expect("handled");

    assert_eq!(
        *port.prompted.borrow(),
        vec![("sess-1".to_string(), "hello from a big body".to_string())]
    );
    assert_eq!(*outbound.sent.borrow(), vec![(42, "hi there".to_string())]);

    let info = settled_consumer_info(&stream, "bridge-test", 0).await;
    assert_eq!(info.num_ack_pending, 0);
    assert_eq!(info.num_pending, 0);
}

/// A claim that cannot be redeemed is left for redelivery instead of acked.
/// Dropping it would be permanent, and the payload alone carries no sign that
/// anything was lost.
#[cfg(not(coverage))]
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
        NatsObjectStore::bind_claim_bucket(&js, ClaimBucket::default())
            .await
            .expect("bind claim bucket")
            .into_store(),
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
    let port = FakePort::new(renderer.clone(), "hi there");
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

    let msg = next_message(&mut messages).await;
    assert!(pipeline.handle_message(&msg).await.is_err(), "must not be acked");

    // Where the failure came from: the claim never resolved, so the agent was
    // never reached.
    assert!(port.prompted.borrow().is_empty());
    assert!(outbound.sent.borrow().is_empty());

    let info = stream.consumer_info("bridge-test").await.expect("consumer info");
    assert_eq!(info.num_ack_pending, 1);
}

/// A suspected lost session is repaired without betting the conversation on the
/// suspicion. "Session lost" is a guess drawn from an error code that also
/// covers ordinary rejections, so the fresh session has to answer before the
/// conversation points at it: a prompt the agent simply refuses must leave the
/// conversation on the session it already had, with its history, rather than
/// rotating it onto a new one and failing anyway. One container for the whole
/// scenario.
#[tokio::test]
async fn pipeline_keeps_the_session_when_a_fresh_one_fails_the_same_way() {
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

    for (update_id, text) in [(1u64, "hello"), (2, "refused"), (3, "after"), (4, "recover")] {
        js.publish("telegram.message", raw_update(update_id, 42, 42, text).into())
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
    let port = FakePort::new(renderer.clone(), "hi there");
    let outbound = FakeOutbound::default();
    let triggers = CommandTriggers::default();
    let claims = unclaimed_resolver();
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

    pipeline
        .handle_message(&next_message(&mut messages).await)
        .await
        .expect("handled");
    let session_before = store
        .conversation_for(&endpoint)
        .await
        .expect("kv read")
        .expect("conversation exists")
        .1
        .current_session;
    assert_eq!(session_before.as_ref().map(AgentSessionId::as_str), Some("sess-1"));

    // Both the original session and the fresh one reject this prompt, which is
    // what an ordinary rejection misread as a lost session looks like.
    port.reject_next_prompts(2);
    assert!(
        pipeline
            .handle_message(&next_message(&mut messages).await)
            .await
            .is_err(),
        "a prompt the agent will not answer must not be acked"
    );

    // The point of the test: the conversation still holds the session it had,
    // and the session nobody got to use was handed back.
    let (_, record) = store
        .conversation_for(&endpoint)
        .await
        .expect("kv read")
        .expect("conversation exists");
    assert_eq!(record.current_session, session_before);
    assert_eq!(
        *port.released.borrow(),
        vec![("sess-2".to_string(), ReleaseReason::RepairFailed)]
    );

    // So the next message continues on it rather than starting over.
    pipeline
        .handle_message(&next_message(&mut messages).await)
        .await
        .expect("handled");

    // A session the agent really has forgotten is still repaired: the fresh one
    // answers, so the conversation moves onto it.
    port.reject_next_prompts(1);
    pipeline
        .handle_message(&next_message(&mut messages).await)
        .await
        .expect("handled");

    assert_eq!(*port.sessions_created.borrow(), 3);
    assert_eq!(
        *port.prompted.borrow(),
        vec![
            ("sess-1".to_string(), "hello".to_string()),
            ("sess-1".to_string(), "refused".to_string()),
            ("sess-2".to_string(), "refused".to_string()),
            ("sess-1".to_string(), "after".to_string()),
            ("sess-1".to_string(), "recover".to_string()),
            ("sess-3".to_string(), "recover".to_string()),
        ]
    );
    let (_, record) = store
        .conversation_for(&endpoint)
        .await
        .expect("kv read")
        .expect("conversation exists");
    assert_eq!(
        record.current_session.as_ref().map(AgentSessionId::as_str),
        Some("sess-3")
    );

    // The replaced session is handed back too. `is_session_lost` is a guess, so
    // the agent may still have had `sess-1`; without this a wrong guess orphans
    // a live session that nothing points at any more.
    assert_eq!(
        *port.released.borrow(),
        vec![
            ("sess-2".to_string(), ReleaseReason::RepairFailed),
            ("sess-1".to_string(), ReleaseReason::Replaced),
        ]
    );

    assert_eq!(*outbound.typing.borrow(), 4);
    assert_eq!(
        *outbound.sent.borrow(),
        vec![
            (42, "hi there".to_string()),
            (42, "hi there".to_string()),
            (42, "hi there".to_string()),
        ]
    );

    // The rejected message is left for redelivery, so the prompt is retried
    // instead of lost to a rotation that did not help. Outbound replies stay
    // clean: any partial text streamed before the failure was discarded rather
    // than joined onto the next successful turn.
    let info = settled_consumer_info(&stream, "bridge-test", 1).await;
    assert_eq!(info.num_ack_pending, 1);
    assert_eq!(info.num_pending, 0);
}

/// Everything the bridge cannot act on is acked and dropped rather than left to
/// redeliver: none of it will parse, authorize, or route any better the second
/// time, so redelivering it would wedge the consumer behind a message that can
/// never succeed. One container for the whole scenario.
#[tokio::test]
async fn pipeline_acks_and_drops_what_no_redelivery_would_fix() {
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
    let group = Endpoint::new("telegram", "mybot", "-1001").expect("group endpoint");
    for linked in [&endpoint, &group] {
        store
            .link_endpoint(&principal, &PrincipalRecord { display_name: None }, linked)
            .await
            .expect("seed principal");
    }

    // Not JSON at all: the gateway carries raw bodies, so a malformed one reaches
    // the bridge intact and no amount of retrying will make it parse.
    js.publish("telegram.message", b"not a telegram update".to_vec().into())
        .await
        .expect("publish")
        .await
        .expect("ack");
    for body in [
        // An update kind the bridge does not carry.
        raw_edit(2, 42, 42, "edited"),
        // A reset before this endpoint has ever had a session.
        raw_update(3, 42, 42, "/new"),
        // A group member the chat authorizes but the store does not know: the
        // command is refused, and since the trigger was the whole message there
        // is nothing left to forward to the agent.
        raw_group_update(4, -1001, 777, "/new"),
    ] {
        js.publish("telegram.message", body.into())
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
    let port = FakePort::new(renderer.clone(), "hi there");
    let outbound = FakeOutbound::default();
    let triggers = CommandTriggers::default();
    let claims = unclaimed_resolver();
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

    for _ in 0..4 {
        pipeline
            .handle_message(&next_message(&mut messages).await)
            .await
            .expect("dropped rather than returned as an error");
    }

    // None of the four reached the agent, so nothing opened a session.
    assert!(port.prompted.borrow().is_empty());
    assert_eq!(*port.sessions_created.borrow(), 0);
    assert!(port.released.borrow().is_empty());

    // Only the reset from a linked sender is answered. Resetting a conversation
    // that has no session is not an error, so it is acknowledged like any other.
    assert_eq!(
        *outbound.sent.borrow(),
        vec![(42, "Started a new session.".to_string())]
    );

    // The conversation the reset created is still bound and still sessionless.
    let (_, record) = store
        .conversation_for(&endpoint)
        .await
        .expect("kv read")
        .expect("conversation exists");
    assert_eq!(record.current_session, None);

    // The refused command left the group's conversation intact: the sender was
    // not authorized for the command, which says nothing about the chat.
    assert!(store.conversation_for(&group).await.expect("kv read").is_some());

    // A bot account that is not an endpoint token can build no sender endpoint
    // at all, so it authorizes nobody rather than authorizing everybody. Only
    // reachable by calling in directly: `parse::inbound_event` rejects the same
    // account earlier, so no update can carry a message this far.
    let misconfigured = Pipeline {
        bot_account: "my bot",
        ..pipeline
    };
    let event = InboundEvent {
        endpoint: endpoint.clone(),
        sender: Sender {
            platform_user_id: PlatformUserId::new("42").expect("id"),
            display_name: "Test".to_string(),
        },
        text: None,
        command: Some(Command::NewSession),
        attachments: Vec::new(),
        message_ref: MessageRef::new("1").expect("message ref"),
        occurred_at: 1_700_000_000,
    };
    assert!(
        !misconfigured
            .sender_is_authorized(&event)
            .await
            .expect("the store is readable; only the endpoint cannot be built"),
        "a sender whose endpoint cannot be built must not be authorized"
    );

    let info = settled_consumer_info(&stream, "bridge-test", 0).await;
    assert_eq!(info.num_ack_pending, 0);
    assert_eq!(info.num_pending, 0);
}

/// A turn that fails leaves nothing behind for the next one. The agent may have
/// streamed part of a reply before failing, and the message is going to be
/// redelivered, so any buffered text has to be dropped or the retry would send
/// the failed turn's fragment glued to the front of the real answer. One
/// container for the whole scenario.
#[tokio::test]
async fn pipeline_leaves_no_partial_reply_behind_when_a_turn_fails() {
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

    for (update_id, text) in [(1u64, "hello"), (2, "refused"), (3, "dead end"), (4, "quiet")] {
        js.publish("telegram.message", raw_update(update_id, 42, 42, text).into())
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
    let port = FakePort::new(renderer.clone(), "hi there");
    let outbound = FakeOutbound::default();
    let triggers = CommandTriggers::default();
    let claims = unclaimed_resolver();
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

    pipeline
        .handle_message(&next_message(&mut messages).await)
        .await
        .expect("handled");

    // An ordinary agent failure, which is not a suspected lost session: the
    // conversation must stay on the session it has and simply be retried.
    port.refuse_next_prompts(1);
    let refused = pipeline.handle_message(&next_message(&mut messages).await).await;
    assert!(
        matches!(refused, Err(PipelineError::Prompt { .. })),
        "an agent failure must surface as a prompt failure: {refused:?}"
    );

    // A suspected lost session with no fresh session to be had. Nothing is
    // committed, because nothing answered.
    port.reject_next_prompts(1);
    port.fail_next_session_creations(1);
    let dead_end = pipeline.handle_message(&next_message(&mut messages).await).await;
    assert!(
        matches!(dead_end, Err(PipelineError::CreateSession(_))),
        "a repair with no session to open must surface as a creation failure: {dead_end:?}"
    );

    // An agent that ends a turn without saying anything, which is what acting
    // only through tool calls looks like. Acked: the turn did complete.
    port.stay_silent_for_next_turns(1);
    pipeline
        .handle_message(&next_message(&mut messages).await)
        .await
        .expect("a silent turn is still a completed turn");

    // The point of the test: every reply the user saw is a whole reply. Neither
    // failure sent the `partial-` fragment it streamed, and the silent turn sent
    // nothing rather than the fragment left by the turn before it.
    assert_eq!(*outbound.sent.borrow(), vec![(42, "hi there".to_string())]);

    // Neither failure rotated the conversation: no session was handed back, none
    // was minted beyond the first, and the pointer never moved.
    assert!(port.released.borrow().is_empty());
    assert_eq!(*port.sessions_created.borrow(), 1);
    assert_eq!(
        *port.prompted.borrow(),
        vec![
            ("sess-1".to_string(), "hello".to_string()),
            ("sess-1".to_string(), "refused".to_string()),
            ("sess-1".to_string(), "dead end".to_string()),
            ("sess-1".to_string(), "quiet".to_string()),
        ]
    );
    let (_, record) = store
        .conversation_for(&endpoint)
        .await
        .expect("kv read")
        .expect("conversation exists");
    assert_eq!(
        record.current_session.as_ref().map(AgentSessionId::as_str),
        Some("sess-1")
    );

    // Both failures are left for redelivery; the silent turn is not.
    let info = settled_consumer_info(&stream, "bridge-test", 2).await;
    assert_eq!(info.num_ack_pending, 2);
    assert_eq!(info.num_pending, 0);
}
