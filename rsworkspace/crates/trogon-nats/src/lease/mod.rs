use std::time::Duration;

mod acquire;
mod lease_bucket;
mod lease_config_error;
mod lease_key;
mod lease_timing;
mod nats_kv_lease_config;
mod provision;
mod release;
mod renew;
mod renew_interval;
pub mod traits;
mod ttl;

#[cfg(test)]
use async_nats::header::{NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, NATS_MESSAGE_TTL};
use async_nats::jetstream::context::{CreateKeyValueError, KeyValueError};
#[cfg(test)]
use async_nats::jetstream::context::{CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind};
use async_nats::jetstream::kv;
use bytes::Bytes;
#[cfg(test)]
use provision::{
    KeyValueSettings, create_bucket_error, inspect_bucket_error, is_create_key_value_already_exists,
    open_existing_bucket_error, validate_bucket_settings,
};
use renew::KvPublishTarget;
#[cfg(test)]
use renew::{build_kv_subject, build_update_headers};
#[cfg(test)]
use trogon_std::time::GetNow;
#[cfg(test)]
use trogon_std::time::MockClock;
use trogon_std::time::{GetElapsed, SystemClock};

pub use lease_bucket::LeaseBucket;
pub use lease_config_error::LeaseConfigError;
pub use lease_key::LeaseKey;
pub use lease_timing::LeaseTiming;
pub use nats_kv_lease_config::NatsKvLeaseConfig;
pub use renew_interval::{LeaseRenewInterval, LeaseRenewIntervalError};
pub use traits::{ReleaseLease, RenewLease, TryAcquireLease};
pub use ttl::{LeaseTtl, LeaseTtlError};

#[derive(Debug)]
pub enum EnsureLeaderError {
    Acquire(kv::CreateError),
    Renew(kv::UpdateError),
}

impl std::fmt::Display for EnsureLeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Acquire(source) => write!(f, "failed to acquire lease: {source}"),
            Self::Renew(source) => write!(f, "failed to renew lease: {source}"),
        }
    }
}

impl std::error::Error for EnsureLeaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Acquire(source) => Some(source),
            Self::Renew(source) => Some(source),
        }
    }
}

#[derive(Debug)]
pub enum LeaseError {
    Provision {
        context: &'static str,
        source: LeaseProvisionError,
    },
    IncompatibleBucketConfig {
        source: IncompatibleLeaseBucketConfig,
    },
}

impl LeaseError {
    fn provision_source(context: &'static str, source: LeaseProvisionError) -> Self {
        Self::Provision { context, source }
    }
}

#[derive(Debug)]
pub enum LeaseProvisionError {
    CreateBucket(CreateKeyValueError),
    OpenExistingBucket(KeyValueError),
    InspectBucket(kv::StatusError),
}

impl std::fmt::Display for LeaseProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateBucket(source) => write!(f, "{source}"),
            Self::OpenExistingBucket(source) => write!(f, "{source}"),
            Self::InspectBucket(source) => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for LeaseProvisionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateBucket(source) => Some(source),
            Self::OpenExistingBucket(source) => Some(source),
            Self::InspectBucket(source) => Some(source),
        }
    }
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Provision { context, source } => {
                write!(f, "lease provision error: {context}: {source}")
            }
            Self::IncompatibleBucketConfig { source } => {
                write!(f, "lease provision error: incompatible bucket config: {source}")
            }
        }
    }
}

impl std::error::Error for LeaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Provision { source, .. } => Some(source),
            Self::IncompatibleBucketConfig { source } => Some(source),
        }
    }
}

#[derive(Debug)]
pub struct IncompatibleLeaseBucketConfig {
    expected_history: i64,
    actual_history: i64,
    expected_max_age: Duration,
    actual_max_age: Duration,
    expected_allow_message_ttl: bool,
    actual_allow_message_ttl: bool,
    expected_subject_delete_marker_ttl: Option<Duration>,
    actual_subject_delete_marker_ttl: Option<Duration>,
}

