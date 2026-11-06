use std::num::NonZeroU64;
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::jetstream::kv;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use trogon_audit::AuditName;
use trogon_nats::NatsToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::*;

#[tokio::test]
async fn rejecting_a_candidate_withdraws_before_the_state_lock_is_released() {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let config = KvPolicyConfig::new(
        "egress_policy",
        AuditName::new("acme").unwrap(),
        NatsToken::new("payments").unwrap(),
        NonZeroU64::new(1).unwrap(),
    )
    .unwrap();
    let store = async_nats::jetstream::new(client.clone())
        .create_key_value(kv::Config {
            bucket: config.bucket().to_owned(),
            max_value_size: LIVE_POLICY_MAX_BYTES as i32,
            max_bytes: LIVE_POLICY_MAX_STORE_BYTES,
            max_age: LIVE_POLICY_MAX_LIFETIME,
            ..kv::Config::default()
        })
        .await
        .unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut document = serde_json::json!({"tenant":"acme", "project":"payments", "incarnation":1, "generation":1, "issued_at":now, "valid_until":now+30, "policy":"[agents.agent-1]\nallow=[\"api.example.com:443\"]\n"});
    store
        .put(config.key(), serde_json::to_vec(&document).unwrap().into())
        .await
        .unwrap();
    let (snapshot, _driver) = KvSnapshot::open(client, config.clone()).await.unwrap();
    document["tenant"] = serde_json::json!("another");
    document["generation"] = serde_json::json!(2);
    store
        .put(config.key(), serde_json::to_vec(&document).unwrap().into())
        .await
        .unwrap();
    let candidate = store
        .stream
        .get_last_raw_message_by_subject(&format!("$KV.{}.{}", config.bucket(), config.key()))
        .await
        .unwrap();
    assert!(matches!(snapshot.apply(candidate), Err(LivePolicyError::Scope)));
    assert_eq!(
        snapshot.lookup(
            &AgentId::new("agent-1").unwrap(),
            &"api.example.com:443".parse().unwrap(),
            SystemTime::now()
        ),
        SnapshotVerdict::Unavailable
    );
}

struct PendingSource {
    _server: JetStreamTestServer,
    client: async_nats::Client,
    config: KvPolicyConfig,
    store: kv::Store,
    snapshot: Arc<KvSnapshot>,
    driver: Option<KvPolicyDriver>,
}

impl PendingSource {
    async fn new() -> Self {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let config = KvPolicyConfig::new(
            "egress_policy",
            AuditName::new("acme").unwrap(),
            NatsToken::new("payments").unwrap(),
            NonZeroU64::MIN,
        )
        .unwrap();
        let store = async_nats::jetstream::new(client.clone())
            .create_key_value(Self::bucket())
            .await
            .unwrap();
        store
            .put(config.key(), serde_json::to_vec(&Self::document(1)).unwrap().into())
            .await
            .unwrap();
        let (snapshot, driver) = KvSnapshot::open(client.clone(), config.clone()).await.unwrap();
        Self {
            _server: server,
            client,
            config,
            store,
            snapshot,
            driver: Some(driver),
        }
    }

    fn bucket() -> kv::Config {
        kv::Config {
            bucket: "egress_policy".into(),
            max_value_size: LIVE_POLICY_MAX_BYTES as i32,
            max_bytes: LIVE_POLICY_MAX_STORE_BYTES,
            max_age: LIVE_POLICY_MAX_LIFETIME,
            ..kv::Config::default()
        }
    }

    fn document(generation: u64) -> serde_json::Value {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        serde_json::json!({"tenant":"acme", "project":"payments", "incarnation":1, "generation":generation, "issued_at":now-60, "valid_until":now+30, "policy":"[agents.agent-1]\nallow=[\"api.example.com:443\"]\n"})
    }

    async fn candidate(&self, document: serde_json::Value) -> StreamMessage {
        self.store
            .put(self.config.key(), serde_json::to_vec(&document).unwrap().into())
            .await
            .unwrap();
        self.current_message(self.config.key()).await
    }

