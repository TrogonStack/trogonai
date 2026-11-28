use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use async_nats::{Client, Request, RequestErrorKind, Subscriber};
use bytes::Bytes;
use futures::StreamExt;
use prometheus::Registry;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_broker::quota::{NatsKvQuotaStore, QuotaStore, SnapshotQuota};
use trogon_sandbox_broker::snapshot_accounting::{SNAPSHOT_PUBLICATION_STREAM, SnapshotAccountingStore};
use trogon_sandbox_broker::snapshot_capture_status::{NatsKvSnapshotCaptureStatusStore, SnapshotCaptureStatusStore};
use trogon_sandbox_broker::snapshot_tag::{InMemorySnapshotTagAuditor, NatsKvSnapshotTagStore, SnapshotTagStore};
use trogon_sandbox_broker::snapshot_tracker::{
    InMemoryLiveSnapshotStore, NatsKvLiveSnapshotStore, account_snapshot_published, handle_snapshot_published,
};
use trogon_sandbox_contract::{
    ArtifactType, Digest, EvictedArtifactAction, LeaseId, LeaseReleased, LeaseReleasedReason, RetentionPolicy,
    RetentionTagLimit, SnapshotAccountingRefusal, SnapshotAccountingResponse, SnapshotCaptureStatus,
    SnapshotExportError, SnapshotExportOutcome, SnapshotExportRequest, SnapshotPolicy, SnapshotPublished,
    SnapshotReceipt, SnapshotTag, SnapshotTagName, TenantId, WorkerId,
};
use trogon_sandbox_protocol::subjects::{
    SNAPSHOT_ACCOUNTING_REQUEST, SNAPSHOT_PUBLISHED, SNAPSHOT_TAG_KV_BUCKET, lease_released, worker_snapshot_export,
};
use trogon_sandbox_snapshot_builder::{
    BuilderConfig, SnapshotBuilder, SnapshotBuilderError, SnapshotBuilderMetrics, SnapshotKvStore,
};

type Boxed<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = Boxed<()>;

const WORKER: &str = "tag-recovery-worker";
const TENANT: &str = "tenant-tag-recovery";
const REPOSITORY: &str = "snapshots/tag-recovery";