impl std::fmt::Display for IncompatibleLeaseBucketConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "expected history={}, max_age={:?}, allow_message_ttl={}, subject_delete_marker_ttl={:?}, got history={}, max_age={:?}, allow_message_ttl={}, subject_delete_marker_ttl={:?}",
            self.expected_history,
            self.expected_max_age,
            self.expected_allow_message_ttl,
            self.expected_subject_delete_marker_ttl,
            self.actual_history,
            self.actual_max_age,
            self.actual_allow_message_ttl,
            self.actual_subject_delete_marker_ttl
        )
    }
}

impl std::error::Error for IncompatibleLeaseBucketConfig {}

#[derive(Clone)]
pub struct NatsKvLease {
    store: kv::Store,
    key: LeaseKey,
    ttl: Option<Duration>,
    publish_target: KvPublishTarget,
    js_context: Option<async_nats::jetstream::Context>,
}

impl NatsKvLease {
    pub fn new(store: kv::Store, key: LeaseKey) -> Self {
        let publish_target = KvPublishTarget::from_store(&store);
        Self {
            store,
            key,
            ttl: None,
            publish_target,
            js_context: None,
        }
    }

    pub fn try_new(store: kv::Store, key: impl Into<String>) -> Result<Self, LeaseConfigError> {
        Ok(Self::new(store, LeaseKey::new(key)?))
    }
}

pub struct LeaderElection<L, C = SystemClock>
where
    L: TryAcquireLease<Error = kv::CreateError>
        + RenewLease<Error = kv::UpdateError>
        + ReleaseLease<Error = kv::DeleteError>,
    C: GetElapsed,
{
    lock: L,
    node_id: String,
    is_leader: bool,
    last_renewed: Option<C::Instant>,
    current_revision: Option<u64>,
    timing: LeaseTiming,
    clock: C,
}

impl<L> LeaderElection<L, SystemClock>
where
    L: TryAcquireLease<Error = kv::CreateError>
        + RenewLease<Error = kv::UpdateError>
        + ReleaseLease<Error = kv::DeleteError>,
{
    pub fn new(lock: L, node_id: String, timing: LeaseTiming) -> Self {
        Self {
            lock,
            node_id,
            is_leader: false,
            last_renewed: None,
            current_revision: None,
            timing,
            clock: SystemClock,
        }
    }
}