    async fn current_message(&self, key: &str) -> StreamMessage {
        self.store
            .stream
            .get_last_raw_message_by_subject(&format!("$KV.{}.{}", self.config.bucket(), key))
            .await
            .unwrap()
    }

    fn verdict(&self) -> SnapshotVerdict {
        self.snapshot.lookup(
            &AgentId::new("agent-1").unwrap(),
            &"api.example.com:443".parse().unwrap(),
            SystemTime::now(),
        )
    }
}

#[tokio::test]
async fn stale_or_equivocating_broker_replies_cannot_restore_authority() {
    let fixture = PendingSource::new().await;
    let first = fixture.current_message(fixture.config.key()).await;
    let second = fixture.candidate(PendingSource::document(2)).await;
    fixture.snapshot.apply(second.clone()).unwrap();
    assert!(matches!(
        fixture.snapshot.apply(first),
        Err(LivePolicyError::Regression)
    ));
    assert_eq!(fixture.verdict(), SnapshotVerdict::Unavailable);
    let mut equivocation = second;
    equivocation.payload = serde_json::to_vec(&PendingSource::document(3)).unwrap().into();
    assert!(matches!(
        fixture.snapshot.apply(equivocation),
        Err(LivePolicyError::Regression)
    ));
    assert_eq!(fixture.verdict(), SnapshotVerdict::Unavailable);
    fixture
        .snapshot
        .apply(fixture.candidate(PendingSource::document(4)).await)
        .unwrap();
    assert_eq!(fixture.verdict(), SnapshotVerdict::Permitted);
}

#[tokio::test]
async fn an_out_of_scope_raw_message_cannot_become_the_scoped_snapshot() {
    let fixture = PendingSource::new().await;
    fixture
        .store
        .put(
            "another_scope",
            serde_json::to_vec(&PendingSource::document(2)).unwrap().into(),
        )
        .await
        .unwrap();
    let wrong = fixture.current_message("another_scope").await;
    assert!(matches!(fixture.snapshot.apply(wrong), Err(LivePolicyError::Removed)));
    assert_eq!(fixture.verdict(), SnapshotVerdict::Unavailable);
}

#[tokio::test]
async fn the_payload_bound_survives_a_broker_configuration_change_after_the_info_read() {
    let fixture = PendingSource::new().await;
    let mut widened = PendingSource::bucket();
    widened.max_value_size = (LIVE_POLICY_MAX_BYTES + 1) as i32;
    async_nats::jetstream::new(fixture.client.clone())
        .update_key_value(widened)
        .await
        .unwrap();
    fixture
        .store
        .put(fixture.config.key(), vec![b' '; LIVE_POLICY_MAX_BYTES + 1].into())
        .await
        .unwrap();
    let oversized = fixture.current_message(fixture.config.key()).await;
    assert!(matches!(
        fixture.snapshot.apply(oversized),
        Err(LivePolicyError::Bounds)
    ));
    assert_eq!(fixture.verdict(), SnapshotVerdict::Unavailable);
}

#[tokio::test]
async fn a_candidate_that_expires_during_validation_cannot_advance_accepted_authority() {
    let fixture = PendingSource::new().await;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut expired = PendingSource::document(2);
    expired["issued_at"] = serde_json::json!(now - 30);
    expired["valid_until"] = serde_json::json!(now - 5);
    let candidate = fixture.candidate(expired).await;
    {
        let mut state = fixture.snapshot.state().unwrap();
        let observed_before_expiry = UNIX_EPOCH + Duration::from_secs(now - 20);
        assert!(matches!(
            fixture
                .snapshot
                .apply_to_state(&mut state, candidate, Instant::now(), observed_before_expiry),
            Err(LivePolicyError::Inactive)
        ));
        assert_eq!(state.generation, 1);
    }
    let candidate = fixture.candidate(PendingSource::document(3)).await;
    let mut state = fixture.snapshot.state().unwrap();
    assert!(matches!(
        fixture.snapshot.apply_to_state(
            &mut state,
            candidate,
            Instant::now() - Duration::from_secs(60),
            SystemTime::now()
        ),
        Err(LivePolicyError::Inactive)
    ));
    assert_eq!(state.generation, 1);
}

