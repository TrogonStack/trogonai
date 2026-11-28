//! A released lease becomes a published snapshot through a real registry: a worker pushes its export to a registry
//! container, the builder records the digest in JetStream KV and publishes `SnapshotPublished`, and the recorded
//! coordinates pull back the bytes the worker pushed.
//!
//! This lane runs a registry container through Docker and needs network access to pull its image, so it is ignored
//! by default:
//!
//! ```text
//! cargo test -p trogon-sandbox-snapshot-builder --test end_to_end -- --ignored
//! ```

use std::net::TcpListener;
use std::num::NonZeroUsize;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::{Client, Request, RequestErrorKind, jetstream};
use bytes::Bytes;
use futures::StreamExt;
use prometheus::Registry as MetricsRegistry;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_broker::quota::{NatsKvQuotaStore, SnapshotQuota};
use trogon_sandbox_broker::snapshot_tracker::{NatsKvLiveSnapshotStore, account_snapshot_published};
use trogon_sandbox_contract::{
    ArtifactType, Digest, LeaseId, LeaseReleased, LeaseReleasedReason, OciArtifactRef, SnapshotExportError,
    SnapshotExportOutcome, SnapshotExportRequest, SnapshotPolicy, SnapshotPublished, SnapshotReceipt, TenantId,
    WorkerId,
};
use trogon_sandbox_oci::{Layer, OciClient, OciClientConfig, RegistryHost, build_push_target};
use trogon_sandbox_protocol::subjects::{
    SNAPSHOT_ACCOUNTING_REQUEST, SNAPSHOT_PUBLISHED, lease_released, worker_snapshot_export,
};
use trogon_sandbox_snapshot_builder::{
    BuilderConfig, SnapshotBuilder, SnapshotBuilderError, SnapshotBuilderMetrics, SnapshotKvStore,
};

type Boxed<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = Boxed<()>;

const REGISTRY_IMAGE: &str = "ghcr.io/project-zot/zot:v2.1.8";
const WORKER: &str = "end-to-end-worker";
const TENANT: &str = "tenant-end-to-end";
const REPOSITORY: &str = "snapshots/end-to-end/rootfs";
/// Long enough for a first run to pull the registry image.
const SCENARIO_LIMIT: Duration = Duration::from_secs(180);
const PUBLICATION_LIMIT: Duration = Duration::from_secs(30);

/// A registry container that is removed when dropped, even when the test fails.
struct Registry {
    container: String,
    address: String,
}

