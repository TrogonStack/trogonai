//! NATS JetStream KV bundle loader with hot-swap and revision rollback (ADR 0026).

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::kv::{self, Operation};
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::bundle_audit::BundleAuditPublisher;
use super::errors::BundleLoadError;
use super::{load_bundle, LoadedBundle, TrustedKeys};

const DEFAULT_RING_CAPACITY: usize = 5;
const DEFAULT_DEBOUNCE_MS: u64 = 0;

#[derive(Debug, Clone)]
pub struct BundleKvLoaderOpts {
    /// KV key holding signed bundle archive bytes (content-addressed or bundle id).
    pub bundle_key: String,
    /// Stable bundle identifier for audit envelopes.
    pub bundle_id: String,
    pub debounce_ms: u64,
    pub ring_capacity: usize,
    pub audit: BundleAuditPublisher,
}

impl BundleKvLoaderOpts {
    pub fn new(
        bundle_key: impl Into<String>,
        bundle_id: impl Into<String>,
        audit: BundleAuditPublisher,
    ) -> Self {
        Self {
            bundle_key: bundle_key.into(),
            bundle_id: bundle_id.into(),
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            ring_capacity: DEFAULT_RING_CAPACITY,
            audit,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BundleStatus {
    pub revision: u64,
    pub loaded_at: SystemTime,
    pub last_swap_at: Option<SystemTime>,
    pub digest_hex: String,
}

struct ActiveState {
    bundle: Arc<LoadedBundle>,
    revision: u64,
    loaded_at: Instant,
    last_swap_at: Option<Instant>,
}

struct RevisionRecord {
    revision: u64,
    archive_bytes: Vec<u8>,
    bundle: Arc<LoadedBundle>,
}

struct RevisionRing {
    capacity: usize,
    records: VecDeque<RevisionRecord>,
}

impl RevisionRing {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            records: VecDeque::new(),
        }
    }

    fn push(&mut self, record: RevisionRecord) {
        if self.records.len() >= self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    fn find(&self, revision: u64) -> Option<&RevisionRecord> {
        self.records.iter().rev().find(|r| r.revision == revision)
    }
}

struct SharedLoaderState {
    active: RwLock<Option<ActiveState>>,
    ring: Mutex<RevisionRing>,
}

pub struct BundleHandle {
    state: Arc<SharedLoaderState>,
    opts: BundleKvLoaderOpts,
    shutdown: Arc<AtomicBool>,
    _watcher: JoinHandle<()>,
}

impl BundleHandle {
    pub fn current(&self) -> Result<Arc<LoadedBundle>, BundleLoadError> {
        let guard = self
            .state
            .active
            .read()
            .map_err(|_| BundleLoadError::BundleNotLoaded)?;
        let active = guard.as_ref().ok_or(BundleLoadError::BundleNotLoaded)?;
        Ok(Arc::clone(&active.bundle))
    }

    pub fn status(&self) -> Result<BundleStatus, BundleLoadError> {
        let guard = self
            .state
            .active
            .read()
            .map_err(|_| BundleLoadError::BundleNotLoaded)?;
        let active = guard.as_ref().ok_or(BundleLoadError::BundleNotLoaded)?;
        Ok(BundleStatus {
            revision: active.revision,
            loaded_at: instant_to_system(active.loaded_at),
            last_swap_at: active.last_swap_at.map(instant_to_system),
            digest_hex: active.bundle.manifest_digest.as_hex().to_string(),
        })
    }

    pub async fn rollback(&self, version_id: u64) -> Result<(), BundleLoadError> {
        let record = {
            let ring = self.state.ring.lock().await;
            ring.find(version_id)
                .map(|r| (r.archive_bytes.clone(), Arc::clone(&r.bundle), r.revision))
        };
        let Some((archive_bytes, bundle, revision)) = record else {
            return Err(BundleLoadError::RevisionNotFound {
                revision: version_id,
            });
        };

        let previous_digest = self.status()?.digest_hex;
        activate_bundle(
            &self.state,
            &self.opts,
            archive_bytes,
            bundle,
            revision,
            true,
        )
        .await?;

        self.opts
            .audit
            .rolled_back(
                &self.opts.bundle_id,
                revision,
                &self.status()?.digest_hex,
                &previous_digest,
            )
            .await;

        Ok(())
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

pub struct BundleKvLoader<S> {
    source: S,
    trusted: TrustedKeys,
    opts: BundleKvLoaderOpts,
}

impl<S> BundleKvLoader<S>
where
    S: BundleKvSource + Send + Sync + 'static,
{
    pub fn new(source: S, trusted: TrustedKeys, opts: BundleKvLoaderOpts) -> Self {
        Self {
            source,
            trusted,
            opts,
        }
    }

    pub async fn start(self) -> Result<BundleHandle, BundleLoadError> {
        let state = Arc::new(SharedLoaderState {
            active: RwLock::new(None),
            ring: Mutex::new(RevisionRing::new(self.opts.ring_capacity)),
        });

        let (bytes, revision) = self
            .source
            .fetch(&self.opts.bundle_key)
            .await?
            .ok_or_else(|| BundleLoadError::KvEmpty {
                key: self.opts.bundle_key.clone(),
            })?;

        let bundle = Arc::new(load_bundle(&bytes, &self.trusted)?);
        activate_bundle(
            &state,
            &self.opts,
            bytes,
            bundle,
            revision,
            false,
        )
        .await?;

        self.opts
            .audit
            .load_succeeded(
                &self.opts.bundle_id,
                revision,
                &state
                    .active
                    .read()
                    .expect("active lock")
                    .as_ref()
                    .expect("cold load activated")
                    .bundle
                    .manifest_digest
                    .as_hex(),
                None,
            )
            .await;

        let watch_stream = self
            .source
            .watch(&self.opts.bundle_key)
            .await
            .map_err(|error| error)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let watcher_opts = self.opts.clone();
        let watcher_trusted = self.trusted.clone();
        let watcher_state = Arc::clone(&state);
        let watcher_shutdown = Arc::clone(&shutdown);

        let watcher_handle = tokio::spawn(async move {
            run_kv_watcher(
                watch_stream,
                watcher_opts,
                watcher_trusted,
                watcher_state,
                watcher_shutdown,
            )
            .await;
        });

        Ok(BundleHandle {
            state,
            opts: self.opts,
            shutdown,
            _watcher: watcher_handle,
        })
    }
}

impl BundleKvLoader<NatsKvSource> {
    pub fn from_store(store: kv::Store, trusted: TrustedKeys, opts: BundleKvLoaderOpts) -> Self {
        Self::new(NatsKvSource::new(store), trusted, opts)
    }
}

async fn activate_bundle(
    state: &SharedLoaderState,
    opts: &BundleKvLoaderOpts,
    archive_bytes: Vec<u8>,
    bundle: Arc<LoadedBundle>,
    revision: u64,
    is_rollback: bool,
) -> Result<(), BundleLoadError> {
    let now = Instant::now();
    let previous_digest = state
        .active
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|a| a.bundle.manifest_digest.as_hex().to_string()));

    {
        let mut guard = state.active.write().map_err(|_| BundleLoadError::BundleNotLoaded)?;
        let had_prior = guard.is_some();
        let loaded_at = guard.as_ref().map(|a| a.loaded_at).unwrap_or(now);
        let last_swap_at = if had_prior || is_rollback {
            Some(now)
        } else {
            None
        };
        *guard = Some(ActiveState {
            bundle: Arc::clone(&bundle),
            revision,
            loaded_at,
            last_swap_at,
        });
    }

    state.ring.lock().await.push(RevisionRecord {
        revision,
        archive_bytes,
        bundle,
    });

    if let (Some(prev), false) = (previous_digest.as_deref(), is_rollback) {
        let digest = state
            .active
            .read()
            .expect("active lock")
            .as_ref()
            .expect("just activated")
            .bundle
            .manifest_digest
            .as_hex()
            .to_string();
        opts.audit
            .hot_swapped(&opts.bundle_id, revision, &digest, prev)
            .await;
    }

    Ok(())
}

async fn try_promote(
    state: &Arc<SharedLoaderState>,
    opts: &BundleKvLoaderOpts,
    trusted: &TrustedKeys,
    bytes: Vec<u8>,
    revision: u64,
) {
    if state
        .active
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|a| a.revision))
        == Some(revision)
    {
        return;
    }

