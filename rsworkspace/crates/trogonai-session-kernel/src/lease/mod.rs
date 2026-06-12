mod guard;
mod operation;
mod session_kv_lease;

pub use guard::SessionLeaseGuard;
pub use operation::SessionMutatingOperation;
pub use session_kv_lease::SessionKvLease;


use async_nats::jetstream::kv;
use bytes::Bytes;
use trogon_nats::lease::{
    LeaseKey, LeaseRenewInterval, LeaseTtl, NatsKvLeaseConfig, ReleaseLease, RenewLease, TryAcquireLease,
};
use trogonai_session_contracts::SessionId;

use crate::config::SessionKernelConfig;
use crate::error::SessionKernelError;
use crate::nats::session_lease_key;
use crate::state::LeaseState;

/// Session-scoped lease policy built on `trogon_nats::lease`.
#[derive(Clone)]
pub struct SessionLeaseManager<L> {
    lease_factory: L,
    holder_id: String,
}

impl<L> SessionLeaseManager<L>
where
    L: SessionLeaseFactory + Clone + Send + Sync + 'static,
{
    pub fn new(lease_factory: L, holder_id: impl Into<String>) -> Self {
        Self {
            lease_factory,
            holder_id: holder_id.into(),
        }
    }

    pub fn holder_id(&self) -> &str {
        &self.holder_id
    }

    pub async fn acquire_session_lease(
        &self,
        session_id: &SessionId,
        operation: SessionMutatingOperation,
    ) -> Result<SessionLeaseGuard<<L as SessionLeaseFactory>::Lease>, SessionKernelError> {
        let lease = self.lease_factory.for_session(session_id)?;
        let holder = Bytes::from(self.holder_id.clone());

        match lease.try_acquire(holder.clone()).await {
            Ok(revision) => Ok(SessionLeaseGuard::new(
                session_id.clone(),
                lease,
                revision,
                operation,
                self.holder_id.clone(),
                LeaseState::Acquired,
            )),
            Err(source) if is_lease_held(&source) => Err(SessionKernelError::session_busy(session_id.clone())),
            Err(source) => Err(SessionKernelError::LeaseAcquire {
                session_id: session_id.clone(),
                detail: source.to_string(),
            }),
        }
    }

    pub async fn renew_session_lease(
        &self,
        guard: &mut SessionLeaseGuard<<L as SessionLeaseFactory>::Lease>,
    ) -> Result<(), SessionKernelError> {
        let holder = Bytes::from(guard.holder_id().to_string());
        match guard.lease().renew(holder, guard.revision()).await {
            Ok(revision) => {
                guard.set_revision(revision);
                guard.set_state(LeaseState::Renewing);
                Ok(())
            }
            Err(source) => Err(SessionKernelError::LeaseRenew {
                session_id: guard.session_id().clone(),
                detail: source.to_string(),
            }),
        }
    }

    pub async fn release_session_lease(
        &self,
        guard: SessionLeaseGuard<<L as SessionLeaseFactory>::Lease>,
    ) -> Result<(), SessionKernelError> {
        let session_id = guard.session_id().clone();
        match guard.lease().release(guard.revision()).await {
            Ok(()) => Ok(()),
            Err(source) => Err(SessionKernelError::LeaseRelease {
                session_id,
                detail: source.to_string(),
            }),
        }
    }
}

pub trait SessionLeaseFactory: Send + Sync {
    type Lease: TryAcquireLease<Error = kv::CreateError>
        + RenewLease<Error = kv::UpdateError>
        + ReleaseLease<Error = kv::DeleteError>
        + Clone
        + Send
        + Sync
        + 'static;

    fn for_session(&self, session_id: &SessionId) -> Result<Self::Lease, SessionKernelError>;
}

pub fn lease_bucket_config(config: &SessionKernelConfig) -> Result<NatsKvLeaseConfig, SessionKernelError> {
    let ttl = LeaseTtl::from_secs(config.lease_ttl().as_secs()).map_err(SessionKernelError::LeaseTtl)?;
    let renew = LeaseRenewInterval::from_secs(config.lease_renew_interval().as_secs())
        .map_err(SessionKernelError::LeaseRenewInterval)?;
    NatsKvLeaseConfig::new(
        crate::nats::session_leases_bucket(&config.nats_prefix),
        "sessions.placeholder.lock",
        ttl,
        renew,
    )
    .map_err(SessionKernelError::LeaseConfig)
}

