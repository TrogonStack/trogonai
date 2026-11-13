use std::{
    error::Error as StdError,
    future::pending,
    io,
    num::{NonZeroU64, NonZeroUsize},
    sync::Arc,
    time::{Duration, Instant},
};

use async_nats::{HeaderMap, jetstream::Context};
use bytes::Bytes;
use ed25519_dalek::SigningKey;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinSet;
use tokio::time::timeout;
use trogon_nats::jetstream::{NativeKvCasError, StrictNativeStreamReader, is_get_stream_not_found};
use trogon_nats::{NatsToken, connect_once_with_inbox_prefix, test_support::TlsJetStreamCluster};

use super::{
    Error, RegistrationArchive, RegistrationArchiveConfig, RegistrationArchiveCreate, RegistrationArchiveProfile,
    RegistrationArchiveRole, RegistrationArtifact, RegistrationArtifactInput, RegistrationArtifactKind,
    RegistrationArtifactObservation, Sha256Digest, bounded, require_time_remaining,
};
use crate::{ProducerConfig, RegistrationArchiveCapacity, constants::OPERATION_TIMEOUT};

type TestResult<T = ()> = Result<T, Box<dyn StdError + Send + Sync>>;

#[derive(Default)]
struct TestGate {
    entered: Notify,
    resume: Notify,
}
impl TestGate {
    async fn pause(&self) {
        self.entered.notify_one();
        self.resume.notified().await;
    }
    async fn wait_until_entered(&self) -> TestResult {
        timeout(OPERATION_TIMEOUT, self.entered.notified()).await?;
        Ok(())
    }
    fn release(&self) {
        self.resume.notify_one();
    }
}

#[derive(Default)]
pub(super) struct ArchiveTestGates {
    before_write: Mutex<Option<Arc<TestGate>>>,
    after_ack: Mutex<Option<Arc<TestGate>>>,
}
impl ArchiveTestGates {
    pub(super) async fn before_native_write(&self) {
        let gate = self.before_write.lock().await.take();
        if let Some(gate) = gate {
            gate.pause().await;
        }
    }
    pub(super) async fn after_native_ack(&self) {
        let gate = self.after_ack.lock().await.take();
        if let Some(gate) = gate {
            gate.pause().await;
        }
    }
    async fn arm_before_write(&self) -> Arc<TestGate> {
        let gate = Arc::new(TestGate::default());
        *self.before_write.lock().await = Some(Arc::clone(&gate));
        gate
    }
    async fn arm_after_ack(&self) -> Arc<TestGate> {
        let gate = Arc::new(TestGate::default());
        *self.after_ack.lock().await = Some(Arc::clone(&gate));
        gate
    }
}

fn manifest(bytes: &'static [u8]) -> Result<RegistrationArtifact, Error> {
    RegistrationArtifact::try_from(RegistrationArtifactInput {
        kind: RegistrationArtifactKind::BootstrapManifest,
        bytes: Bytes::from_static(bytes),
    })
}

fn record(operation: Option<&'static str>, sequence: u64) -> TestResult<async_nats::jetstream::message::StreamMessage> {
    let mut headers = HeaderMap::new();
    if let Some(operation) = operation {
        headers.insert("KV-Operation", operation);
    }
    Ok(async_nats::jetstream::message::StreamMessage {
        subject: "$KV.bounded.original".into(),
        sequence,
        headers,
        payload: Bytes::from_static(b"bounded original public bytes"),
        time: async_nats::datetime::from_nanos(0)?,
    })
}

#[test]
fn native_record_boundary_refuses_tombstones_and_missing_revisions() -> TestResult {
    let digest = Sha256Digest::hash(b"bounded original public bytes");
    for operation in ["DEL", "PURGE"] {
        assert!(matches!(
            RegistrationArchive::record(
                RegistrationArtifactKind::BootstrapManifest,
                digest,
                record(Some(operation), 1)?
            ),
            Err(Error::Operation)
        ));
    }
    assert!(matches!(
        RegistrationArchive::record(RegistrationArtifactKind::BootstrapManifest, digest, record(None, 0)?),
        Err(Error::Integrity)
    ));
    Ok(())
}

#[tokio::test]
async fn expired_absolute_boundary_returns_its_typed_deadline_error() {
    let outcome = bounded(Instant::now(), pending::<Result<(), Error>>()).await;
    assert!(matches!(outcome, Err(Error::Deadline(_))));
    assert!(matches!(
        require_time_remaining(Instant::now()),
        Err(Error::DeadlineExpired)
    ));
}