    let previous_digest = state
        .active
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|a| a.bundle.manifest_digest.as_hex().to_string()));

    match load_bundle(&bytes, trusted) {
        Ok(loaded) => {
            if let Err(error) = activate_bundle(
                state,
                opts,
                bytes.clone(),
                Arc::new(loaded),
                revision,
                false,
            )
            .await
            {
                opts.audit
                    .load_failed(&opts.bundle_id, revision, &error)
                    .await;
            }
            let _ = previous_digest;
        }
        Err(error) => {
            opts.audit
                .load_failed(&opts.bundle_id, revision, &error)
                .await;
        }
    }
}

async fn run_kv_watcher(
    mut stream: Pin<Box<dyn futures::Stream<Item = Result<KvEntry, BundleLoadError>> + Send>>,
    opts: BundleKvLoaderOpts,
    trusted: TrustedKeys,
    state: Arc<SharedLoaderState>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::SeqCst) {
        let Some(entry) = stream.next().await else {
            break;
        };
        let Ok(mut entry) = entry else {
            continue;
        };
        if entry.operation == Operation::Delete || entry.operation == Operation::Purge {
            continue;
        }

        if opts.debounce_ms > 0 {
            let debounce = tokio::time::Duration::from_millis(opts.debounce_ms);
            let mut deadline = tokio::time::Instant::now() + debounce;
            while tokio::time::Instant::now() < deadline {
                if shutdown.load(Ordering::SeqCst) {
                    return;
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                match tokio::time::timeout(remaining, stream.next()).await {
                    Ok(Some(Ok(later)))
                        if later.operation != Operation::Delete
                            && later.operation != Operation::Purge =>
                    {
                        entry = later;
                        deadline = tokio::time::Instant::now() + debounce;
                    }
                    _ => break,
                }
            }
        }

        try_promote(&state, &opts, &trusted, entry.value, entry.revision).await;
    }
}