/// Polls `check` until it holds, or fails once `limit` has passed.
async fn eventually<F, Fut>(limit: Duration, mut check: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Boxed<bool>>,
{
    tokio::time::timeout(limit, async {
        loop {
            if check().await? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
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

fn url_of(server: &JetStreamTestServer) -> String {
    format!("nats://{}", server.address())
}

fn fixture_config(server: &JetStreamTestServer) -> BuilderConfig {
    BuilderConfig {
        nats_url: url_of(server),
        default_registry: "registry.invalid".to_string(),
        max_concurrent: 1,
        cosign_kms_uri: None,
        scan_cvss_threshold: 7.0,
        require_scan_pass: false,
        oci_insecure_registries: Vec::new(),
        dispatch_signer: None,
    }
}

fn fresh_metrics() -> Boxed<SnapshotBuilderMetrics> {
    Ok(SnapshotBuilderMetrics::register(&Registry::new())?)
}

async fn start_builder(server: &JetStreamTestServer, metrics: SnapshotBuilderMetrics) -> Boxed<RunningBuilder> {
    Ok(RunningBuilder::start(
        SnapshotBuilder::connect(fixture_config(server), metrics).await?,
    ))
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

fn fixture_receipt(hex: char) -> Boxed<SnapshotReceipt> {
    Ok(SnapshotReceipt {
        digest: Digest::new(format!("sha256:{}", hex.to_string().repeat(64)))?,
        bytes: 1_024,
        artifact_type: ArtifactType::SandboxRootfsV1,
    })
}

fn fixture_release(id: &str, timestamp: u64, tag: Option<&str>) -> Boxed<LeaseReleased> {
    Ok(LeaseReleased {
        lease_id: LeaseId::new(id),
        tenant: TenantId::new(TENANT)?,
        reason: LeaseReleasedReason::Completed,
        snapshot_out: Some(SnapshotPolicy {
            artifact_type: ArtifactType::SandboxRootfsV1,
            target_repository: REPOSITORY.to_string(),
            retention: None,
            prewarm: None,
            tag_name: tag.map(str::to_string),
        }),
        worker_id: Some(WorkerId::new(WORKER)?),
        correlation_id: None,
        restored_from: None,
        released_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp)),
    })
}

fn latest_tag(tenant: &TenantId) -> Boxed<SnapshotTag> {
    Ok(SnapshotTag {
        tenant: tenant.clone(),
        name: SnapshotTagName::new("latest")?,
    })
}

async fn publish_release(nats: &Client, event: &LeaseReleased) -> TestResult {
    nats.publish(lease_released(&event.lease_id), serde_json::to_vec(event)?.into())
        .await?;
    Ok(())
}

async fn next_publication(publications: &mut Subscriber) -> Boxed<(SnapshotPublished, Bytes)> {
    let message = tokio::time::timeout(Duration::from_secs(5), publications.next())
        .await?
        .ok_or("the publication observer closed")?;
    Ok((serde_json::from_slice(&message.payload)?, message.payload))
}

async fn no_publication_within(publications: &mut Subscriber, wait: Duration) -> bool {
    tokio::time::timeout(wait, publications.next()).await.is_err()
}

/// Answers every accounting request with the real broker accounting, as a broker would.
struct AccountingFixture(JoinHandle<()>);

impl Drop for AccountingFixture {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn real_accounting_fixture(nats: Client) -> Boxed<AccountingFixture> {
    let js = jetstream::new(nats.clone());
    let quota = NatsKvQuotaStore::new(js.clone(), "tag-fixture-quota").await?;
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

/// Plays the worker: answers each export request with a receipt and counts them.
struct FakeExporter {
    calls: Arc<AtomicUsize>,
    repositories: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Drop for FakeExporter {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fake_exporter(
    nats: Client,
    receipt_for: impl Fn(&SnapshotExportRequest) -> SnapshotReceipt + Send + 'static,
) -> Boxed<FakeExporter> {
    let calls = Arc::new(AtomicUsize::new(0));
    let repositories = Arc::new(Mutex::new(Vec::new()));
    let (observed, targets) = (calls.clone(), repositories.clone());
    let mut subscription = nats.subscribe(worker_snapshot_export(&WorkerId::new(WORKER)?)).await?;
    nats.flush().await?;
    let task = tokio::spawn(async move {
        while let Some(message) = subscription.next().await {
            let Ok(request) = serde_json::from_slice::<SnapshotExportRequest>(&message.payload) else {
                continue;
            };
            observed.fetch_add(1, Ordering::SeqCst);
            targets.lock().await.push(request.target.repository.clone());
            let outcome = SnapshotExportOutcome::Ok(receipt_for(&request));
            let (Some(reply), Ok(payload)) = (message.reply, serde_json::to_vec(&outcome)) else {
                continue;
            };
            if nats.publish(reply, payload.into()).await.is_err() {
                break;
            }
        }
    });
    Ok(FakeExporter {
        calls,
        repositories,
        task,
    })
}

async fn drain_tag_work(js: &jetstream::Context) -> TestResult {
    eventually(Duration::from_secs(5), || async {
        let mut stream = js.get_stream(SNAPSHOT_PUBLICATION_STREAM).await?;
        Ok(stream.info().await?.state.messages == 0)
    })
    .await
}

async fn wait_capture(capture: &dyn SnapshotCaptureStatusStore, lease: &LeaseId) -> TestResult {
    eventually(Duration::from_secs(5), || async {
        Ok(matches!(
            capture.get(lease).await?,
            Some(SnapshotCaptureStatus::Created { .. })
        ))
    })
    .await
}

async fn capture_store(js: &jetstream::Context) -> Boxed<NatsKvSnapshotCaptureStatusStore> {
    Ok(NatsKvSnapshotCaptureStatusStore::new(js.clone()).await?)
}

async fn tag_store(js: &jetstream::Context) -> Boxed<NatsKvSnapshotTagStore> {
    Ok(NatsKvSnapshotTagStore::new(js.clone(), Arc::new(InMemorySnapshotTagAuditor::new())).await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_builder_recovers_a_failed_tag_without_export_or_publication_replay() -> TestResult {
    tokio::time::timeout(Duration::from_secs(60), recovery_scenario()).await?
}

async fn recovery_scenario() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let js = jetstream::new(nats.clone());
    let _accounting = real_accounting_fixture(nats.clone()).await?;
    let receipt = fixture_receipt('a')?;
    let exporter = fake_exporter(nats.clone(), {
        let receipt = receipt.clone();
        move |_| receipt.clone()
    })
    .await?;
    let mut published = nats.subscribe(SNAPSHOT_PUBLISHED).await?;
    nats.flush().await?;

    let metrics = fresh_metrics()?;
    let builder = SnapshotBuilder::connect(fixture_config(&server), metrics.clone()).await?;
    let mut tag_stream = js.get_stream(format!("KV_{SNAPSHOT_TAG_KV_BUCKET}")).await?;
    let original_tag_config = tag_stream.info().await?.config.clone();
    let mut failing_tag_config = original_tag_config.clone();
    failing_tag_config.max_message_size = 1;
    js.update_stream(&failing_tag_config).await?;
    let mut first = RunningBuilder::start(builder);

    let retention = RetentionPolicy {
        max_tags: RetentionTagLimit::new(3)?,
        max_age_days: 7,
        delete_evicted: EvictedArtifactAction::Keep,
    };
    let mut event = fixture_release("lease-tag-recovery", 1_234, Some("latest"))?;
    if let Some(policy) = event.snapshot_out.as_mut() {
        policy.retention = Some(retention);
    }
    let released_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_234);
    wait_for_release_subscription(&nats, &lease_released(&event.lease_id)).await?;
    publish_release(&nats, &event).await?;
    let (publication, _) = next_publication(&mut published).await?;
    assert_eq!(publication.digest, receipt.digest);

    let capture = capture_store(&js).await?;
    eventually(Duration::from_secs(15), || async {
        Ok(matches!(
            capture.get(&event.lease_id).await?,
            Some(SnapshotCaptureStatus::Created { .. })
        ) && metrics.failed_total.with_label_values(&["tag_not_moved"]).get() == 1)
    })
    .await?;
    first.stop().await?;
    assert_eq!(exporter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(metrics.pushed_total.with_label_values(&["sandbox_rootfs_v1"]).get(), 1);
    assert_eq!(metrics.inflight.get(), 0);
    js.update_stream(&original_tag_config).await?;

    let tags = tag_store(&js).await?;
    let tag = latest_tag(&event.tenant)?;
    assert!(
        tags.get(&tag).await?.is_none(),
        "the original process could not move the tag"
    );
    let mut fresh = start_builder(&server, fresh_metrics()?).await?;
    eventually(Duration::from_secs(10), || async {
        Ok(tags.get(&tag).await?.is_some())
    })
    .await?;
    let recovered = tags.get(&tag).await?.ok_or("the tag is not set")?;
    assert_eq!(recovered.digest, receipt.digest);
    assert_eq!(recovered.set_at, released_at);
    assert_eq!(recovered.retention, Some(retention));
    assert_eq!(
        exporter.calls.load(Ordering::SeqCst),
        1,
        "tag recovery must not re-export"
    );
    assert_eq!(*exporter.repositories.lock().await, [REPOSITORY]);
    assert!(
        no_publication_within(&mut published, Duration::from_millis(250)).await,
        "tag recovery must not republish the quota event"
    );
    fresh.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_release_is_exported_and_published_once_across_two_builders() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let _accounting = real_accounting_fixture(nats.clone()).await?;
    let receipt = fixture_receipt('a')?;
    let exporter = fake_exporter(nats.clone(), move |_| receipt.clone()).await?;
    let mut publications = nats.subscribe(SNAPSHOT_PUBLISHED).await?;
    nats.flush().await?;
    let mut first = start_builder(&server, fresh_metrics()?).await?;
    let mut second = start_builder(&server, fresh_metrics()?).await?;
    let event = fixture_release("one-release", 100, None)?;
    // A malformed release is dropped by the builder that receives it, so a queue group that has both members
    // answers none of the probes; the probe only proves a member is listening.
    wait_for_release_subscription(&nats, &lease_released(&event.lease_id)).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    publish_release(&nats, &event).await?;
    let (publication, _) = next_publication(&mut publications).await?;
    assert_eq!(publication.lease_id, event.lease_id);
    assert!(
        no_publication_within(&mut publications, Duration::from_millis(500)).await,
        "one release must not publish the snapshot twice when two builders are running"
    );
    assert_eq!(
        exporter.calls.load(Ordering::SeqCst),
        1,
        "one release must export exactly once"
    );
    first.stop().await?;
    second.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newer_identical_snapshot_advances_tag_order_and_retention_without_replaying_it() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let js = jetstream::new(nats.clone());
    let _accounting = real_accounting_fixture(nats.clone()).await?;
    let (older, newer) = (fixture_receipt('a')?, fixture_receipt('b')?);
    let exporter = fake_exporter(nats.clone(), {
        let (older, newer) = (older.clone(), newer.clone());
        move |request| {
            if request.lease_id == LeaseId::new("delayed-b") {
                newer.clone()
            } else {
                older.clone()
            }
        }
    })
    .await?;
    let mut publications = nats.subscribe(SNAPSHOT_PUBLISHED).await?;
    nats.flush().await?;
    let mut builder = start_builder(&server, fresh_metrics()?).await?;
    let mut first = fixture_release("first-a", 100, Some("latest"))?;
    let mut identical = fixture_release("identical-a", 300, Some("latest"))?;
    let older_retention = RetentionPolicy {
        max_tags: RetentionTagLimit::new(2)?,
        max_age_days: 7,
        delete_evicted: EvictedArtifactAction::Keep,
    };
    let newer_retention = RetentionPolicy {
        max_tags: RetentionTagLimit::new(4)?,
        max_age_days: 14,
        delete_evicted: EvictedArtifactAction::Delete,
    };
    if let Some(policy) = first.snapshot_out.as_mut() {
        policy.retention = Some(older_retention);
    }
    if let Some(policy) = identical.snapshot_out.as_mut() {
        policy.retention = Some(newer_retention);
    }
    let capture = capture_store(&js).await?;
    let tags = tag_store(&js).await?;
    let tag = latest_tag(&first.tenant)?;
    wait_for_release_subscription(&nats, &lease_released(&first.lease_id)).await?;
    let mut identical_publication = None;
    for event in [&first, &identical] {
        publish_release(&nats, event).await?;
        let (decoded, payload) = next_publication(&mut publications).await?;
        assert_eq!(decoded.lease_id, event.lease_id);
        wait_capture(&capture, &event.lease_id).await?;
        drain_tag_work(&js).await?;
        if event.lease_id == identical.lease_id {
            identical_publication = Some(payload);
        }
    }
    let revision = tags.get_entry(&tag).await?.ok_or("the tag is not set")?.1;
    js.publish(
        SNAPSHOT_PUBLISHED,
        identical_publication.ok_or("the identical release was not published")?,
    )
    .await?
    .await?;
    drain_tag_work(&js).await?;
    assert_eq!(
        tags.get_entry(&tag).await?.ok_or("the tag is not set")?.1,
        revision,
        "an exact tag-event retry must not rewrite its revision"
    );
    assert_eq!(
        exporter.calls.load(Ordering::SeqCst),
        2,
        "tag-event replay must not export"
    );
    let delayed = fixture_release("delayed-b", 200, Some("latest"))?;
    publish_release(&nats, &delayed).await?;
    wait_capture(&capture, &delayed.lease_id).await?;
    drain_tag_work(&js).await?;
    let current = tags.get(&tag).await?.ok_or("the tag is not set")?;
    assert_eq!(
        current.digest, older.digest,
        "A@300 must keep delayed B@200 from moving the tag backwards"
    );
    assert_eq!(current.set_at, SystemTime::UNIX_EPOCH + Duration::from_secs(300));
    assert_eq!(current.retention, Some(newer_retention));
    builder.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn billing_obligation_survives_broker_outage_refusals_and_a_fresh_builder() -> TestResult {
    tokio::time::timeout(Duration::from_secs(45), billing_outage_scenario()).await?
}

async fn tag_work_messages(stream: &mut jetstream::stream::Stream) -> Boxed<u64> {
    Ok(stream.info().await?.state.messages)
}

async fn billing_outage_scenario() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let js = jetstream::new(nats.clone());
    let receipt = fixture_receipt('a')?;
    let exporter = fake_exporter(nats.clone(), {
        let receipt = receipt.clone();
        move |_| receipt.clone()
    })
    .await?;
    let mut publications = nats.subscribe(SNAPSHOT_PUBLISHED).await?;
    nats.flush().await?;
    let mut builder = start_builder(&server, fresh_metrics()?).await?;
    let event = fixture_release("billing-outage", 100, Some("latest"))?;
    let subject = lease_released(&event.lease_id);
    wait_for_release_subscription(&nats, &subject).await?;
    publish_release(&nats, &event).await?;
    let (published, _) = next_publication(&mut publications).await?;
    let capture = capture_store(&js).await?;
    wait_capture(&capture, &event.lease_id).await?;
    let mut work = js.get_stream(SNAPSHOT_PUBLICATION_STREAM).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        tag_work_messages(&mut work).await?,
        1,
        "all brokers are unavailable: the original publication must retain its billing obligation"
    );
    let tags = tag_store(&js).await?;
    let tag = latest_tag(&event.tenant)?;
    assert!(
        tags.get(&tag).await?.is_none(),
        "a tag must await positive accounting confirmation"
    );
    builder.stop().await?;
    let mut builder = start_builder(&server, fresh_metrics()?).await?;

    let mut accounting = nats.subscribe(SNAPSHOT_ACCOUNTING_REQUEST).await?;
    nats.flush().await?;
    let refusals = [
        Some(SnapshotAccountingRefusal::Backend),
        Some(SnapshotAccountingRefusal::Pending),
        None,
    ];
    for refusal in refusals {
        let request = tokio::time::timeout(Duration::from_secs(8), accounting.next())
            .await?
            .ok_or("the accounting observer closed")?;
        let requested: SnapshotPublished = serde_json::from_slice(&request.payload)?;
        assert_eq!(requested, published);
        if let (Some(reason), Some(reply)) = (refusal, request.reply) {
            let response = SnapshotAccountingResponse::Refused { reason };
            nats.publish(reply, serde_json::to_vec(&response)?.into()).await?;
            nats.flush().await?;
        }
        assert_eq!(
            tag_work_messages(&mut work).await?,
            1,
            "a refusal or timeout cannot acknowledge the obligation"
        );
        assert!(tags.get(&tag).await?.is_none());
    }

    let request = tokio::time::timeout(Duration::from_secs(8), accounting.next())
        .await?
        .ok_or("the accounting observer closed")?;
    let requested: SnapshotPublished = serde_json::from_slice(&request.payload)?;
    assert_eq!(requested, published);
    // Durable accounting may finish after the requesting inbox has expired.
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    let quota = NatsKvQuotaStore::new(js.clone(), "snapshot-outage-quota").await?;
    let live = InMemoryLiveSnapshotStore::new();
    handle_snapshot_published(
        live.as_ref(),
        &quota,
        SnapshotQuota::new(100, 1_000_000),
        requested.clone(),
    )
    .await;
    let reopened = NatsKvQuotaStore::new(js.clone(), "snapshot-outage-quota").await?;
    let budget = reopened
        .quota_record(&requested.tenant)
        .await?
        .ok_or("no quota record was charged")?;
    assert_eq!(
        (budget.snapshot.count_used, budget.snapshot.bytes_used),
        (1, requested.bytes),
        "the test adapter can confirm only a real durable charge"
    );
    let artifact = SnapshotKvStore::open(js.clone())
        .await?
        .get(&requested.digest)
        .await?
        .ok_or("the artifact record is missing")?;
    assert_eq!(artifact.tenant, requested.tenant);
    assert_eq!(artifact.lease_id, requested.lease_id);
    assert_eq!(artifact.artifact_type, requested.artifact_type);
    let confirmed =
        account_snapshot_published(live.as_ref(), &reopened, SnapshotQuota::new(100, 1_000_000), &requested).await;
    assert!(matches!(confirmed, SnapshotAccountingResponse::Committed { .. }));
    let confirmation = Bytes::from(serde_json::to_vec(&confirmed)?);
    nats.publish(
        request.reply.ok_or("the request had no reply subject")?,
        confirmation.clone(),
    )
    .await?;
    let answering = async {
        while let Some(request) = accounting.next().await {
            let retried: SnapshotPublished = serde_json::from_slice(&request.payload)?;
            assert_eq!(retried, published);
            if let Some(reply) = request.reply {
                nats.publish(reply, confirmation.clone()).await?;
            }
        }
        Err::<(), Box<dyn std::error::Error>>(
            "accounting subscriber closed before the obligation was acknowledged".into(),
        )
    };
    tokio::select! {
        drained = drain_tag_work(&js) => drained?,
        failed = answering => failed?,
    }
    assert_eq!(
        tags.get(&tag).await?.ok_or("the tag is not set")?.digest,
        published.digest
    );
    let budget = reopened
        .quota_record(&published.tenant)
        .await?
        .ok_or("no quota record was charged")?;
    assert_eq!(
        (budget.snapshot.count_used, budget.snapshot.bytes_used),
        (1, published.bytes)
    );
    assert_eq!(exporter.calls.load(Ordering::SeqCst), 1);
    assert!(
        no_publication_within(&mut publications, Duration::from_millis(250)).await,
        "billing recovery must not republish the original event"
    );
    builder.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_release_cannot_export_again_while_the_original_receipt_is_pending() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let _accounting = real_accounting_fixture(nats.clone()).await?;
    let mut exports = nats.subscribe(worker_snapshot_export(&WorkerId::new(WORKER)?)).await?;
    nats.flush().await?;
    let mut config = fixture_config(&server);
    config.max_concurrent = 2;
    let mut builder = RunningBuilder::start(SnapshotBuilder::connect(config, fresh_metrics()?).await?);
    let event = fixture_release("duplicate-pending-export", 100, None)?;
    let subject = lease_released(&event.lease_id);
    wait_for_release_subscription(&nats, &subject).await?;
    let payload = Bytes::from(serde_json::to_vec(&event)?);
    nats.publish(subject.clone(), payload.clone()).await?;
    let first = tokio::time::timeout(Duration::from_secs(3), exports.next())
        .await?
        .ok_or("the export observer closed")?;
    nats.publish(subject, payload).await?;
    nats.flush().await?;
    let second = tokio::time::timeout(Duration::from_secs(1), exports.next())
        .await
        .ok()
        .flatten();
    let duplicated = second.is_some();
    let receipt = Bytes::from(serde_json::to_vec(&SnapshotExportOutcome::Ok(fixture_receipt('a')?))?);
    nats.publish(first.reply.ok_or("the export had no reply subject")?, receipt.clone())
        .await?;
    if let Some(reply) = second.and_then(|message| message.reply) {
        nats.publish(reply, receipt).await?;
    }
    let capture = capture_store(&jetstream::new(nats.clone())).await?;
    wait_capture(&capture, &event.lease_id).await?;
    builder.stop().await?;
    assert!(
        !duplicated,
        "an unbound host reservation must not grant a second worker export effect"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_worker_export_outcome_is_retained_without_replaying_the_effect() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let mut exports = nats.subscribe(worker_snapshot_export(&WorkerId::new(WORKER)?)).await?;
    nats.flush().await?;
    let mut builder = start_builder(&server, fresh_metrics()?).await?;
    let event = fixture_release("ambiguous-export", 100, None)?;
    let subject = lease_released(&event.lease_id);
    wait_for_release_subscription(&nats, &subject).await?;
    publish_release(&nats, &event).await?;
    let first = tokio::time::timeout(Duration::from_secs(3), exports.next())
        .await?
        .ok_or("the export observer closed")?;
    let transient = SnapshotExportOutcome::Err(SnapshotExportError::Transient {
        message: "export side effect outcome is unknown".into(),
    });
    nats.publish(
        first.reply.ok_or("the export had no reply subject")?,
        Bytes::from(serde_json::to_vec(&transient)?),
    )
    .await?;
    let second = tokio::time::timeout(Duration::from_secs(1), exports.next())
        .await
        .ok()
        .flatten();
    let replayed = second.is_some();
    if let Some(reply) = second.and_then(|message| message.reply) {
        let permanent = SnapshotExportOutcome::Err(SnapshotExportError::Permanent {
            message: "stop the owned fixture after observing the duplicate".into(),
        });
        nats.publish(reply, Bytes::from(serde_json::to_vec(&permanent)?))
            .await?;
    }
    let js = jetstream::new(nats.clone());
    let capture = capture_store(&js).await?;
    eventually(Duration::from_secs(3), || async {
        Ok(matches!(
            capture.get(&event.lease_id).await?,
            Some(SnapshotCaptureStatus::Failed { .. })
        ))
    })
    .await?;
    let pending = SnapshotAccountingStore::open(js)
        .await?
        .pending_captures(&event.tenant)
        .await
        .map_err(|refusal| format!("pending captures were refused: {refusal:?}"))?;
    builder.stop().await?;
    assert_eq!(
        pending.len(),
        1,
        "unknown effects retain their original host reservation"
    );
    assert!(
        !replayed,
        "a generic transient export result does not prove that replaying the physical export is safe"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_records_its_snapshot_digest_and_publishes_it() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let _accounting = real_accounting_fixture(nats.clone()).await?;
    let receipt = fixture_receipt('c')?;
    let exporter = fake_exporter(nats.clone(), {
        let receipt = receipt.clone();
        move |_| receipt.clone()
    })
    .await?;
    let mut publications = nats.subscribe(SNAPSHOT_PUBLISHED).await?;
    nats.flush().await?;
    let mut builder = start_builder(&server, fresh_metrics()?).await?;
    let parent = Digest::new(format!("sha256:{}", "b".repeat(64)))?;
    let mut event = fixture_release("records-and-publishes", 100, None)?;
    event.restored_from = Some(parent.clone());
    wait_for_release_subscription(&nats, &lease_released(&event.lease_id)).await?;
    publish_release(&nats, &event).await?;

    let (published, _) = next_publication(&mut publications).await?;
    assert_eq!(published.lease_id, event.lease_id);
    assert_eq!(published.tenant, event.tenant);
    assert_eq!(published.digest, receipt.digest);
    assert_eq!(published.artifact_type, receipt.artifact_type);
    assert_eq!(published.bytes, receipt.bytes);

    let record = SnapshotKvStore::open(jetstream::new(nats.clone()))
        .await?
        .get_owned(&event.tenant, &published.digest)
        .await?
        .ok_or("the snapshot record is missing")?;
    assert_eq!(record.lease_id, event.lease_id);
    assert_eq!(record.digest, receipt.digest);
    assert_eq!(record.artifact_type, ArtifactType::SandboxRootfsV1);
    assert_eq!(record.registry, fixture_config(&server).default_registry);
    assert_eq!(record.repository, REPOSITORY);
    assert_eq!(record.parent, Some(parent));
    assert!(record.created_at.is_some());
    assert_eq!(
        record.env_digest, None,
        "nothing was scanned or signed, so no environment digest was read"
    );
    assert_eq!(*exporter.repositories.lock().await, [REPOSITORY]);
    builder.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_with_an_out_of_range_retention_is_dropped_without_export() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let nats = server.client().await;
    let _accounting = real_accounting_fixture(nats.clone()).await?;
    let receipt = fixture_receipt('a')?;
    let exporter = fake_exporter(nats.clone(), move |_| receipt.clone()).await?;
    let mut publications = nats.subscribe(SNAPSHOT_PUBLISHED).await?;
    nats.flush().await?;
    let mut builder = start_builder(&server, fresh_metrics()?).await?;
    let refused = fixture_release("out-of-range-retention", 100, Some("latest"))?;
    let mut payload = serde_json::to_value(&refused)?;
    payload["snapshot_out"]["retention"] = serde_json::json!({ "max_tags": 0, "max_age_days": 7 });
    let accepted = fixture_release("in-range-retention", 200, Some("latest"))?;
    wait_for_release_subscription(&nats, &lease_released(&refused.lease_id)).await?;

    // One builder reads its releases in order, so the in-range release arrives after the refused one was handled.
    nats.publish(lease_released(&refused.lease_id), serde_json::to_vec(&payload)?.into())
        .await?;
    publish_release(&nats, &accepted).await?;
    let (published, _) = next_publication(&mut publications).await?;
    assert_eq!(published.lease_id, accepted.lease_id);
    assert!(
        no_publication_within(&mut publications, Duration::from_millis(250)).await,
        "a release whose retention is out of range must not be published"
    );
    assert_eq!(
        exporter.calls.load(Ordering::SeqCst),
        1,
        "a release whose retention is out of range must not be exported"
    );
    let capture = capture_store(&jetstream::new(nats.clone())).await?;
    assert!(capture.get(&refused.lease_id).await?.is_none());
    builder.stop().await
}
