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
use crate::jetstream::is_create_key_value_already_exists;
#[cfg(test)]
use async_nats::header::{NATS_EXPECTED_LAST_SUBJECT_SEQUENCE, NATS_MESSAGE_TTL};
use async_nats::jetstream::context::{CreateKeyValueError, KeyValueError};
#[cfg(test)]
use async_nats::jetstream::context::{CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind};
use async_nats::jetstream::kv;
use bytes::Bytes;
#[cfg(test)]
use provision::{
    KeyValueSettings, create_bucket_error, inspect_bucket_error, open_existing_bucket_error, validate_bucket_settings,
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

#[derive(Debug, thiserror::Error)]
pub enum EnsureLeaderError {
    #[error("failed to acquire lease: {0}")]
    Acquire(#[source] kv::CreateError),
    #[error("failed to renew lease: {0}")]
    Renew(#[source] kv::UpdateError),
}

#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("lease provision error: {context}: {source}")]
    Provision {
        context: &'static str,
        #[source]
        source: LeaseProvisionError,
    },
    #[error("lease provision error: incompatible bucket config: {source}")]
    IncompatibleBucketConfig {
        #[source]
        source: IncompatibleLeaseBucketConfigError,
    },
}

impl LeaseError {
    #[cfg_attr(coverage, allow(dead_code))]
    fn provision_source(context: &'static str, source: LeaseProvisionError) -> Self {
        Self::Provision { context, source }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LeaseProvisionError {
    #[error("{0}")]
    CreateBucket(#[source] CreateKeyValueError),
    #[error("{0}")]
    OpenExistingBucket(#[source] KeyValueError),
    #[error("{0}")]
    InspectBucket(#[source] kv::StatusError),
}

#[derive(Debug, thiserror::Error)]
#[error(
    "expected history={expected_history}, max_age={expected_max_age:?}, allow_message_ttl={expected_allow_message_ttl}, subject_delete_marker_ttl={expected_subject_delete_marker_ttl:?}, got history={actual_history}, max_age={actual_max_age:?}, allow_message_ttl={actual_allow_message_ttl}, subject_delete_marker_ttl={actual_subject_delete_marker_ttl:?}"
)]
pub struct IncompatibleLeaseBucketConfigError {
    expected_history: i64,
    actual_history: i64,
    expected_max_age: Duration,
    actual_max_age: Duration,
    expected_allow_message_ttl: bool,
    actual_allow_message_ttl: bool,
    expected_subject_delete_marker_ttl: Option<Duration>,
    actual_subject_delete_marker_ttl: Option<Duration>,
}

#[derive(Clone)]
#[cfg_attr(coverage, allow(dead_code))]
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
mod tests;