impl Registry {
    async fn start() -> Boxed<Self> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let output = Command::new("docker")
            .args(["run", "-d", "--rm", "-p", &format!("{port}:5000"), REGISTRY_IMAGE])
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!("docker run failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        let registry = Self {
            container: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            address: format!("127.0.0.1:{port}"),
        };
        registry.wait_ready().await?;
        Ok(registry)
    }

    async fn wait_ready(&self) -> TestResult {
        let client = reqwest::Client::new();
        for _ in 0..240 {
            if let Ok(response) = client.get(format!("http://{}/v2/", self.address)).send().await
                && (response.status().is_success() || response.status().as_u16() == 401)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(format!("the registry did not become ready at {}", self.address).into())
    }

    fn host(&self) -> Boxed<RegistryHost> {
        Ok(RegistryHost::new(self.address.as_str())?)
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn oci_client(host: RegistryHost) -> OciClient {
    OciClient::new(OciClientConfig {
        insecure_registries: vec![host],
        ..Default::default()
    })
}

/// A rootfs layer holding one file, so the pulled bytes can be told apart from an empty artifact.
fn rootfs_layer() -> Boxed<Vec<u8>> {
    let data: &[u8] = b"end to end fixture\n";
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(data.len())?);
    header.set_mode(0o644);
    builder.append_data(&mut header, "etc/motd", data)?;
    Ok(builder.into_inner()?)
}

/// Answers every accounting request with the real broker accounting, as a broker would.
struct AccountingFixture(JoinHandle<()>);

impl Drop for AccountingFixture {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn accounting_fixture(nats: Client) -> Boxed<AccountingFixture> {
    let js = jetstream::new(nats.clone());
    let quota = NatsKvQuotaStore::new(js.clone(), "end-to-end-quota").await?;
    let live = NatsKvLiveSnapshotStore::open(js).await?;
    let mut requests = nats.subscribe(SNAPSHOT_ACCOUNTING_REQUEST).await?;
    nats.flush().await?;
    Ok(AccountingFixture(tokio::spawn(async move {
        while let Some(request) = requests.next().await {
            let Ok(event) = serde_json::from_slice::<SnapshotPublished>(&request.payload) else {
                continue;
            };
            let response =
                account_snapshot_published(live.as_ref(), &quota, SnapshotQuota::new(100, 1_000_000), &event).await;
            let (Some(reply), Ok(payload)) = (request.reply, serde_json::to_vec(&response)) else {
                continue;
            };
            if nats.publish(reply, payload.into()).await.is_err() {
                break;
            }
        }
    })))
}

/// Plays the worker: pushes `layer` to the registry each export request names and answers with the pushed digest.
/// The last push failure is kept so a scenario that never publishes can say why.
struct PushingWorker {
    last_failure: Arc<Mutex<Option<String>>>,
    task: JoinHandle<()>,
}

impl Drop for PushingWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn push_export(client: &OciClient, request: &SnapshotExportRequest, layer: &[u8]) -> Boxed<SnapshotReceipt> {
    let artifact_type = request.target.artifact_type;
    let layers = vec![Layer {
        data: layer.to_vec(),
        media_type: artifact_type.media_type().to_string(),
        annotations: Default::default(),
    }];
    let subject = build_push_target(
        &request.target.registry,
        &request.target.repository,
        artifact_type,
        &layers,
        &Default::default(),
    )?;
    client.push_artifact(&subject, layers, Default::default()).await?;
    Ok(SnapshotReceipt {
        digest: subject.digest,
        bytes: u64::try_from(layer.len())?,
        artifact_type,
    })
}

async fn pushing_worker(nats: Client, client: OciClient, layer: Vec<u8>) -> Boxed<PushingWorker> {
    let last_failure = Arc::new(Mutex::new(None));
    let failures = last_failure.clone();
    let mut subscription = nats.subscribe(worker_snapshot_export(&WorkerId::new(WORKER)?)).await?;
    nats.flush().await?;
    let task = tokio::spawn(async move {
        while let Some(message) = subscription.next().await {
            let outcome = match serde_json::from_slice::<SnapshotExportRequest>(&message.payload) {
                Ok(request) => match push_export(&client, &request, &layer)
                    .await
                    .map_err(|error| error.to_string())
                {
                    Ok(receipt) => SnapshotExportOutcome::Ok(receipt),
                    Err(message) => {
                        *failures.lock().await = Some(message.clone());
                        SnapshotExportOutcome::Err(SnapshotExportError::Permanent { message })
                    }
                },
                Err(error) => {
                    *failures.lock().await = Some(format!("malformed export request: {error}"));
                    continue;
                }
            };
            let (Some(reply), Ok(payload)) = (message.reply, serde_json::to_vec(&outcome)) else {
                continue;
            };
            if nats.publish(reply, payload.into()).await.is_err() {
                break;
            }
        }
    });
    Ok(PushingWorker { last_failure, task })
}

struct RunningBuilder {
    cancel: CancellationToken,
    task: JoinHandle<Result<(), SnapshotBuilderError>>,
}

impl RunningBuilder {
    fn start(builder: SnapshotBuilder) -> Self {
        let cancel = CancellationToken::new();
        let task = tokio::spawn(builder.run(cancel.clone()));
        Self { cancel, task }
    }

    async fn stop(&mut self) -> TestResult {
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), &mut self.task).await???;
        Ok(())
    }
}

impl Drop for RunningBuilder {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// Waits until a builder listens on `subject`, which a probe that nobody answers can tell from no listener at all.
async fn wait_for_release_subscription(nats: &Client, subject: &str) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let request = Request::new()
                .payload(Bytes::from_static(b"{}"))
                .timeout(Some(Duration::from_millis(50)));
            match nats.send_request(subject.to_string(), request).await {
                Err(error) if error.kind() == RequestErrorKind::TimedOut => return Ok(()),
                Err(error) if error.kind() == RequestErrorKind::NoResponders => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                other => return Err(format!("unexpected release subscription probe result: {other:?}")),
            }
        }
    })
    .await?
    .map_err(Into::into)
}

