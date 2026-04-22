//! Integration tests — real NATS JetStream + GatewayBridge dispatch.
//!
//! Requires Docker (testcontainers spins up a NATS container with JetStream).
//! JetStream publish_with_headers also delivers to plain core-NATS subscribers,
//! so assertions use `nats.subscribe()` — no JetStream consumer setup needed.
//!
//! Run with:
//!   cargo test -p trogon-source-discord --test gateway_e2e

use std::time::Duration;

use futures_util::StreamExt as _;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, NatsJetStreamClient, NatsObjectStore, StreamMaxAge,
};
use trogon_source_discord::GatewayBridge;
use trogon_source_discord::config::{DiscordBotToken, DiscordConfig};
use trogon_source_discord::constants::{NATS_HEADER_EVENT_NAME, NATS_HEADER_GUILD_ID};
use trogon_source_discord::provision;
use trogon_std::NonZeroDuration;
use twilight_model::gateway::Intents;

// ── Helpers ────────────────────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect failed")
}

fn test_config() -> DiscordConfig {
    DiscordConfig {
        bot_token: DiscordBotToken::new("Bot token").unwrap(),
        intents: Intents::GUILDS,
        subject_prefix: NatsToken::new("discord").unwrap(),
        stream_name: NatsToken::new("DISCORD").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
    }
}

struct TestFixture {
    nats: async_nats::Client,
    bridge: GatewayBridge<NatsJetStreamClient, NatsObjectStore>,
    _container: ContainerAsync<Nats>,
}

async fn setup() -> TestFixture {
    let (container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(nats.clone());

    let object_store = NatsObjectStore::provision(
        &js,
        async_nats::jetstream::object_store::Config {
            bucket: "test-claims".to_string(),
            ..Default::default()
        },
    )
    .await
    .expect("object store provision failed");

    let js_client = NatsJetStreamClient::new(js);
    let config = test_config();
    provision(&js_client, &config)
        .await
        .expect("stream provision failed");

    let publisher = ClaimCheckPublisher::new(
        js_client,
        object_store,
        "test-claims".to_string(),
        MaxPayload::from_server_limit(1024 * 1024),
    );

    let bridge = GatewayBridge::new(publisher, config.subject_prefix, Duration::from_secs(5));

    TestFixture {
        nats,
        bridge,
        _container: container,
    }
}

// ── provision tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn provision_creates_stream_with_correct_config() {
    let (container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js_raw = async_nats::jetstream::new(nats);
    let js = NatsJetStreamClient::new(js_raw.clone());
    let config = test_config();

    provision(&js, &config).await.unwrap();

    let mut stream = js_raw.get_stream("DISCORD").await.unwrap();
    let info = stream.info().await.unwrap();
    assert_eq!(info.config.name, "DISCORD");
    assert!(info.config.subjects.contains(&"discord.>".to_string()));
    assert_eq!(info.config.max_age, Duration::from_secs(3600));

    drop(container);
}

#[tokio::test]
async fn provision_is_idempotent() {
    let (container, nats_port) = start_nats().await;
    let nats = nats_client(nats_port).await;
    let js = NatsJetStreamClient::new(async_nats::jetstream::new(nats));
    let config = test_config();

    provision(&js, &config).await.unwrap();
    provision(&js, &config).await.unwrap();

    drop(container);
}

// ── dispatch tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_publishes_message_create_to_nats() {
    let fixture = setup().await;
    let mut sub = fixture
        .nats
        .subscribe("discord.message_create")
        .await
        .unwrap();

    fixture.bridge.dispatch(
        r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"123","channel_id":"456","content":"hi"}}"#,
    ).await;

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out waiting for NATS message")
        .expect("subscription closed");

    assert_eq!(msg.subject.as_str(), "discord.message_create");
}

