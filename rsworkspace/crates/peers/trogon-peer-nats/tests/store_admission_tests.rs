use std::time::Duration;

use async_nats::jetstream::{self, stream};
use futures::StreamExt as _;
use serde_json::Value;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer_nats::{
    PeerTenant,
    peer::{
        PeerSnapshotConfigMismatchError, PeerStorageBudget, PeerStoreConfig, PeerStoreError,
        PeerStreamConfigMismatchError, open_store,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const BOUND: Duration = Duration::from_secs(2);

async fn barrier(client: &async_nats::Client) -> TestResult {
    let subject = client.new_inbox();
    let mut incoming = client.subscribe(subject.clone()).await?;
    client.publish(subject, vec![0].into()).await?;
    tokio::time::timeout(BOUND, incoming.next())
        .await?
        .ok_or("broker barrier missing")?;
    Ok(())
}
struct Fixture {
    _broker: JetStreamTestServer,
    client: async_nats::Client,
    config: PeerStoreConfig,
    events: Value,
    snapshots: Value,
    account: Value,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        let broker = JetStreamTestServer::start_with_atomic_batches().await;
        let client = broker.client().await;
        let js = broker.jetstream().await;
        let config = PeerStoreConfig::new(PeerTenant::new("admission")?, PeerStorageBudget::new(65_536)?);
        assert_eq!(config.budget().as_bytes(), 65_536);
        js.create_stream(config.stream_config()).await?;
        let snapshots = js.create_key_value(config.snapshot_config()).await?;
        let raw = client
            .request(
                format!("$JS.API.STREAM.INFO.{}", config.stream_name()),
                Vec::new().into(),
            )
            .await?;
        let events = serde_json::from_slice(&raw.payload)?;
        let raw = client
            .request(
                format!("$JS.API.STREAM.INFO.{}", snapshots.stream.cached_info().config.name),
                Vec::new().into(),
            )
            .await?;
        let snapshots = serde_json::from_slice(&raw.payload)?;
        let raw = client.request("$JS.API.INFO", Vec::new().into()).await?;
        let account = serde_json::from_slice(&raw.payload)?;
        Ok(Self {
            _broker: broker,
            client,
            config,
            events,
            snapshots,
            account,
        })
    }
    async fn rejected(
        &self,
        label: &str,
        event_config: stream::Config,
        snapshot_config: Option<stream::Config>,
    ) -> TestResult<PeerStoreError> {
        let prefix = format!("PEER_FIXTURE_{label}.API");
        let mut events = self.events.clone();
        events["config"] = serde_json::to_value(event_config)?;
        let mut replies = vec![
            (
                format!("{prefix}.STREAM.INFO.{}", self.config.stream_name()),
                serde_json::json!({"error":{"code":404,"err_code":10059,"description":"stream not found"}}),
            ),
            (format!("{prefix}.STREAM.CREATE.{}", self.config.stream_name()), events),
        ];
        if let Some(config) = snapshot_config {
            let mut snapshots = self.snapshots.clone();
            snapshots["config"] = serde_json::to_value(config)?;
            replies.extend([
                (
                    format!("{prefix}.STREAM.INFO.KV_{}", self.config.snapshot_bucket_name()),
                    serde_json::json!({"error":{"code":404,"err_code":10059,"description":"stream not found"}}),
                ),
                (format!("{prefix}.INFO"), self.account.clone()),
                (
                    format!("{prefix}.STREAM.CREATE.KV_{}", self.config.snapshot_bucket_name()),
                    snapshots,
                ),
            ]);
        }
        let mut incoming = self.client.subscribe(format!("{prefix}.>")).await?;
        barrier(&self.client).await?;
        let serve = async {
            for (subject, body) in replies {
                let request = tokio::time::timeout(BOUND, incoming.next())
                    .await?
                    .ok_or("expected provisioning request")?;
                assert_eq!(request.subject.as_str(), subject);
                let reply = request.reply.ok_or("missing provisioning reply")?;
                self.client.publish(reply, serde_json::to_vec(&body)?.into()).await?;
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        let context = jetstream::with_prefix(self.client.clone(), &prefix);
        let (opened, served) = tokio::join!(tokio::time::timeout(BOUND, open_store(context, &self.config)), serve);
        served?;
        match opened? {
            Err(error) => Ok(error),
            Ok(_) => Err("unsafe successful CREATE response was admitted".into()),
        }
    }
}

#[tokio::test]
async fn event_create_replies_cannot_silently_weaken_history_or_tenant_admission() -> TestResult {
    let f = Fixture::new().await?;
    for (label, expected) in [
        ("discard", PeerStreamConfigMismatchError::Discard),
        ("retention", PeerStreamConfigMismatchError::Retention),
        ("bytes", PeerStreamConfigMismatchError::MaxBytes),
        ("age", PeerStreamConfigMismatchError::MaxAge),
        ("messages", PeerStreamConfigMismatchError::MaxMessages),
        ("per_subject", PeerStreamConfigMismatchError::MaxMessagesPerSubject),
        ("atomic", PeerStreamConfigMismatchError::AtomicPublish),
        ("subjects", PeerStreamConfigMismatchError::Subjects),
        ("storage", PeerStreamConfigMismatchError::Storage),
        ("rollup", PeerStreamConfigMismatchError::Rollup),
        ("ttl", PeerStreamConfigMismatchError::MessageTtl),
        ("mirror", PeerStreamConfigMismatchError::Sources),
        ("sources", PeerStreamConfigMismatchError::Sources),
    ] {
        let mut actual = f.config.stream_config();
        match expected {
            PeerStreamConfigMismatchError::Discard => actual.discard = stream::DiscardPolicy::Old,
            PeerStreamConfigMismatchError::Retention => actual.retention = stream::RetentionPolicy::Interest,
            PeerStreamConfigMismatchError::MaxBytes => actual.max_bytes -= 1,
            PeerStreamConfigMismatchError::MaxAge => actual.max_age = Duration::from_secs(1),
            PeerStreamConfigMismatchError::MaxMessages => actual.max_messages = 1,
            PeerStreamConfigMismatchError::MaxMessagesPerSubject => actual.max_messages_per_subject = 1,
            PeerStreamConfigMismatchError::AtomicPublish => actual.allow_atomic_publish = false,
            PeerStreamConfigMismatchError::Subjects => actual.subjects.push("outside.peer.>".into()),
            PeerStreamConfigMismatchError::Storage => actual.storage = stream::StorageType::Memory,
            PeerStreamConfigMismatchError::Rollup => actual.allow_rollup = true,
            PeerStreamConfigMismatchError::MessageTtl => actual.allow_message_ttl = true,
            PeerStreamConfigMismatchError::Sources if label == "mirror" => {
                actual.mirror = Some(stream::Source {
                    name: "outside".into(),
                    ..Default::default()
                })
            }
            PeerStreamConfigMismatchError::Sources => {
                actual.sources = Some(vec![stream::Source {
                    name: "outside".into(),
                    ..Default::default()
                }])
            }
        }
        let error = f.rejected(label, actual, None).await?;
        assert!(
            matches!(error, PeerStoreError::UnsafeConfig(found) if found == expected),
            "{label}: wrong typed admission failure"
        );
    }
    Ok(())
}

#[tokio::test]
async fn snapshot_create_replies_cannot_weaken_retention_bound_or_import_other_buckets() -> TestResult {
    let f = Fixture::new().await?;
    let original: stream::Config = serde_json::from_value(f.snapshots["config"].clone())?;
    for (label, expected) in [
        ("history", PeerSnapshotConfigMismatchError::History),
        ("bytes", PeerSnapshotConfigMismatchError::MaxBytes),
        ("value", PeerSnapshotConfigMismatchError::MaxValueSize),
        ("age", PeerSnapshotConfigMismatchError::MaxAge),
        ("storage", PeerSnapshotConfigMismatchError::Storage),
        ("subjects", PeerSnapshotConfigMismatchError::Subjects),
        ("discard", PeerSnapshotConfigMismatchError::Discard),
        ("mirror", PeerSnapshotConfigMismatchError::Sources),
        ("sources", PeerSnapshotConfigMismatchError::Sources),
    ] {
        let mut actual = original.clone();
        match expected {
            PeerSnapshotConfigMismatchError::History => actual.max_messages_per_subject = 2,
            PeerSnapshotConfigMismatchError::MaxBytes => actual.max_bytes -= 1,
            PeerSnapshotConfigMismatchError::MaxValueSize => actual.max_message_size -= 1,
            PeerSnapshotConfigMismatchError::MaxAge => actual.max_age = Duration::from_secs(1),
            PeerSnapshotConfigMismatchError::Storage => actual.storage = stream::StorageType::Memory,
            PeerSnapshotConfigMismatchError::Subjects => actual.subjects.push("$KV.outside.>".into()),
            PeerSnapshotConfigMismatchError::Discard => actual.discard = stream::DiscardPolicy::Old,
            PeerSnapshotConfigMismatchError::Sources if label == "mirror" => {
                actual.mirror = Some(stream::Source {
                    name: "KV_outside".into(),
                    ..Default::default()
                })
            }
            PeerSnapshotConfigMismatchError::Sources => {
                actual.sources = Some(vec![stream::Source {
                    name: "KV_outside".into(),
                    ..Default::default()
                }])
            }
        }
        let error = f
            .rejected(&format!("snapshot_{label}"), f.config.stream_config(), Some(actual))
            .await?;
        assert!(
            matches!(error, PeerStoreError::UnsafeSnapshotConfig(found) if found == expected),
            "{label}: wrong typed snapshot admission failure"
        );
    }
    Ok(())
}
