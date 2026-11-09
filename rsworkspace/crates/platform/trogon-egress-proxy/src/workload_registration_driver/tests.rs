use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use futures::StreamExt;
use trogon_audit::{AuditName, WriterId, WriterIdentity};
use trogon_nats::test_support::TlsTestServer;
use trogon_nats::{NatsConfig, NatsToken};
use trogon_std::env::InMemoryEnv;
use trogon_workload_identity::test_support::{TestCa, WorkloadApiFixture};
use trogon_workload_identity::{ActivationAudit, ActivationAuditError, ActivationFact};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

use super::*;
use crate::audit_config::AuditConfig;
use crate::constants::LIVE_POLICY_TEST_WAIT as WAIT;
use crate::trust_domain::TrustDomain;

mod readiness_tests;

struct Acknowledged;

#[async_trait::async_trait]
impl ActivationAudit for Acknowledged {
    async fn record(&self, _: &ActivationFact) -> Result<(), ActivationAuditError> {
        Ok(())
    }
}

struct Fixture {
    _broker: Option<TlsTestServer>,
    context: async_nats::jetstream::Context,
    client: async_nats::Client,
    config: WorkloadIdentityConfig,
    identity: IdentityStore,
    store: Store,
    wire: RegistrationSnapshot,
    driver: RegistrationDriver,
    api: WorkloadApiFixture,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let broker = TlsTestServer::start_jetstream_verifying_clients().await;
        let env = InMemoryEnv::new();
        env.set("NATS_URL", format!("nats://{}", broker.address()));
        env.set("NATS_CA", broker.ca().to_string_lossy());
        env.set("NATS_CERT", broker.client_cert().to_string_lossy());
        env.set("NATS_KEY", broker.client_key().to_string_lossy());
        let nats = NatsConfig::from_env(&env);
        let admin = trogon_nats::connect_once(&nats, nats.auth.clone(), WAIT).await.unwrap();
        let client = trogon_nats::connect(&nats, WAIT).await.unwrap();
        client.flush().await.unwrap();
        let context = async_nats::jetstream::new(admin);
        let ca = TestCa::new();
        let local = "spiffe://trogon.example/service/egress";
        let api = WorkloadApiFixture::start(vec![ca.issue(local, Duration::from_secs(120))]).await;
        let audit = AuditConfig {
            nats,
            identity: WriterIdentity::new(
                WriterId::new("egress-proxy:registration-fixture").unwrap(),
                NatsToken::new("egress_proxy").unwrap(),
                AuditName::new("acme").unwrap(),
                NatsToken::new("payments").unwrap(),
            ),
        };
        let config = WorkloadIdentityConfig::new(
            &api.endpoint().as_uri(),
            local,
            "REGISTRATIONS",
            NonZeroU64::MIN,
            &TrustDomain::new("trogon.example").unwrap(),
            &audit,
        )
        .unwrap();
        let store = context
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: config.registration_bucket().into(),
                max_value_size: REGISTRATION_MAX_BYTES as i32,
                max_bytes: LIVE_POLICY_MAX_STORE_BYTES,
                max_age: Duration::from_secs(330),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut profile = store.stream.get_info().await.unwrap().config;
        profile.subjects = vec![format!("$KV.{}.{}", store.name, config.registration_key())];
        profile.max_messages = 1;
        profile.discard = async_nats::jetstream::stream::DiscardPolicy::Old;
        context.update_stream(profile).await.unwrap();
        let now = SystemTime::now();
        let wire = RegistrationSnapshot {
            tenant: "acme".into(),
            project: "payments".into(),
            recovery_fence: 1,
            registration_revision: 1,
            trust_revision: 1,
            issued_at: MessageField::some(Timestamp::from(now)),
            expires_at: MessageField::some(Timestamp::from(now + Duration::from_secs(60))),
            bindings: vec![RegistrationBinding {
                spiffe_id: local.into(),
                tenant: "acme".into(),
                project: "payments".into(),
                role: EnumValue::from(WorkloadRole::EgressProxy),
                application_id: None,
            }],
            approved_root_sha256: vec![ca.root_sha256().to_vec()],
        };
        store
            .put(config.registration_key(), wire.encode_to_vec().into())
            .await
            .unwrap();
        let (identity, source) = IdentityStore::new(config.identity().clone(), Arc::new(Acknowledged));
        let driver = RegistrationDriver::open(client.clone(), config.clone(), identity.clone())
            .await
            .unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(source.run(shutdown.clone()));
        let fixture = Self {
            _broker: Some(broker),
            context,
            client,
            config,
            identity,
            store,
            wire,
            driver,
            api,
            shutdown,
            task,
        };
        fixture.usable().await;
        fixture
    }

    async fn usable(&self) {
        tokio::time::timeout(WAIT, async {
            while self.identity.material().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("current authenticated material");
    }

    async fn renew(&mut self) {
        self.wire.registration_revision += 1;
        self.store
            .put(self.config.registration_key(), self.wire.encode_to_vec().into())
            .await
            .unwrap();
        self.driver
            .refresh()
            .await
            .expect("new governed revision recovers authority");
        self.usable().await;
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.unwrap();
        self.api.stop().await;
    }

    async fn relay(&mut self) -> Relay {
        let admin = self.context.client();
        let mut requests = admin.subscribe("registration_api.>").await.unwrap();
        admin.flush().await.unwrap();
        let fault = Arc::new(Mutex::new(Fault::Pass));
        let selected = Arc::clone(&fault);
        let reader = self.client.clone();
        let task = tokio::spawn(async move {
            while let Some(request) = requests.next().await {
                let current = *selected.lock().unwrap();
                let get = request.subject.as_str().contains("STREAM.MSG.GET");
                if current == Fault::Silent && get {
                    continue;
                }
                let suffix = request.subject.as_str().strip_prefix("registration_api.").unwrap();
                let reply = admin
                    .request(format!("$JS.API.{suffix}"), request.payload)
                    .await
                    .unwrap();
                let mut body: serde_json::Value = serde_json::from_slice(&reply.payload).unwrap();
                match current {
                    Fault::Incarnation if !get => body["created"] = serde_json::json!("2000-01-01T00:00:00Z"),
                    Fault::Regression if get => body["message"]["seq"] = serde_json::json!(0),
                    Fault::Equivocation if get => body["message"]["data"] = serde_json::json!("AA=="),
                    Fault::Oversized if get => {
                        body["message"]["seq"] = serde_json::json!(body["message"]["seq"].as_u64().unwrap() + 1);
                        body["message"]["data"] = serde_json::json!("A".repeat((REGISTRATION_MAX_BYTES / 3 + 1) * 4));
                    }
                    Fault::WrongSubject if get => {
                        body["message"]["subject"] = serde_json::json!("$KV.REGISTRATIONS.other")
                    }
                    Fault::MissingInfo if !get => body = missing(),
                    Fault::MissingEntry if get => body = missing(),
                    Fault::DisconnectDuringRead if get => {
                        let before = RegistrationConnection::current(&reader).unwrap();
                        reader.force_reconnect().await.unwrap();
                        changed_connection(&reader, &before).await;
                        async_nats::jetstream::new(reader.clone())
                            .get_stream("KV_REGISTRATIONS")
                            .await
                            .unwrap();
                    }
                    _ => {}
                }
                admin
                    .publish(request.reply.unwrap(), serde_json::to_vec(&body).unwrap().into())
                    .await
                    .unwrap();
            }
        });
        self.context.query_account().await.unwrap();
        let store = async_nats::jetstream::with_prefix(self.client.clone(), "registration_api")
            .get_key_value(self.config.registration_bucket())
            .await
            .unwrap();
        self.driver.stream = store;
        Relay { fault, task }
    }
}

fn missing() -> serde_json::Value {
    serde_json::json!({"error":{"code":404,"err_code":10059,"description":"fixture entry unavailable"}})
}

async fn changed_connection(client: &async_nats::Client, before: &RegistrationConnection) {
    tokio::time::timeout(WAIT, async {
        while !RegistrationConnection::current(client).is_ok_and(|current| &current != before) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual native replacement connection");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    Pass,
    Incarnation,
    Regression,
    Equivocation,
    Oversized,
    WrongSubject,
    MissingInfo,
    MissingEntry,
    DisconnectDuringRead,
    Silent,
}

struct Relay {
    fault: Arc<Mutex<Fault>>,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    fn select(&self, fault: Fault) {
        *self.fault.lock().unwrap() = fault;
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn authenticated_registration_replies_cannot_regress_equivocate_or_cross_a_stream_incarnation() {
    let mut fixture = Fixture::new().await;
    let relay = fixture.relay().await;
    for (fault, reason) in [
        (Fault::Incarnation, "incarnation"),
        (Fault::Regression, "revision"),
        (Fault::Equivocation, "revision"),
        (Fault::WrongSubject, "removed"),
        (Fault::MissingInfo, "transport"),
        (Fault::MissingEntry, "transport"),
        (Fault::Silent, "timeout"),
    ] {
        relay.select(fault);
        let error = fixture
            .driver
            .refresh()
            .await
            .expect_err("invalid authority must fail closed");
        assert_eq!(error.reason(), reason);
        assert!(fixture.identity.material().is_none());
        relay.select(Fault::Pass);
        fixture.renew().await;
    }
    relay.select(Fault::Oversized);
    let error = fixture.driver.refresh().await.expect_err("oversized broker response");
    assert!(matches!(
        error,
        RegistrationSourceError::Projection(RegistrationProjectionError::TooLarge)
    ));
    assert_eq!(error.reason(), "invalid_projection");
    assert!(fixture.identity.material().is_none());
    relay.select(Fault::Pass);
    fixture.wire.registration_revision += 1;
    fixture
        .store
        .put(fixture.config.registration_key(), fixture.wire.encode_to_vec().into())
        .await
        .unwrap();
    assert!(matches!(
        fixture.driver.refresh().await,
        Err(RegistrationSourceError::Revision)
    ));
    assert!(fixture.identity.material().is_none());
    fixture.renew().await;
    drop(relay);
    drop(fixture._broker.take().unwrap());
    tokio::time::timeout(WAIT, async {
        while fixture.client.connection_state() == State::Connected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        fixture.driver.refresh().await,
        Err(RegistrationSourceError::Disconnected)
    ));
    assert!(fixture.identity.material().is_none());
    fixture.stop().await;
}

#[tokio::test]
async fn registration_sources_reject_another_host_identity_and_unbounded_streams() {
    let fixture = Fixture::new().await;
    let other = trogon_workload_identity::IdentityConfig::new(
        fixture.api.endpoint(),
        fixture.config.identity().local().clone(),
        NonZeroU64::new(2).unwrap(),
    );
    let (identity, _) = IdentityStore::new(other, Arc::new(Acknowledged));
    let error = RegistrationDriver::open(fixture.client.clone(), fixture.config.clone(), identity)
        .await
        .err()
        .unwrap();
    assert!(matches!(
        error,
        RegistrationSourceError::Authority(RegistrationError::Scope)
    ));
    let mut profile = fixture.store.stream.get_info().await.unwrap().config;
    profile.max_messages = 2;
    fixture.context.update_stream(profile).await.unwrap();
    let error = RegistrationDriver::open(fixture.client.clone(), fixture.config.clone(), fixture.identity.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(error.reason(), "configuration");
    assert!(fixture.identity.material().is_none());
    fixture.stop().await;
}

#[tokio::test]
async fn reconnect_between_or_during_reads_requires_a_new_governed_revision() {
    let mut fixture = Fixture::new().await;
    let before = RegistrationConnection::current(&fixture.client).unwrap();
    fixture.client.force_reconnect().await.unwrap();
    changed_connection(&fixture.client, &before).await;
    let error = fixture
        .driver
        .refresh()
        .await
        .expect_err("the old revision cannot reactivate after reconnect");
    assert_eq!(error.reason(), "authority");
    assert!(fixture.identity.material().is_none());
    fixture.renew().await;
    let relay = fixture.relay().await;
    relay.select(Fault::DisconnectDuringRead);
    let error = fixture
        .driver
        .refresh()
        .await
        .expect_err("a response from a changed connection is not authority");
    assert!(
        matches!(error, RegistrationSourceError::Disconnected),
        "unexpected rejection: {}",
        error.reason()
    );
    assert_eq!(error.reason(), "disconnected");
    assert!(fixture.identity.material().is_none());
    relay.select(Fault::Pass);
    fixture.renew().await;
    drop(relay);
    fixture.stop().await;
}