fn instant_to_system(instant: Instant) -> SystemTime {
    let elapsed = instant.elapsed();
    SystemTime::now()
        .checked_sub(elapsed)
        .unwrap_or(UNIX_EPOCH)
}

#[derive(Debug, Clone)]
pub struct KvEntry {
    pub value: Vec<u8>,
    pub revision: u64,
    pub operation: Operation,
}

#[async_trait]
pub trait BundleKvSource: Send + Sync {
    async fn fetch(&self, key: &str) -> Result<Option<(Vec<u8>, u64)>, BundleLoadError>;
    async fn watch(
        &self,
        key: &str,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<KvEntry, BundleLoadError>> + Send>>, BundleLoadError>;
    fn clone_source(&self) -> Self
    where
        Self: Sized;
}

#[derive(Clone)]
pub struct NatsKvSource {
    store: kv::Store,
}

impl NatsKvSource {
    pub fn new(store: kv::Store) -> Self {
        Self { store }
    }
}

#[async_trait]
impl BundleKvSource for NatsKvSource {
    async fn fetch(&self, key: &str) -> Result<Option<(Vec<u8>, u64)>, BundleLoadError> {
        match self.store.entry(key).await {
            Ok(Some(entry))
                if entry.operation != Operation::Delete && entry.operation != Operation::Purge =>
            {
                Ok(Some((entry.value.to_vec(), entry.revision)))
            }
            Ok(_) => Ok(None),
            Err(error) => Err(BundleLoadError::KvFetch(error.to_string())),
        }
    }