struct NativeArchiveFixture {
    archive: Arc<RegistrationArchive>,
    admin: Context,
    competing: Context,
    profile: RegistrationArchiveProfile,
    gates: Arc<ArchiveTestGates>,
    _cluster: TlsJetStreamCluster,
}
impl NativeArchiveFixture {
    async fn start() -> TestResult<Self> {
        let registration = ProducerConfig::new(
            "acme",
            "project",
            "identity.example.org",
            NonZeroU64::MIN,
            SigningKey::from_bytes(&[7; 32]).verifying_key(),
        )?;
        let config = RegistrationArchiveConfig::new(
            registration,
            RegistrationArchiveCapacity::new(RegistrationArtifactKind::OriginalApproval, 16, 4 * 1024 * 1024)?,
            RegistrationArchiveCapacity::new(RegistrationArtifactKind::BootstrapManifest, 16, 4 * 1024 * 1024)?,
        )?;
        let profile = RegistrationArchiveProfile::new(&config)?;
        let writer = NatsToken::new("archive-fault-writer")?;
        let cluster = TlsJetStreamCluster::start(&[(
            writer.clone(),
            serde_json::json!({
                "publish": profile.publish_allowlist(RegistrationArchiveRole::Writer),
                "subscribe": profile.subscribe_allowlist(RegistrationArchiveRole::Writer)?,
            }),
        )])
        .await;
        let admin_config = cluster
            .config(&NatsToken::new("fixture-admin")?)
            .ok_or_else(|| io::Error::other("fixture administrator identity is absent"))?;
        let admin = async_nats::jetstream::new(
            connect_once_with_inbox_prefix(
                &admin_config,
                admin_config.auth.clone(),
                OPERATION_TIMEOUT,
                &profile.inbox_prefix(RegistrationArchiveRole::Provisioner)?,
            )
            .await?,
        );
        for kind in RegistrationArtifactKind::ALL {
            timeout(OPERATION_TIMEOUT, admin.create_stream(profile.stream_config(kind))).await??;
        }
        let writer_config = cluster
            .config(&writer)
            .ok_or_else(|| io::Error::other("fixture writer identity is absent"))?;
        let competing = async_nats::jetstream::new(
            connect_once_with_inbox_prefix(
                &writer_config,
                writer_config.auth.clone(),
                OPERATION_TIMEOUT,
                &profile.inbox_prefix(RegistrationArchiveRole::Writer)?,
            )
            .await?,
        );
        let mut archive = RegistrationArchive::connect(config, writer_config).await?;
        let gates = Arc::new(ArchiveTestGates::default());
        Arc::get_mut(&mut archive)
            .ok_or_else(|| io::Error::other("new archive has an unexpected owner"))?
            .test_gates = Some(Arc::clone(&gates));
        Ok(Self {
            archive,
            admin,
            competing,
            profile,
            gates,
            _cluster: cluster,
        })
    }
    async fn head(&self) -> TestResult<u64> {
        let stream = timeout(
            OPERATION_TIMEOUT,
            self.admin.get_stream(
                self.profile
                    .stream_config(RegistrationArtifactKind::BootstrapManifest)
                    .name,
            ),
        )
        .await??;
        Ok(stream.cached_info().state.last_sequence)
    }
    async fn stored(
        &self,
        artifact: &RegistrationArtifact,
    ) -> TestResult<Option<async_nats::jetstream::message::StreamMessage>> {
        let (scope, key) = self.archive.binding(artifact.kind(), artifact.digest())?;
        let reader = StrictNativeStreamReader::new(
            self.competing.clone(),
            NonZeroUsize::new(artifact.kind().maximum_bytes())
                .ok_or_else(|| io::Error::other("zero artifact bound"))?,
            OPERATION_TIMEOUT,
        )?;
        Ok(timeout(OPERATION_TIMEOUT, reader.read_kv_last(&scope, &key)).await??)
    }
}

async fn finish_create(
    creations: &mut JoinSet<Result<RegistrationArchiveCreate, Error>>,
    wait: Duration,
) -> TestResult<Result<RegistrationArchiveCreate, Error>> {
    Ok(timeout(wait, creations.join_next())
        .await?
        .ok_or_else(|| io::Error::other("owned create task is absent"))??)
}