#[tokio::test]
async fn poisoned_authority_is_unavailable_and_notifies_session_monitors() {
    let events = CapturedEvents::new();
    let _capture = events.install(LevelFilter::WARN);
    let mut fixture = PendingSource::new().await;
    let mut changes = fixture.snapshot.subscribe().unwrap();
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _held = fixture.snapshot.state.lock().unwrap();
        std::panic::resume_unwind(Box::new(()));
    }));
    assert!(poisoned.is_err());
    assert_eq!(fixture.verdict(), SnapshotVerdict::Unavailable);
    assert_eq!(fixture.snapshot.statistics(), None);
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(fixture.driver.take().unwrap().run(shutdown.clone()));
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, changes.changed())
        .await
        .unwrap()
        .unwrap();
    wait_reason(&events, "state").await;
    shutdown.cancel();
    task.await.unwrap();
}

async fn wait_reason(events: &CapturedEvents, reason: &str) {
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, async {
        loop {
            if events.events().iter().any(|event| {
                event.message() == Some("live egress policy unavailable") && event.field("reason") == Some(reason)
            }) {
                break;
            }
            tokio::time::sleep(crate::constants::LIVE_POLICY_TEST_POLL).await;
        }
    })
    .await
    .expect("the operational failure must be observable");
}

#[tokio::test]
async fn a_connected_peer_without_server_identity_metadata_is_not_an_authority_source() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await?;
        let (read, mut write) = socket.into_split();
        write.write_all(b"INFO {\"server_id\":\"\",\"client_id\":0,\"version\":\"2.10.14\",\"proto\":1,\"max_payload\":1048576}\r\n").await?;
        let mut read = tokio::io::BufReader::new(read);
        let mut line = String::new();
        loop {
            line.clear();
            if read.read_line(&mut line).await? == 0 {
                break;
            }
            if line == "PING\r\n" {
                write.write_all(b"PONG\r\n").await?;
            }
        }
        Ok::<(), std::io::Error>(())
    });
    let client = tokio::time::timeout(
        crate::constants::LIVE_POLICY_TEST_WAIT,
        async_nats::connect(address.to_string()),
    )
    .await
    .unwrap()
    .unwrap();
    let control = client.clone();
    let config = KvPolicyConfig::new(
        "egress_policy",
        AuditName::new("acme").unwrap(),
        NatsToken::new("payments").unwrap(),
        NonZeroU64::MIN,
    )
    .unwrap();
    let error = KvSnapshot::open(client, config).await.err().unwrap();
    control.drain().await.unwrap();
    let closed = tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, server)
        .await
        .unwrap()
        .unwrap();
    assert!(
        closed.is_ok()
            || closed.is_err_and(|error| matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ))
    );
    assert!(matches!(error, LivePolicyError::Disconnected));
}

mod timeout_tests;

#[tokio::test]
async fn a_reconnect_between_observations_withdraws_and_notifies_the_cached_authority() {
    let fixture = PendingSource::new().await;
    let mut changes = fixture.snapshot.subscribe().unwrap();
    let before = fixture.client.server_info().client_id;
    fixture.client.force_reconnect().await.unwrap();
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, async {
        while fixture.client.connection_state() != ConnectionState::Connected
            || fixture.client.server_info().client_id == before
        {
            tokio::time::sleep(crate::constants::LIVE_POLICY_TEST_POLL).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.verdict(), SnapshotVerdict::Unavailable);
    tokio::time::timeout(crate::constants::LIVE_POLICY_TEST_WAIT, changes.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.snapshot.statistics(), None);
    fixture
        .snapshot
        .apply(fixture.candidate(PendingSource::document(2)).await)
        .unwrap();
    assert_eq!(fixture.verdict(), SnapshotVerdict::Permitted);
}