    async fn watch(
        &self,
        key: &str,
    ) -> Result<Pin<Box<dyn futures::Stream<Item = Result<KvEntry, BundleLoadError>> + Send>>, BundleLoadError>
    {
        let watch = self
            .store
            .watch(key)
            .await
            .map_err(|error| BundleLoadError::KvWatch(error.to_string()))?;
        let mapped = watch.map(|result| {
            result
                .map(|entry| KvEntry {
                    value: entry.value.to_vec(),
                    revision: entry.revision,
                    operation: entry.operation,
                })
                .map_err(|error| BundleLoadError::KvWatch(error.to_string()))
        });
        Ok(Box::pin(mapped))
    }

    fn clone_source(&self) -> Self {
        Self {
            store: self.store.clone(),
        }
    }
}

#[allow(dead_code)]
pub mod fake_kv {
    use std::collections::HashMap;

    use futures::stream;
    use tokio::sync::{mpsc, RwLock};

    use super::*;

    #[derive(Clone, Default)]
    pub struct FakeBundleKv {
        inner: Arc<RwLock<HashMap<String, Vec<(u64, Vec<u8>)>>>>,
        revision: Arc<RwLock<u64>>,
        watchers: Arc<RwLock<HashMap<String, Vec<mpsc::UnboundedSender<KvEntry>>>>>,
    }

    impl FakeBundleKv {
        pub async fn put(&self, key: &str, value: Vec<u8>) -> u64 {
            let mut revision = self.revision.write().await;
            *revision += 1;
            let rev = *revision;
            self.inner
                .write()
                .await
                .entry(key.to_string())
                .or_default()
                .push((rev, value.clone()));
            let entry = KvEntry {
                value,
                revision: rev,
                operation: Operation::Put,
            };
            let watchers = self.watchers.read().await;
            if let Some(senders) = watchers.get(key) {
                for sender in senders {
                    let _ = sender.send(entry.clone());
                }
            }
            rev
        }

        async fn latest(&self, key: &str) -> Option<(Vec<u8>, u64)> {
            let map = self.inner.read().await;
            map.get(key).and_then(|history| {
                history.last().map(|(rev, bytes)| (bytes.clone(), *rev))
            })
        }

        async fn register_watch(&self, key: &str) -> mpsc::UnboundedReceiver<KvEntry> {
            let (tx, rx) = mpsc::unbounded_channel();
            self.watchers
                .write()
                .await
                .entry(key.to_string())
                .or_default()
                .push(tx);
            rx
        }
    }

    #[derive(Clone)]
    pub struct FakeKvSource {
        kv: FakeBundleKv,
        key: String,
    }

    impl FakeKvSource {
        pub fn new(kv: FakeBundleKv, key: impl Into<String>) -> Self {
            Self {
                kv,
                key: key.into(),
            }
        }
    }

    #[async_trait]
    impl BundleKvSource for FakeKvSource {
        async fn fetch(&self, key: &str) -> Result<Option<(Vec<u8>, u64)>, BundleLoadError> {
            Ok(self.kv.latest(key).await)
        }

        async fn watch(
            &self,
            key: &str,
        ) -> Result<
            Pin<Box<dyn futures::Stream<Item = Result<KvEntry, BundleLoadError>> + Send>>,
            BundleLoadError,
        > {
            let rx = self.kv.register_watch(key).await;
            let stream = stream::unfold(rx, |mut rx| async move {
                match rx.recv().await {
                    Some(entry) => Some((Ok(entry), rx)),
                    None => None,
                }
            });
            Ok(Box::pin(stream))
        }