impl<L, C> LeaderElection<L, C>
where
    L: TryAcquireLease<Error = kv::CreateError>
        + RenewLease<Error = kv::UpdateError>
        + ReleaseLease<Error = kv::DeleteError>,
    C: GetElapsed,
{
    pub fn with_clock(lock: L, node_id: String, timing: LeaseTiming, clock: C) -> Self {
        Self {
            lock,
            node_id,
            is_leader: false,
            last_renewed: None,
            current_revision: None,
            timing,
            clock,
        }
    }
    pub fn is_leader(&self) -> bool {
        self.is_leader
    }

    pub async fn ensure_leader(&mut self) -> Result<bool, EnsureLeaderError> {
        if self.is_leader {
            self.maybe_renew().await?;
        } else {
            self.try_acquire().await?;
        }

        Ok(self.is_leader)
    }

    pub async fn release(&mut self) -> Result<(), kv::DeleteError> {
        if !self.is_leader {
            return Ok(());
        }

        let Some(revision) = self.current_revision else {
            self.clear_state();
            return Ok(());
        };

        let result = self.lock.release(revision).await;
        self.clear_state();
        match result {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == kv::DeleteErrorKind::WrongLastRevision => Ok(()),
            Err(source) => Err(source),
        }
    }

    fn clear_state(&mut self) {
        self.is_leader = false;
        self.last_renewed = None;
        self.current_revision = None;
    }

    async fn try_acquire(&mut self) -> Result<(), EnsureLeaderError> {
        let value = Bytes::from(self.node_id.clone());
        match self.lock.try_acquire(value).await {
            Ok(revision) => {
                self.is_leader = true;
                self.current_revision = Some(revision);
                self.last_renewed = Some(self.clock.now());
            }
            Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => {
                self.clear_state();
            }
            Err(source) => {
                self.clear_state();
                return Err(EnsureLeaderError::Acquire(source));
            }
        }
        Ok(())
    }

    async fn maybe_renew(&mut self) -> Result<(), EnsureLeaderError> {
        if !self.should_renew() {
            return Ok(());
        }

        let Some(revision) = self.current_revision else {
            self.clear_state();
            return Ok(());
        };

        let value = Bytes::from(self.node_id.clone());
        match self.lock.renew(value, revision).await {
            Ok(new_revision) => {
                self.current_revision = Some(new_revision);
                self.last_renewed = Some(self.clock.now());
                Ok(())
            }
            Err(error) => {
                self.clear_state();
                Err(EnsureLeaderError::Renew(error))
            }
        }
    }

    fn should_renew(&self) -> bool {
        match self.last_renewed {
            None => true,
            Some(renewed_at) => Self::renew_interval_elapsed(&self.clock, renewed_at, self.timing.renew_interval()),
        }
    }

    fn renew_interval_elapsed(clock: &C, renewed_at: C::Instant, renew_interval: Duration) -> bool {
        clock.elapsed(renewed_at) >= renew_interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };

    #[derive(Clone, Debug)]
    enum AcquireBehavior {
        Acquired,
        HeldByOther,
        Error,
    }

    #[derive(Clone, Debug)]
    enum ReleaseBehavior {
        Ok,
        WrongLastRevision,
        Error,
    }

    #[derive(Clone)]
    struct MockLease {
        acquire_behavior: Arc<Mutex<AcquireBehavior>>,
        renew_error: Arc<Mutex<bool>>,
        release_behavior: Arc<Mutex<ReleaseBehavior>>,
        released_revisions: Arc<Mutex<Vec<u64>>>,
        next_revision: Arc<AtomicU64>,
    }

    impl Default for MockLease {
        fn default() -> Self {
            Self {
                acquire_behavior: Arc::new(Mutex::new(AcquireBehavior::Acquired)),
                renew_error: Arc::new(Mutex::new(false)),
                release_behavior: Arc::new(Mutex::new(ReleaseBehavior::Ok)),
                released_revisions: Arc::new(Mutex::new(Vec::new())),
                next_revision: Arc::new(AtomicU64::new(1)),
            }
        }
    }

    impl MockLease {
        fn new() -> Self {
            Self::default()
        }

        fn hold_by_other(&self) {
            *self.acquire_behavior.lock().unwrap() = AcquireBehavior::HeldByOther;
        }

        fn fail_acquire(&self) {
            *self.acquire_behavior.lock().unwrap() = AcquireBehavior::Error;
        }

        fn allow_acquire(&self) {
            *self.acquire_behavior.lock().unwrap() = AcquireBehavior::Acquired;
        }

        fn fail_renew(&self) {
            *self.renew_error.lock().unwrap() = true;
        }

        fn fail_release(&self) {
            *self.release_behavior.lock().unwrap() = ReleaseBehavior::Error;
        }

        fn wrong_revision_on_release(&self) {
            *self.release_behavior.lock().unwrap() = ReleaseBehavior::WrongLastRevision;
        }

        fn released_revisions(&self) -> Vec<u64> {
            self.released_revisions.lock().unwrap().clone()
        }
    }

    impl TryAcquireLease for MockLease {
        type Error = kv::CreateError;

        async fn try_acquire(&self, _value: Bytes) -> Result<u64, Self::Error> {
            match &*self.acquire_behavior.lock().unwrap() {
                AcquireBehavior::Acquired => Ok(self.next_revision.fetch_add(1, Ordering::SeqCst)),
                AcquireBehavior::HeldByOther => Err(kv::CreateError::new(kv::CreateErrorKind::AlreadyExists)),
                AcquireBehavior::Error => Err(kv::CreateError::new(kv::CreateErrorKind::Other)),
            }
        }
    }

    impl RenewLease for MockLease {
        type Error = kv::UpdateError;

        async fn renew(&self, _value: Bytes, revision: u64) -> Result<u64, Self::Error> {
            if *self.renew_error.lock().unwrap() {
                Err(kv::UpdateError::new(kv::UpdateErrorKind::Other))
            } else {
                Ok(revision + 1)
            }
        }
    }

    impl ReleaseLease for MockLease {
        type Error = kv::DeleteError;

        async fn release(&self, revision: u64) -> Result<(), Self::Error> {
            self.released_revisions.lock().unwrap().push(revision);
            match &*self.release_behavior.lock().unwrap() {
                ReleaseBehavior::Ok => Ok(()),
                ReleaseBehavior::WrongLastRevision => Err(kv::DeleteError::new(kv::DeleteErrorKind::WrongLastRevision)),
                ReleaseBehavior::Error => Err(kv::DeleteError::new(kv::DeleteErrorKind::Other)),
            }
        }
    }

    fn ttl(secs: u64) -> LeaseTtl {
        LeaseTtl::from_secs(secs).unwrap()
    }

    fn renew_interval(secs: u64) -> LeaseRenewInterval {
        LeaseRenewInterval::from_secs(secs).unwrap()
    }

    fn lease_timing() -> LeaseTiming {
        LeaseTiming::new(ttl(10), renew_interval(5)).unwrap()
    }

    fn stream_exists_error() -> CreateStreamError {
        let source: async_nats::jetstream::Error = serde_json::from_str(
            r#"{"code":400,"err_code":10058,"description":"stream name already in use with a different configuration"}"#,
        )
        .unwrap();

        CreateStreamError::new(CreateStreamErrorKind::JetStream(source))
    }

    #[test]
    fn lease_config_rejects_invalid_bucket_name() {
        let error = NatsKvLeaseConfig::new("invalid.bucket", "key", ttl(10), renew_interval(5)).unwrap_err();

        assert!(matches!(error, LeaseConfigError::InvalidBucketName(_)));
    }

    #[test]
    fn lease_bucket_and_key_accessors_return_original_values() {
        let bucket = LeaseBucket::new("bucket_name").unwrap();
        let key = LeaseKey::new("lease/key").unwrap();
        assert_eq!(bucket.as_str(), "bucket_name");
        assert_eq!(key.as_str(), "lease/key");
    }

    #[test]
    fn lease_bucket_and_key_reject_empty_values() {
        let bucket_error = LeaseBucket::new("").unwrap_err();
        let key_error = LeaseKey::new("").unwrap_err();

        assert_eq!(bucket_error, LeaseConfigError::EmptyBucket);
        assert_eq!(key_error, LeaseConfigError::EmptyKey);
    }

    #[test]
    fn lease_config_rejects_invalid_key_name() {
        let error = NatsKvLeaseConfig::new("bucket", "invalid key", ttl(10), renew_interval(5)).unwrap_err();

        assert!(matches!(error, LeaseConfigError::InvalidKeyName(_)));
    }

    #[test]
    fn lease_config_accessors_expose_expected_values() {
        let config = NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap();
        assert_eq!(config.bucket().as_str(), "bucket");
        assert_eq!(config.key().as_str(), "key");
        assert_eq!(config.timing().ttl(), Duration::from_secs(10));
        assert_eq!(config.ttl(), Duration::from_secs(10));
        assert_eq!(config.renew_interval(), Duration::from_secs(5));
    }

    #[test]
    fn lease_config_rejects_renew_interval_not_less_than_ttl() {
        let error = NatsKvLeaseConfig::new("bucket", "key", ttl(5), renew_interval(5)).unwrap_err();

        assert!(matches!(error, LeaseConfigError::RenewIntervalNotLessThanTtl { .. }));
    }

    #[tokio::test]
    async fn acquires_leadership_when_lease_is_free() {
        let lease = MockLease::new();
        let mut election = LeaderElection::new(lease, "node-1".to_string(), lease_timing());

        assert!(!election.is_leader());
        assert!(election.ensure_leader().await.unwrap());
        assert!(election.is_leader());
    }

    #[tokio::test]
    async fn remains_follower_when_lease_is_held_by_other() {
        let lease = MockLease::new();
        lease.hold_by_other();
        let mut election = LeaderElection::new(lease, "node-2".to_string(), lease_timing());

        assert!(!election.ensure_leader().await.unwrap());
        assert!(!election.is_leader());
    }

    #[tokio::test]
    async fn does_not_renew_before_interval() {
        let lease = MockLease::new();
        let clock = MockClock::new();
        let mut election =
            LeaderElection::with_clock(lease.clone(), "node-3".to_string(), lease_timing(), clock.clone());

        assert!(election.ensure_leader().await.unwrap());
        lease.fail_renew();
        clock.advance(Duration::from_secs(4));

        assert!(election.ensure_leader().await.unwrap());
        assert!(election.is_leader());
    }

    #[tokio::test]
    async fn renews_when_interval_elapsed() {
        let lease = MockLease::new();
        let clock = MockClock::new();
        let mut election =
            LeaderElection::with_clock(lease.clone(), "node-renew".to_string(), lease_timing(), clock.clone());

        assert!(election.ensure_leader().await.unwrap());
        let previous = election.current_revision;
        clock.advance(Duration::from_secs(5));
        assert!(election.ensure_leader().await.unwrap());

        assert!(election.is_leader());
        assert!(election.current_revision > previous);
    }

    #[tokio::test]
    async fn loses_leadership_when_renew_fails() {
        let lease = MockLease::new();
        let clock = MockClock::new();
        let mut election =
            LeaderElection::with_clock(lease.clone(), "node-4".to_string(), lease_timing(), clock.clone());

        assert!(election.ensure_leader().await.unwrap());
        lease.fail_renew();
        clock.advance(Duration::from_secs(5));

        let error = election.ensure_leader().await.unwrap_err();
        assert!(matches!(
            error,
            EnsureLeaderError::Renew(source) if source.kind() == kv::UpdateErrorKind::Other
        ));
        assert!(!election.is_leader());
        assert!(election.current_revision.is_none());
    }

    #[tokio::test]
    async fn release_clears_revision_and_leader_state() {
        let lease = MockLease::new();
        let mut election = LeaderElection::new(lease.clone(), "node-5".to_string(), lease_timing());

        assert!(election.ensure_leader().await.unwrap());
        assert!(election.current_revision.is_some());

        election.release().await.unwrap();
        assert_eq!(lease.released_revisions(), vec![1]);
        assert!(!election.is_leader());
        assert!(election.current_revision.is_none());
    }

    #[tokio::test]
    async fn can_reacquire_after_release() {
        let lease = MockLease::new();
        let mut election = LeaderElection::new(lease.clone(), "node-6".to_string(), lease_timing());

        assert!(election.ensure_leader().await.unwrap());
        election.release().await.unwrap();

        lease.allow_acquire();
        assert!(election.ensure_leader().await.unwrap());
        assert!(election.is_leader());
    }

    #[tokio::test]
    async fn acquire_error_is_propagated_without_marking_leader() {
        let lease = MockLease::new();
        lease.fail_acquire();
        let mut election = LeaderElection::new(lease, "node-7".to_string(), lease_timing());

        let error = election.ensure_leader().await.unwrap_err();
        assert!(matches!(
            error,
            EnsureLeaderError::Acquire(source) if source.kind() == kv::CreateErrorKind::Other
        ));
        assert!(!election.is_leader());
    }

    #[tokio::test]
    async fn release_error_clears_local_state() {
        let lease = MockLease::new();
        let mut election = LeaderElection::new(lease.clone(), "node-8".to_string(), lease_timing());

        assert!(election.ensure_leader().await.unwrap());
        lease.fail_release();

        let error = election.release().await.unwrap_err();
        assert_eq!(error.kind(), kv::DeleteErrorKind::Other);
        assert!(!election.is_leader());
        assert!(election.current_revision.is_none());
    }

    #[tokio::test]
    async fn release_treats_wrong_last_revision_as_success() {
        let lease = MockLease::new();
        let mut election = LeaderElection::new(lease.clone(), "node-10".to_string(), lease_timing());

        assert!(election.ensure_leader().await.unwrap());
        lease.wrong_revision_on_release();
        assert!(election.release().await.is_ok());
        assert!(!election.is_leader());
    }

    #[tokio::test]
    async fn release_without_revision_clears_local_state_without_calling_lock() {
        let lease = MockLease::new();
        let clock = MockClock::new();
        let mut election =
            LeaderElection::with_clock(lease.clone(), "node-9".to_string(), lease_timing(), clock.clone());
        election.is_leader = true;
        election.current_revision = None;
        election.last_renewed = Some(clock.now());

        election.release().await.unwrap();

        assert!(!election.is_leader());
        assert!(election.current_revision.is_none());
        assert!(lease.released_revisions().is_empty());
    }

    #[tokio::test]
    async fn release_noops_when_not_leader() {
        let lease = MockLease::new();
        let mut election = LeaderElection::new(lease.clone(), "node-noop".to_string(), lease_timing());

        election.release().await.unwrap();

        assert!(!election.is_leader());
        assert!(lease.released_revisions().is_empty());
    }

    #[tokio::test]
    async fn maybe_renew_without_revision_clears_state() {
        let lease = MockLease::new();
        let mut election = LeaderElection::new(lease.clone(), "node-missing-rev".to_string(), lease_timing());

        assert!(election.ensure_leader().await.unwrap());
        election.current_revision = None;
        election.last_renewed = None;

        assert!(!election.ensure_leader().await.unwrap());
        assert!(!election.is_leader());
        assert!(election.current_revision.is_none());
    }

    #[test]
    fn create_key_value_already_exists_matches_only_wrapped_stream_exists() {
        let error = CreateKeyValueError::with_source(CreateKeyValueErrorKind::BucketCreate, stream_exists_error());

        assert!(is_create_key_value_already_exists(&error));

        let timed_out = CreateKeyValueError::new(CreateKeyValueErrorKind::TimedOut);
        assert!(!is_create_key_value_already_exists(&timed_out));
    }

    #[test]
    fn validate_bucket_settings_rejects_incompatible_history() {
        let config = NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap();

        let error = validate_bucket_settings(
            KeyValueSettings {
                history: 2,
                max_age: Duration::ZERO,
                allow_message_ttl: true,
                subject_delete_marker_ttl: Some(config.ttl()),
            },
            &config,
        )
        .unwrap_err();

        assert!(matches!(error, LeaseError::IncompatibleBucketConfig { .. }));
    }

    #[test]
    fn validate_bucket_settings_rejects_incompatible_max_age() {
        let config = NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap();

        let error = validate_bucket_settings(
            KeyValueSettings {
                history: 1,
                max_age: Duration::from_secs(20),
                allow_message_ttl: true,
                subject_delete_marker_ttl: Some(config.ttl()),
            },
            &config,
        )
        .unwrap_err();

        assert!(matches!(error, LeaseError::IncompatibleBucketConfig { .. }));
    }

    #[test]
    fn validate_bucket_settings_rejects_missing_allow_message_ttl() {
        let config = NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap();

        let error = validate_bucket_settings(
            KeyValueSettings {
                history: 1,
                max_age: Duration::ZERO,
                allow_message_ttl: false,
                subject_delete_marker_ttl: Some(config.ttl()),
            },
            &config,
        )
        .unwrap_err();

        assert!(matches!(error, LeaseError::IncompatibleBucketConfig { .. }));
    }

    #[test]
    fn validate_bucket_settings_rejects_mismatched_subject_delete_marker_ttl() {
        let config = NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap();

        let error = validate_bucket_settings(
            KeyValueSettings {
                history: 1,
                max_age: Duration::ZERO,
                allow_message_ttl: true,
                subject_delete_marker_ttl: Some(Duration::from_secs(20)),
            },
            &config,
        )
        .unwrap_err();

        assert!(matches!(error, LeaseError::IncompatibleBucketConfig { .. }));
    }

    #[test]
    fn validate_bucket_settings_accepts_strict_per_message_ttl_configuration() {
        let config = NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap();

        validate_bucket_settings(
            KeyValueSettings {
                history: 1,
                max_age: Duration::ZERO,
                allow_message_ttl: true,
                subject_delete_marker_ttl: Some(config.ttl()),
            },
            &config,
        )
        .unwrap();
    }

    #[test]
    fn build_kv_subject_prefers_put_prefix_when_present() {
        let subject = build_kv_subject(Some("$KV.bucket."), "$KV.default.", "key");
        assert_eq!(subject, "$KV.bucket.key");
    }

    #[test]
    fn build_kv_subject_uses_default_prefix_when_put_prefix_is_missing() {
        let subject = build_kv_subject(None, "$KV.bucket.", "key");
        assert_eq!(subject, "$KV.bucket.key");
    }

    #[test]
    fn build_update_headers_sets_expected_revision_and_integer_seconds_ttl() {
        let headers = build_update_headers(7, Duration::from_secs(9));
        assert_eq!(
            headers
                .get(NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .map(|value| value.as_str()),
            Some("7")
        );
        assert_eq!(headers.get(NATS_MESSAGE_TTL).map(|value| value.as_str()), Some("9"));
    }

    #[test]
    fn lease_config_error_display_messages_are_actionable() {
        let empty_bucket = LeaseConfigError::EmptyBucket;
        assert!(empty_bucket.to_string().contains("must not be empty"));
        let empty_key = LeaseConfigError::EmptyKey;
        assert!(empty_key.to_string().contains("must not be empty"));

        let invalid_bucket = LeaseConfigError::InvalidBucketName("bad.bucket".to_string());
        assert!(invalid_bucket.to_string().contains("must contain only ASCII letters"));

        let invalid_key = LeaseConfigError::InvalidKeyName("bad key".to_string());
        assert!(invalid_key.to_string().contains("must contain only ASCII letters"));

        let invalid_timing = LeaseConfigError::RenewIntervalNotLessThanTtl {
            renew_interval: Duration::from_secs(5),
            ttl: Duration::from_secs(5),
        };
        assert!(invalid_timing.to_string().contains("must be less than ttl"));
    }

    #[test]
    fn ensure_leader_error_exposes_display_and_source() {
        let acquire = EnsureLeaderError::Acquire(kv::CreateError::new(kv::CreateErrorKind::Other));
        assert!(acquire.to_string().contains("failed to acquire lease"));
        assert!(std::error::Error::source(&acquire).is_some());

        let renew = EnsureLeaderError::Renew(kv::UpdateError::new(kv::UpdateErrorKind::Other));
        assert!(renew.to_string().contains("failed to renew lease"));
        assert!(std::error::Error::source(&renew).is_some());
    }

    #[test]
    fn incompatible_bucket_config_error_has_message_and_source() {
        let config = NatsKvLeaseConfig::new("bucket", "key", ttl(10), renew_interval(5)).unwrap();
        let error = validate_bucket_settings(
            KeyValueSettings {
                history: 1,
                max_age: Duration::ZERO,
                allow_message_ttl: true,
                subject_delete_marker_ttl: Some(Duration::from_secs(1)),
            },
            &config,
        )
        .unwrap_err();

        assert!(matches!(error, LeaseError::IncompatibleBucketConfig { .. }));
        assert!(error.to_string().contains("incompatible bucket config"));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn lease_error_source_chain_wraps_create_bucket_error() {
        let error = LeaseError::provision_source(
            "failed to create lease bucket",
            LeaseProvisionError::CreateBucket(CreateKeyValueError::new(CreateKeyValueErrorKind::TimedOut)),
        );

        let provision = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<LeaseProvisionError>())
            .expect("expected LeaseProvisionError source");
        assert!(matches!(provision, LeaseProvisionError::CreateBucket(_)));
        assert!(
            std::error::Error::source(provision)
                .and_then(|source| source.downcast_ref::<CreateKeyValueError>())
                .is_some()
        );
    }

    #[test]
    fn lease_error_source_chain_wraps_open_existing_bucket_error() {
        let error = LeaseError::provision_source(
            "failed to open existing lease bucket after create reported already exists",
            LeaseProvisionError::OpenExistingBucket(KeyValueError::new(
                async_nats::jetstream::context::KeyValueErrorKind::GetBucket,
            )),
        );

        let provision = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<LeaseProvisionError>())
            .expect("expected LeaseProvisionError source");
        assert!(matches!(provision, LeaseProvisionError::OpenExistingBucket(_)));
        assert!(
            std::error::Error::source(provision)
                .and_then(|source| source.downcast_ref::<KeyValueError>())
                .is_some()
        );
    }

    #[test]
    fn lease_error_source_chain_wraps_status_error() {
        let error = LeaseError::provision_source(
            "failed to inspect lease bucket configuration",
            LeaseProvisionError::InspectBucket(kv::StatusError::new(kv::StatusErrorKind::TimedOut)),
        );

        let provision = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<LeaseProvisionError>())
            .expect("expected LeaseProvisionError source");
        assert!(matches!(provision, LeaseProvisionError::InspectBucket(_)));
        assert!(
            std::error::Error::source(provision)
                .and_then(|source| source.downcast_ref::<kv::StatusError>())
                .is_some()
        );
    }

    #[test]
    fn lease_error_display_mentions_context_for_provision_failures() {
        let error = LeaseError::provision_source(
            "failed to create lease bucket",
            LeaseProvisionError::CreateBucket(CreateKeyValueError::new(CreateKeyValueErrorKind::TimedOut)),
        );
        assert!(error.to_string().contains("failed to create lease bucket"));
    }

    #[test]
    fn nats_kv_lease_provision_error_helpers_set_expected_context() {
        let create = create_bucket_error(CreateKeyValueError::new(CreateKeyValueErrorKind::TimedOut));
        assert!(create.to_string().contains("failed to create lease bucket"));

        let open = open_existing_bucket_error(KeyValueError::new(
            async_nats::jetstream::context::KeyValueErrorKind::GetBucket,
        ));
        assert!(open.to_string().contains("failed to open existing lease bucket"));

        let inspect = inspect_bucket_error(kv::StatusError::new(kv::StatusErrorKind::TimedOut));
        assert!(
            inspect
                .to_string()
                .contains("failed to inspect lease bucket configuration")
        );
    }

    #[test]
    fn lease_provision_error_display_covers_all_variants() {
        let create = LeaseProvisionError::CreateBucket(CreateKeyValueError::new(CreateKeyValueErrorKind::TimedOut));
        assert!(!create.to_string().is_empty());

        let open = LeaseProvisionError::OpenExistingBucket(KeyValueError::new(
            async_nats::jetstream::context::KeyValueErrorKind::GetBucket,
        ));
        assert!(!open.to_string().is_empty());

        let inspect = LeaseProvisionError::InspectBucket(kv::StatusError::new(kv::StatusErrorKind::TimedOut));
        assert!(!inspect.to_string().is_empty());
    }
}