#[tokio::test]
async fn dispatch_sets_event_name_header() {
    let fixture = setup().await;
    let mut sub = fixture
        .nats
        .subscribe("discord.guild_create")
        .await
        .unwrap();

    fixture
        .bridge
        .dispatch(r#"{"op":0,"t":"GUILD_CREATE","s":1,"d":{"id":"111","name":"test"}}"#)
        .await;

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    let headers = msg.headers.expect("headers must be present");
    assert_eq!(
        headers.get(NATS_HEADER_EVENT_NAME).map(|v| v.as_str()),
        Some("guild_create")
    );
}

#[tokio::test]
async fn dispatch_sets_guild_id_header_when_present() {
    let fixture = setup().await;
    let mut sub = fixture
        .nats
        .subscribe("discord.message_create")
        .await
        .unwrap();

    fixture.bridge.dispatch(
        r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"1","guild_id":"999","channel_id":"2"}}"#,
    ).await;

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    let headers = msg.headers.expect("headers must be present");
    assert_eq!(
        headers.get(NATS_HEADER_GUILD_ID).map(|v| v.as_str()),
        Some("999")
    );
}

#[tokio::test]
async fn dispatch_omits_guild_id_header_when_absent() {
    let fixture = setup().await;
    let mut sub = fixture.nats.subscribe("discord.user_update").await.unwrap();

    fixture
        .bridge
        .dispatch(r#"{"op":0,"t":"USER_UPDATE","s":1,"d":{"id":"1","username":"alice"}}"#)
        .await;

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    let no_guild_id = msg
        .headers
        .as_ref()
        .and_then(|h| h.get(NATS_HEADER_GUILD_ID))
        .is_none();
    assert!(no_guild_id);
}

#[tokio::test]
async fn dispatch_sets_dedup_id_for_message_create() {
    let fixture = setup().await;
    let mut sub = fixture
        .nats
        .subscribe("discord.message_create")
        .await
        .unwrap();

    fixture
        .bridge
        .dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"msg-abc","channel_id":"456"}}"#)
        .await;

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    let headers = msg.headers.expect("headers must be present");
    assert_eq!(
        headers
            .get(async_nats::header::NATS_MESSAGE_ID)
            .map(|v| v.as_str()),
        Some("message_create:msg-abc")
    );
}

#[tokio::test]
async fn dispatch_lowercases_event_type() {
    let fixture = setup().await;
    let mut sub = fixture
        .nats
        .subscribe("discord.guild_member_add")
        .await
        .unwrap();

    fixture
        .bridge
        .dispatch(r#"{"op":0,"t":"GUILD_MEMBER_ADD","s":1,"d":{"guild_id":"1","user":{"id":"2"}}}"#)
        .await;

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    assert_eq!(msg.subject.as_str(), "discord.guild_member_add");
}

#[tokio::test]
async fn dispatch_non_dispatch_ops_do_not_publish() {
    let fixture = setup().await;
    let mut sub = fixture.nats.subscribe("discord.>").await.unwrap();

    fixture.bridge.dispatch(r#"{"op":11}"#).await;
    fixture
        .bridge
        .dispatch(r#"{"op":10,"d":{"heartbeat_interval":41250}}"#)
        .await;
    fixture.bridge.dispatch(r#"{"op":7}"#).await;

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "non-dispatch ops must not publish to NATS");
}

#[tokio::test]
async fn dispatch_missing_event_type_does_not_publish() {
    let fixture = setup().await;
    let mut sub = fixture.nats.subscribe("discord.>").await.unwrap();

    fixture.bridge.dispatch(r#"{"op":0,"s":1,"d":{}}"#).await;

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "dispatch without event type must not publish");
}

#[tokio::test]
async fn dispatch_invalid_json_does_not_publish() {
    let fixture = setup().await;
    let mut sub = fixture.nats.subscribe("discord.>").await.unwrap();

    fixture.bridge.dispatch("not json at all").await;

    let timed_out = tokio::time::timeout(Duration::from_millis(300), sub.next())
        .await
        .is_err();
    assert!(timed_out, "invalid JSON must not publish");
}

#[tokio::test]
async fn dispatch_forwards_raw_data_payload() {
    let fixture = setup().await;
    let mut sub = fixture
        .nats
        .subscribe("discord.message_create")
        .await
        .unwrap();

    fixture
        .bridge
        .dispatch(r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"id":"123","content":"hello world"}}"#)
        .await;

    let msg = tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .expect("timed out")
        .expect("closed");

    let parsed: serde_json::Value = serde_json::from_slice(&msg.payload).expect("valid json");
    assert_eq!(parsed["id"], "123");
    assert_eq!(parsed["content"], "hello world");
}