        fn clone_source(&self) -> Self {
            Self {
                kv: self.kv.clone(),
                key: self.key.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nkeys::KeyPair;

    use super::fake_kv::{FakeBundleKv, FakeKvSource};
    use super::*;
    use crate::bundle::bundle_audit::{EVENT_HOT_SWAPPED, EVENT_LOAD_FAILED, EVENT_LOAD_SUCCEEDED, EVENT_ROLLED_BACK};
    use crate::bundle::{
        build_tar, hash_member, manifest_digest_bytes, signature_path, BundleArchive, HOST_TARGET_WIT,
        MANIFEST_FILENAME,
    };

    fn manifest_toml(nkey_pub: &str, cel_hash: &str, version: &str) -> String {
        format!(
            r#"
name = "acme/demo"
version = "{version}"
target_wit = "{HOST_TARGET_WIT}"
min_gateway_version = "0.0.1"
author = "platform"
created_at = "2026-05-28T00:00:00Z"
description = "demo"

[signing]
nkey_pub = "{nkey_pub}"

[[programs]]
id = "rule"
path = "policies/rule.cel"
sha256 = "{cel_hash}"
class = "ingress_gate"
effect = "allow"
priority = 1
"#
        )
    }

    fn signed_tar(kp: &KeyPair, cel_body: &[u8], version: &str) -> Vec<u8> {
        let hash = hash_member(cel_body);
        let manifest = manifest_toml(&kp.public_key(), &hash, version);
        let manifest_bytes = manifest.into_bytes();
        let sig = kp.sign(&manifest_digest_bytes(&manifest_bytes)).expect("sign");
        let mut archive = BundleArchive::default();
        archive.insert(MANIFEST_FILENAME, manifest_bytes);
        archive.insert("policies/rule.cel", cel_body.to_vec());
        archive.insert(signature_path(), sig);
        build_tar(&archive)
    }

    fn trusted_for(kp: &KeyPair) -> TrustedKeys {
        TrustedKeys::from_allowlist([kp.public_key()])
    }

    #[tokio::test]
    async fn cold_load_happy_path() {
        let kp = KeyPair::new_user();
        let kv = FakeBundleKv::default();
        let key = "acme/demo";
        kv.put(key, signed_tar(&kp, b"true", "1.0.0")).await;
        let (audit, records) = BundleAuditPublisher::recording();
        let opts = BundleKvLoaderOpts::new(key, "acme/demo", audit);
        let loader = BundleKvLoader::new(FakeKvSource::new(kv, key), trusted_for(&kp), opts);
        let handle = loader.start().await.expect("cold load");
        let bundle = handle.current().expect("active");
        assert_eq!(bundle.manifest.version, "1.0.0");
        let events = records.lock().await;
        assert!(events.iter().any(|e| e.event == EVENT_LOAD_SUCCEEDED));
    }

    #[tokio::test]
    async fn startup_fails_when_kv_empty() {
        let kp = KeyPair::new_user();
        let kv = FakeBundleKv::default();
        let key = "missing";
        let (audit, _) = BundleAuditPublisher::recording();
        let opts = BundleKvLoaderOpts::new(key, "acme/demo", audit);
        let loader = BundleKvLoader::new(FakeKvSource::new(kv, key), trusted_for(&kp), opts);
        assert!(matches!(
            loader.start().await,
            Err(BundleLoadError::KvEmpty { .. })
        ));
    }

    #[tokio::test]
    async fn watcher_swaps_on_kv_update() {
        let kp = KeyPair::new_user();
        let kv = FakeBundleKv::default();
        let key = "acme/demo";
        kv.put(key, signed_tar(&kp, b"v1", "1.0.0")).await;
        let (audit, records) = BundleAuditPublisher::recording();
        let mut opts = BundleKvLoaderOpts::new(key, "acme/demo", audit);
        opts.debounce_ms = 10;
        let loader = BundleKvLoader::new(FakeKvSource::new(kv.clone(), key), trusted_for(&kp), opts);
        let handle = loader.start().await.expect("cold load");
        assert_eq!(handle.current().expect("v1").manifest.version, "1.0.0");

        kv.put(key, signed_tar(&kp, b"v2", "2.0.0")).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(handle.current().expect("v2").manifest.version, "2.0.0");
        let events = records.lock().await;
        assert!(events.iter().any(|e| e.event == EVENT_HOT_SWAPPED));
    }

    #[tokio::test]
    async fn validation_failure_keeps_previous_bundle() {
        let kp = KeyPair::new_user();
        let kv = FakeBundleKv::default();
        let key = "acme/demo";
        kv.put(key, signed_tar(&kp, b"good", "1.0.0")).await;
        let (audit, records) = BundleAuditPublisher::recording();
        let mut opts = BundleKvLoaderOpts::new(key, "acme/demo", audit);
        opts.debounce_ms = 5;
        let loader = BundleKvLoader::new(FakeKvSource::new(kv.clone(), key), trusted_for(&kp), opts);
        let handle = loader.start().await.expect("cold load");
        let digest_v1 = handle.status().expect("status").digest_hex;

        let bad = signed_tar(&kp, b"bad", "2.0.0");
        let mut archive = crate::bundle::extract_archive(&bad).expect("extract");
        archive.insert(
            MANIFEST_FILENAME,
            manifest_toml(&kp.public_key(), "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc", "2.0.0").into_bytes(),
        );
        let bad_tar = build_tar(&archive);
        kv.put(key, bad_tar).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;

        assert_eq!(handle.status().expect("status").digest_hex, digest_v1);
        let events = records.lock().await;
        assert!(events.iter().any(|e| e.event == EVENT_LOAD_FAILED));
        assert!(!events.iter().any(|e| e.event == EVENT_HOT_SWAPPED && e.fields.revision > 1));
    }

    #[tokio::test]
    async fn rollback_restores_prior_revision_without_refetch() {
        let kp = KeyPair::new_user();
        let kv = FakeBundleKv::default();
        let key = "acme/demo";
        let rev1 = kv.put(key, signed_tar(&kp, b"v1", "1.0.0")).await;
        let (audit, records) = BundleAuditPublisher::recording();
        let mut opts = BundleKvLoaderOpts::new(key, "acme/demo", audit);
        opts.debounce_ms = 5;
        let loader = BundleKvLoader::new(FakeKvSource::new(kv.clone(), key), trusted_for(&kp), opts);
        let handle = loader.start().await.expect("cold load");
        let rev2 = kv.put(key, signed_tar(&kp, b"v2", "2.0.0")).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
        assert_eq!(handle.current().expect("v2").manifest.version, "2.0.0");

        handle.rollback(rev1).await.expect("rollback");
        assert_eq!(handle.current().expect("v1").manifest.version, "1.0.0");
        assert_eq!(handle.status().expect("status").revision, rev1);
        let _ = rev2;
        let events = records.lock().await;
        assert!(events.iter().any(|e| e.event == EVENT_ROLLED_BACK));
    }

    #[tokio::test]
    async fn audit_emitted_on_each_transition() {
        let kp = KeyPair::new_user();
        let kv = FakeBundleKv::default();
        let key = "acme/demo";
        kv.put(key, signed_tar(&kp, b"v1", "1.0.0")).await;
        let (audit, records) = BundleAuditPublisher::recording();
        let mut opts = BundleKvLoaderOpts::new(key, "acme/demo", audit);
        opts.debounce_ms = 5;
        let loader = BundleKvLoader::new(FakeKvSource::new(kv.clone(), key), trusted_for(&kp), opts);
        let handle = loader.start().await.expect("start");
        let rev1 = handle.status().expect("status").revision;
        let rev2 = kv.put(key, signed_tar(&kp, b"v2", "2.0.0")).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(40)).await;
        handle.rollback(rev1).await.expect("rollback");
        let events = records.lock().await;
        let kinds: Vec<_> = events.iter().map(|e| e.event).collect();
        assert!(kinds.contains(&EVENT_LOAD_SUCCEEDED));
        assert!(kinds.contains(&EVENT_HOT_SWAPPED));
        assert!(kinds.contains(&EVENT_ROLLED_BACK));
        let _ = rev2;
    }
}