pub(crate) fn lease_key_for_session(session_id: &SessionId) -> Result<LeaseKey, SessionKernelError> {
    LeaseKey::new(session_lease_key(session_id)).map_err(SessionKernelError::LeaseConfig)
}

fn is_lease_held(source: &kv::CreateError) -> bool {
    matches!(source.kind(), kv::CreateErrorKind::AlreadyExists)
}

#[cfg(any(test, feature = "test-support"))]
mod mock {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    pub struct MockSessionLease {
        inner: Arc<MockSessionLeaseInner>,
    }

    #[derive(Default)]
    struct MockSessionLeaseInner {
        acquire_behavior: Mutex<AcquireBehavior>,
        renew_error: Mutex<bool>,
        release_behavior: Mutex<ReleaseBehavior>,
        next_revision: AtomicU64,
        released_revisions: Mutex<Vec<u64>>,
    }

    #[derive(Clone, Copy, Default)]
    enum AcquireBehavior {
        #[default]
        Acquired,
        HeldByOther,
        #[allow(dead_code)]
        Error,
    }

    #[derive(Clone, Copy, Default)]
    enum ReleaseBehavior {
        #[default]
        Ok,
        #[allow(dead_code)]
        WrongLastRevision,
        #[allow(dead_code)]
        Error,
    }

    impl MockSessionLease {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn hold_by_other(&self) {
            *self.inner.acquire_behavior.lock().unwrap() = AcquireBehavior::HeldByOther;
        }

        pub fn released_revisions(&self) -> Vec<u64> {
            self.inner.released_revisions.lock().unwrap().clone()
        }
    }

    #[derive(Clone)]
    pub struct MockSessionLeaseFactory {
        lease: MockSessionLease,
    }

    impl MockSessionLeaseFactory {
        pub fn new(lease: MockSessionLease) -> Self {
            Self { lease }
        }
    }

    impl SessionLeaseFactory for MockSessionLeaseFactory {
        type Lease = MockSessionLease;

        fn for_session(&self, _session_id: &SessionId) -> Result<Self::Lease, SessionKernelError> {
            Ok(self.lease.clone())
        }
    }

    impl TryAcquireLease for MockSessionLease {
        type Error = kv::CreateError;

        async fn try_acquire(&self, _value: Bytes) -> Result<u64, Self::Error> {
            match *self.inner.acquire_behavior.lock().unwrap() {
                AcquireBehavior::Acquired => {
                    Ok(self.inner.next_revision.fetch_add(1, Ordering::SeqCst))
                }
                AcquireBehavior::HeldByOther => {
                    Err(kv::CreateError::new(kv::CreateErrorKind::AlreadyExists))
                }
                AcquireBehavior::Error => Err(kv::CreateError::new(kv::CreateErrorKind::Other)),
            }
        }
    }

    impl RenewLease for MockSessionLease {
        type Error = kv::UpdateError;

        async fn renew(&self, _value: Bytes, revision: u64) -> Result<u64, Self::Error> {
            if *self.inner.renew_error.lock().unwrap() {
                Err(kv::UpdateError::new(kv::UpdateErrorKind::Other))
            } else {
                Ok(revision + 1)
            }
        }
    }

    impl ReleaseLease for MockSessionLease {
        type Error = kv::DeleteError;

        async fn release(&self, revision: u64) -> Result<(), Self::Error> {
            self.inner.released_revisions.lock().unwrap().push(revision);
            match *self.inner.release_behavior.lock().unwrap() {
                ReleaseBehavior::Ok => Ok(()),
                ReleaseBehavior::WrongLastRevision => {
                    Err(kv::DeleteError::new(kv::DeleteErrorKind::WrongLastRevision))
                }
                ReleaseBehavior::Error => Err(kv::DeleteError::new(kv::DeleteErrorKind::Other)),
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub use mock::{MockSessionLease, MockSessionLeaseFactory};