#[tokio::test]
async fn actual_native_faults_cannot_manufacture_acknowledged_archive_receipts() -> TestResult {
    let fixture = NativeArchiveFixture::start().await?;
    let mut creations = JoinSet::new();
    let retained = manifest(b"retained original public manifest")?;
    assert!(matches!(
        fixture.archive.create_once(retained.clone()).await?,
        RegistrationArchiveCreate::Acknowledged(_)
    ));
    let observed = fixture
        .archive
        .observe_exact(retained.kind(), retained.digest())
        .await?;
    assert!(matches!(
        fixture.archive.check_remaining(Instant::now()),
        Err(Error::DeadlineExpired)
    ));
    // Public observations are closed; this private control tests altered retained provenance.
    let altered = RegistrationArtifactObservation::from_native(
        observed.artifact().clone(),
        observed.native_revision(),
        observed.stream_created_at() + Duration::from_secs(1),
        Arc::clone(&fixture.archive),
    );
    assert!(matches!(altered.revalidate_original().await, Err(Error::Integrity)));

    let target = manifest(b"target displaced by a concurrent native frontier")?;
    let competitor = manifest(b"independently acknowledged competing original")?;
    let before = fixture.head().await?;
    let gate = fixture.gates.arm_before_write().await;
    let owner = Arc::clone(&fixture.archive);
    let requested = target.clone();
    creations.spawn(async move { owner.create_once(requested).await });
    gate.wait_until_entered().await?;
    assert_eq!(fixture.head().await?, before);
    assert!(fixture.stored(&target).await?.is_none());
    let (scope, key) = fixture.archive.binding(competitor.kind(), competitor.digest())?;
    let acknowledged = timeout(
        OPERATION_TIMEOUT,
        fixture
            .competing
            .publish(scope.subject(&key), competitor.bytes().clone()),
    )
    .await??;
    let acknowledged = timeout(OPERATION_TIMEOUT, acknowledged).await??;
    assert_eq!(acknowledged.stream, scope.stream());
    assert_eq!(acknowledged.sequence, before + 1);
    gate.release();
    assert!(matches!(
        finish_create(&mut creations, OPERATION_TIMEOUT).await?,
        Err(Error::Conflict)
    ));
    assert!(fixture.stored(&target).await?.is_none());

    let target = manifest(b"native ACK followed by actual source profile drift")?;
    let gate = fixture.gates.arm_after_ack().await;
    let owner = Arc::clone(&fixture.archive);
    let requested = target.clone();
    creations.spawn(async move { owner.create_once(requested).await });
    gate.wait_until_entered().await?;
    let stored = fixture
        .stored(&target)
        .await?
        .ok_or_else(|| io::Error::other("actual native ACK lacks its stored occurrence"))?;
    assert_eq!(stored.payload, *target.bytes());
    let profile = fixture.profile.stream_config(target.kind());
    let mut changed = profile.clone();
    changed.max_messages += 1;
    timeout(OPERATION_TIMEOUT, fixture.admin.update_stream(changed)).await??;
    gate.release();
    let outcome = finish_create(&mut creations, OPERATION_TIMEOUT).await?;
    assert!(
        matches!(outcome, Err(Error::AcknowledgedButUnusable { native_revision, ref source })
        if native_revision.get() == stored.sequence && matches!(source.as_ref(), Error::Profile))
    );
    timeout(OPERATION_TIMEOUT, fixture.admin.update_stream(profile.clone())).await??;

    let target = manifest(b"actual native ACK outlives the original verification deadline")?;
    let gate = fixture.gates.arm_after_ack().await;
    let owner = Arc::clone(&fixture.archive);
    let requested = target.clone();
    creations.spawn(async move { owner.create_once(requested).await });
    gate.wait_until_entered().await?;
    let stored = fixture
        .stored(&target)
        .await?
        .ok_or_else(|| io::Error::other("acknowledged occurrence is absent before deadline"))?;
    let outcome = finish_create(&mut creations, OPERATION_TIMEOUT + OPERATION_TIMEOUT).await?;
    assert!(
        matches!(outcome, Err(Error::AcknowledgedButUnusable { native_revision, ref source })
        if native_revision.get() == stored.sequence && matches!(source.as_ref(), Error::Deadline(_)))
    );
    assert_eq!(
        fixture
            .stored(&target)
            .await?
            .ok_or_else(|| io::Error::other("original acknowledged bytes were lost"))?
            .payload,
        *target.bytes()
    );

    let target = manifest(b"native create remains suspended through its original deadline")?;
    let before = fixture.head().await?;
    let gate = fixture.gates.arm_before_write().await;
    let owner = Arc::clone(&fixture.archive);
    let requested = target.clone();
    creations.spawn(async move { owner.create_once(requested).await });
    gate.wait_until_entered().await?;
    assert!(fixture.stored(&target).await?.is_none());
    let outcome = finish_create(&mut creations, OPERATION_TIMEOUT + OPERATION_TIMEOUT).await?;
    assert!(matches!(outcome, Err(Error::WriteDeadline(_))));
    assert_eq!(fixture.head().await?, before);
    assert!(fixture.stored(&target).await?.is_none());

    let target = manifest(b"native source disappears before conditional lookup")?;
    let gate = fixture.gates.arm_before_write().await;
    let owner = Arc::clone(&fixture.archive);
    creations.spawn(async move { owner.create_once(target).await });
    gate.wait_until_entered().await?;
    timeout(OPERATION_TIMEOUT, fixture.admin.delete_stream(profile.name.clone())).await??;
    gate.release();
    assert!(matches!(
        finish_create(&mut creations, OPERATION_TIMEOUT).await?,
        Err(Error::WriteUncertain(NativeKvCasError::Lookup(_)))
    ));
    let missing = timeout(OPERATION_TIMEOUT, fixture.admin.get_stream(profile.name)).await?;
    assert!(missing.as_ref().is_err_and(is_get_stream_not_found));
    Ok(())
}
