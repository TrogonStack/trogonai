use trogon_channel::{ChannelStore, ChannelStoreError, PrincipalId, SafeToken};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::env::InMemoryEnv;
use trogon_std::log_capture::{CapturedEvents, CapturedLogs, LevelFilter, LogLevel};

use super::{config::BridgeConfig, seed_principals};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::InitializeResponse;
use futures::{StreamExt, stream};
use std::time::Duration;
use teloxide::Bot;
use trogon_nats::jetstream::{ClaimBucket, ClaimResolver, ClaimRetention, NatsObjectStore};

fn config(users: &str) -> BridgeConfig {
    let env = InMemoryEnv::new();
    env.set("TELEGRAM_BOT_TOKEN", "fixture-token");
    env.set("TELEGRAM_BOT_ACCOUNT", "fixturebot");
    env.set("CHANNEL_PREFIX", "seedtest");
    env.set("CHANNEL_SEED_TELEGRAM_USERS", users);
    BridgeConfig::from_env(&env).expect("bridge config")
}

#[tokio::test]
async fn configured_principals_resolve_to_the_same_endpoints_after_restart() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let config = config("42,7,42");
    let store = ChannelStore::ensure(&js, &config.channel_prefix)
        .await
        .expect("channel store");
    seed_principals(&store, &config).await.expect("initial seeding");
    seed_principals(&store, &config).await.expect("restart seeding");
    for user in [42_i64, 7] {
        let endpoint = config.account.endpoint_for(&SafeToken::from(user));
        assert_eq!(
            store.principal_for(&endpoint).await.expect("principal lookup"),
            Some(PrincipalId::new(format!("telegram-{user}")).expect("principal id"))
        );
        assert!(
            store
                .conversation_for(&endpoint)
                .await
                .expect("conversation lookup")
                .is_none()
        );
    }
    let unknown = config.account.endpoint_for(&SafeToken::from(8_i64));
    assert!(
        store
            .principal_for(&unknown)
            .await
            .expect("unknown principal")
            .is_none()
    );
}

#[tokio::test]
async fn empty_seed_configuration_does_not_touch_storage_and_write_failure_propagates() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let empty = config("");
    let store = ChannelStore::ensure(&js, &empty.channel_prefix)
        .await
        .expect("channel store");
    js.delete_key_value("channel_principals_seedtest")
        .await
        .expect("remove principal bucket");
    seed_principals(&store, &empty)
        .await
        .expect("empty seed does not access missing bucket");
    let error = seed_principals(&store, &config("42"))
        .await
        .expect_err("seed write must fail");
    assert!(error.downcast_ref::<ChannelStoreError>().is_some());
}

struct RuntimeFixture {
    _server: JetStreamTestServer,
    nats: async_nats::Client,
    js: async_nats::jetstream::Context,
    store: ChannelStore,
    claims: ClaimResolver<NatsObjectStore>,
    config: BridgeConfig,
}

impl RuntimeFixture {
    async fn new() -> Self {
        let server = JetStreamTestServer::start().await;
        let nats = server.client().await;
        let js = async_nats::jetstream::new(nats.clone());
        let config = config("");
        let store = ChannelStore::ensure(&js, &config.channel_prefix).await.unwrap();
        let claims = ClaimResolver::new(
            NatsObjectStore::provision_claim_bucket(&js, ClaimBucket::default(), ClaimRetention::EventSourced)
                .await
                .unwrap(),
        );
        Self {
            _server: server,
            nats,
            js,
            store,
            claims,
            config,
        }
    }

    async fn accept_initialization(&self) -> tokio::task::JoinHandle<()> {
        let subject = acp_nats::nats::global::InitializeSubject::new(self.config.acp.acp_prefix_ref());
        let mut requests = self.nats.subscribe(subject.to_string()).await.unwrap();
        self.nats.flush().await.unwrap();
        let nats = self.nats.clone();
        tokio::spawn(async move {
            let request = tokio::time::timeout(Duration::from_secs(5), requests.next())
                .await
                .unwrap()
                .unwrap();
            let payload: serde_json::Value = serde_json::from_slice(&request.payload).unwrap();
            assert_eq!(payload["method"], "initialize");
            let response = acp_nats::wire::encode_success(
                acp_nats::wire::response_id_from_request_headers(&request.headers.unwrap_or_default()),
                &InitializeResponse::new(ProtocolVersion::LATEST),
            )
            .unwrap();
            nats.publish_with_headers(request.reply.unwrap(), response.headers, response.body)
                .await
                .unwrap();
            nats.flush().await.unwrap();
        })
    }
}