fn release(parent: &Digest) -> Boxed<LeaseReleased> {
    Ok(LeaseReleased {
        lease_id: LeaseId::new("lease-end-to-end"),
        tenant: TenantId::new(TENANT)?,
        reason: LeaseReleasedReason::Completed,
        snapshot_out: Some(SnapshotPolicy {
            artifact_type: ArtifactType::SandboxRootfsV1,
            target_repository: REPOSITORY.to_string(),
            retention: None,
            prewarm: None,
            tag_name: None,
        }),
        worker_id: Some(WorkerId::new(WORKER)?),
        correlation_id: None,
        restored_from: Some(parent.clone()),
        released_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(100)),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires docker to run a registry container and network access to pull its image"]
async fn a_release_pushed_to_a_real_registry_is_recorded_published_and_pullable() -> TestResult {
    tokio::time::timeout(SCENARIO_LIMIT, end_to_end_scenario()).await?
}

async fn end_to_end_scenario() -> TestResult {
    let registry = Registry::start().await?;
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let _accounting = accounting_fixture(nats.clone()).await?;
    let layer = rootfs_layer()?;
    let worker = pushing_worker(nats.clone(), oci_client(registry.host()?), layer.clone()).await?;
    let mut publications = nats.subscribe(SNAPSHOT_PUBLISHED).await?;
    nats.flush().await?;

    let config = BuilderConfig {
        nats_url: format!("nats://{}", server.address()),
        default_registry: registry.address.clone(),
        max_concurrent: NonZeroUsize::MIN.saturating_add(1),
        cosign_kms_uri: None,
        scan_cvss_threshold: 7.0,
        require_scan_pass: false,
        oci_insecure_registries: vec![registry.host()?],
        dispatch_signer: None,
    };
    let metrics = SnapshotBuilderMetrics::register(&MetricsRegistry::new())?;
    let mut builder = RunningBuilder::start(SnapshotBuilder::connect(config, metrics).await?);

    let parent = Digest::new(format!("sha256:{}", "b".repeat(64)))?;
    let event = release(&parent)?;
    wait_for_release_subscription(&nats, &lease_released(&event.lease_id)).await?;
    nats.publish(lease_released(&event.lease_id), serde_json::to_vec(&event)?.into())
        .await?;

    let published = match tokio::time::timeout(PUBLICATION_LIMIT, publications.next()).await {
        Ok(Some(message)) => serde_json::from_slice::<SnapshotPublished>(&message.payload)?,
        Ok(None) => return Err("the publication observer closed".into()),
        Err(_) => {
            let failure = worker.last_failure.lock().await.clone();
            return Err(format!("no snapshot was published; last worker push failure: {failure:?}").into());
        }
    };
    assert_eq!(published.lease_id, event.lease_id);
    assert_eq!(published.tenant, event.tenant);
    assert_eq!(published.artifact_type, ArtifactType::SandboxRootfsV1);
    assert_eq!(published.bytes, u64::try_from(layer.len())?);

    let record = SnapshotKvStore::open(jetstream::new(nats.clone()))
        .await?
        .get_owned(&event.tenant, &published.digest)
        .await?
        .ok_or("the snapshot record is missing")?;
    assert_eq!(record.lease_id, event.lease_id);
    assert_eq!(record.digest, published.digest);
    assert_eq!(record.artifact_type, ArtifactType::SandboxRootfsV1);
    assert_eq!(record.registry, registry.address);
    assert_eq!(record.repository, REPOSITORY);
    assert_eq!(record.parent, Some(parent));
    assert!(record.created_at.is_some());

    let recorded = OciArtifactRef {
        registry: record.registry.clone(),
        repository: record.repository.clone(),
        digest: record.digest.clone(),
        artifact_type: record.artifact_type,
    };
    let pulled = oci_client(registry.host()?).pull_artifact(&recorded).await?;
    let pulled_layers: Vec<&[u8]> = pulled.layers.iter().map(|pulled| pulled.data.as_slice()).collect();
    assert_eq!(
        pulled_layers,
        [layer.as_slice()],
        "the recorded coordinates resolve to the bytes the worker pushed"
    );
    builder.stop().await
}