#[tokio::test]
async fn runtime_initializes_before_stopping_when_inbound_stream_ends() {
    let fixture = RuntimeFixture::new().await;
    let initialized = fixture.accept_initialization().await;
    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::time::timeout(
                Duration::from_secs(5),
                super::run(
                    fixture.nats,
                    fixture.store,
                    fixture.claims,
                    stream::empty(),
                    Bot::new("fixture-token"),
                    fixture.config,
                    std::future::pending(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            initialized.await.unwrap();
        })
        .await;
}

#[tokio::test]
async fn runtime_honors_shutdown_while_inbound_stream_remains_open() {
    let fixture = RuntimeFixture::new().await;
    let initialized = fixture.accept_initialization().await;
    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::time::timeout(
                Duration::from_secs(5),
                super::run(
                    fixture.nats,
                    fixture.store,
                    fixture.claims,
                    stream::pending(),
                    Bot::new("fixture-token"),
                    fixture.config,
                    std::future::ready(()),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            initialized.await.unwrap();
        })
        .await;
}

#[tokio::test]
async fn runtime_continues_after_receive_and_message_processing_errors() {
    let logs = CapturedLogs::isolated();
    let events = CapturedEvents::new();
    let _capture = logs.is_none().then(|| events.install(LevelFilter::WARN));
    let fixture = RuntimeFixture::new().await;
    let initialized = fixture.accept_initialization().await;
    let updates = fixture
        .js
        .create_stream(async_nats::jetstream::stream::Config {
            name: "TELEGRAM_RUNTIME_RECOVERY".into(),
            subjects: vec!["telegram.update".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(trogon_nats::jetstream::HEADER_CLAIM_CHECK, "v1");
    fixture
        .js
        .publish_with_headers("telegram.update", headers, "not-json".into())
        .await
        .unwrap()
        .await
        .unwrap();
    fixture
        .js
        .publish("telegram.update", "not-json".into())
        .await
        .unwrap()
        .await
        .unwrap();
    let consumer = updates
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
            ..Default::default()
        })
        .await
        .unwrap();
    let mut deliveries = consumer.fetch().max_messages(2).messages().await.unwrap();
    let (unredeemable, invalid_message) = tokio::time::timeout(Duration::from_secs(5), async {
        let unredeemable = deliveries.next().await.unwrap().unwrap();
        let invalid_message = deliveries.next().await.unwrap().unwrap();
        (unredeemable, invalid_message)
    })
    .await
    .unwrap();
    assert_eq!(consumer.get_info().await.unwrap().num_ack_pending, 2);
    let messages = stream::iter([
        Err(async_nats::jetstream::consumer::pull::MessagesError::from(
            async_nats::jetstream::consumer::pull::MessagesErrorKind::MissingHeartbeat,
        )),
        Ok(unredeemable),
        Ok(invalid_message),
    ]);
    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::time::timeout(
                Duration::from_secs(5),
                super::run(
                    fixture.nats,
                    fixture.store,
                    fixture.claims,
                    messages,
                    Bot::new("fixture-token"),
                    fixture.config,
                    std::future::pending(),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            initialized.await.unwrap();
        })
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let info = consumer.get_info().await.unwrap();
            if info.num_ack_pending == 1 && info.num_pending == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the unredeemable update remains pending while the malformed update is acknowledged");
    if let Some(logs) = logs {
        let records = logs.records();
        assert!(records.iter().any(|record| {
            record.level == LogLevel::Error
                && record
                    .message
                    .contains("Failed to process update; leaving unacked for redelivery")
                && record.message.contains("MissingKey")
        }));
        assert!(records.iter().any(|record| {
            record.level == LogLevel::Warn && record.message.contains("Unparseable Telegram update; dropping")
        }));
        return;
    }
    let events = events.events();
    assert!(events.iter().any(|event| {
        event.message() == Some("Failed to process update; leaving unacked for redelivery")
            && event.field("error").is_some_and(|error| error.contains("MissingKey"))
    }));
    assert!(
        events
            .iter()
            .any(|event| event.message() == Some("Unparseable Telegram update; dropping"))
    );
}

#[tokio::test]
async fn runtime_reports_unavailable_agent_before_consuming_updates() {
    let fixture = RuntimeFixture::new().await;
    let error = tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::time::timeout(
                Duration::from_secs(5),
                super::run(
                    fixture.nats,
                    fixture.store,
                    fixture.claims,
                    stream::empty(),
                    Bot::new("fixture-token"),
                    fixture.config,
                    std::future::pending(),
                ),
            )
            .await
            .unwrap()
            .unwrap_err()
        })
        .await;
    assert!(error.to_string().starts_with("ACP initialize failed:"));
}

#[tokio::test]
async fn runtime_stops_when_the_client_subscription_is_drained() {
    let fixture = RuntimeFixture::new().await;
    let initialized = fixture.accept_initialization().await;
    let client = fixture.nats.clone();
    let (consuming, ready) = tokio::sync::oneshot::channel();
    let mut consuming = Some(consuming);
    let messages = stream::poll_fn(move |_| {
        if let Some(consuming) = consuming.take() {
            consuming.send(()).unwrap();
        }
        std::task::Poll::Pending
    });
    tokio::task::LocalSet::new()
        .run_until(async move {
            let drain = async {
                initialized.await.unwrap();
                ready.await.unwrap();
                client.drain().await.unwrap();
            };
            let runtime = super::run(
                fixture.nats,
                fixture.store,
                fixture.claims,
                messages,
                Bot::new("fixture-token"),
                fixture.config,
                std::future::pending(),
            );
            let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(runtime, drain) })
                .await
                .unwrap();
            result.unwrap();
        })
        .await;
}
