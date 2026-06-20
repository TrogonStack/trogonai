mod guard;
mod operation;
mod session_kv_lease;

pub use guard::SessionLeaseGuard;
pub use operation::SessionMutatingOperation;
pub use session_kv_lease::SessionKvLease;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

    /// Release a lease that was acquired with a renewal heartbeat: stop the
    /// heartbeat cleanly, then delete the lease at its latest committed revision
    /// (Session Lease flow: *acquire -> mutation -> append -> materialize ->
    /// release*). The heartbeat advances the revision out-of-band, so the guard's
    /// revision is refreshed before the delete to avoid a `WrongLastRevision`.
    pub async fn release_session_lease_renewing(
        &self,
        mut guard: SessionLeaseGuard<<L as SessionLeaseFactory>::Lease>,
        renewal: LeaseRenewal,
    ) -> Result<(), SessionKernelError> {
        let final_revision = renewal.stop_and_revision().await;
        guard.set_revision(final_revision);
        self.release_session_lease(guard).await
    }

    /// Start a background heartbeat that renews `lease` every `renew_interval` so a
    /// long operation never lets the lease expire mid-flight (Session Lease:
    /// *"heartbeat/renew mientras dura la operacion"*). The returned [`LeaseRenewal`]
    /// tracks the latest committed lease revision (each renew advances it) so the
    /// operation can release explicitly at the right revision. Dropping the handle
    /// aborts the heartbeat; the lease then expires by TTL if the process dies
    /// (*"expiracion automatica si el proceso muere"*). Renewal stops on the first
    /// failure (the lease was lost), letting another holder take over.
    pub fn start_renewal(
        &self,
        lease: <L as SessionLeaseFactory>::Lease,
        initial_revision: u64,
        renew_interval: std::time::Duration,
    ) -> LeaseRenewal {
        let holder = self.holder_id.clone();
        let revision = Arc::new(AtomicU64::new(initial_revision));
        let cancel = Arc::new(tokio::sync::Notify::new());
        let task_revision = revision.clone();
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            let mut current = initial_revision;
            let mut ticker = tokio::time::interval(renew_interval);
            // The first tick fires immediately; skip it so the first renew happens
            // after one interval, not at acquire time.
            ticker.tick().await;
            loop {
                tokio::select! {
                    // Prefer a pending stop over firing another renew.
                    biased;
                    _ = task_cancel.notified() => break,
                    _ = ticker.tick() => {
                        match lease.renew(Bytes::from(holder.clone()), current).await {
                            Ok(next) => {
                                current = next;
                                task_revision.store(next, Ordering::SeqCst);
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
        LeaseRenewal {
            handle: Some(handle),
            cancel,
            revision,
        }
    }
}

/// RAII handle for a background lease-renewal heartbeat. While held, the lease is
/// renewed every `renew_interval` and the handle tracks the latest committed
/// revision. Dropping it aborts the heartbeat (the lease then expires by TTL); to
/// release the lease explicitly, hand it to
/// [`SessionLeaseManager::release_session_lease_renewing`].
pub struct LeaseRenewal {
    handle: Option<tokio::task::JoinHandle<()>>,
    cancel: Arc<tokio::sync::Notify>,
    revision: Arc<AtomicU64>,
}

impl LeaseRenewal {
    /// The latest committed lease revision (advances as the heartbeat renews).
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }

    /// Stop the heartbeat cleanly (waiting for the task to finish so no renew races
    /// a subsequent release) and return the latest committed revision.
    pub async fn stop_and_revision(mut self) -> u64 {
        self.cancel.notify_one();
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
        self.revision.load(Ordering::SeqCst)
    }

    /// Abandon the heartbeat without releasing the lease (it expires by TTL).
    pub fn stop(self) {
        // Drop aborts the task.
    }
}

impl Drop for LeaseRenewal {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
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
        renew_count: AtomicU64,
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

        pub fn renew_count(&self) -> u64 {
            self.inner.renew_count.load(Ordering::SeqCst)
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
                AcquireBehavior::Acquired => Ok(self.inner.next_revision.fetch_add(1, Ordering::SeqCst)),
                AcquireBehavior::HeldByOther => Err(kv::CreateError::new(kv::CreateErrorKind::AlreadyExists)),
                AcquireBehavior::Error => Err(kv::CreateError::new(kv::CreateErrorKind::Other)),
            }
        }
    }

    impl RenewLease for MockSessionLease {
        type Error = kv::UpdateError;

        async fn renew(&self, _value: Bytes, revision: u64) -> Result<u64, Self::Error> {
            self.inner.renew_count.fetch_add(1, Ordering::SeqCst);
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
                ReleaseBehavior::WrongLastRevision => Err(kv::DeleteError::new(kv::DeleteErrorKind::WrongLastRevision)),
                ReleaseBehavior::Error => Err(kv::DeleteError::new(kv::DeleteErrorKind::Other)),
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub use mock::{MockSessionLease, MockSessionLeaseFactory};

#[cfg(test)]
mod renewal_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn heartbeat_renews_during_operation_and_stops_when_dropped() {
        let lease = MockSessionLease::new();
        let manager = SessionLeaseManager::new(MockSessionLeaseFactory::new(lease.clone()), "node-test");
        let guard = manager
            .acquire_session_lease(
                &SessionId::new("sess_renew").unwrap(),
                SessionMutatingOperation::SwitchModel,
            )
            .await
            .unwrap();
        assert_eq!(lease.renew_count(), 0, "no renew before the heartbeat starts");

        // Short interval so the test runs fast; the operation outlives several ticks.
        let renewal = manager.start_renewal(guard.lease().clone(), guard.revision(), Duration::from_millis(5));
        tokio::time::sleep(Duration::from_millis(40)).await;
        let during = lease.renew_count();
        assert!(
            during >= 1,
            "the lease must be renewed during the operation, got {during}"
        );

        // Dropping the handle stops the heartbeat -> no further renewals.
        drop(renewal);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let after_drop = lease.renew_count();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            lease.renew_count(),
            after_drop,
            "renewal must stop once the LeaseRenewal handle is dropped"
        );
    }

    #[tokio::test]
    async fn explicit_release_uses_the_latest_renewed_revision() {
        let lease = MockSessionLease::new();
        let manager = SessionLeaseManager::new(MockSessionLeaseFactory::new(lease.clone()), "node-test");
        let session_id = SessionId::new("sess_release").unwrap();
        let guard = manager
            .acquire_session_lease(&session_id, SessionMutatingOperation::PromptTurn)
            .await
            .unwrap();
        let initial_revision = guard.revision();

        // Heartbeat advances the revision (mock renew returns revision + 1) while the
        // operation runs.
        let renewal = manager.start_renewal(guard.lease().clone(), guard.revision(), Duration::from_millis(5));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            lease.renew_count() >= 1,
            "the heartbeat must have renewed at least once"
        );

        // Explicit release must delete at the ADVANCED revision, not the stale one.
        manager.release_session_lease_renewing(guard, renewal).await.unwrap();
        let released = lease.released_revisions();
        assert_eq!(released.len(), 1, "the lease must be released exactly once");
        assert!(
            released[0] > initial_revision,
            "release must use the renewed revision {} > initial {initial_revision}",
            released[0]
        );
    }
}

#[cfg(test)]
mod expiry_tests {
    use super::*;
    use trogon_nats::jetstream::MockJetStreamKvStore;

    use crate::SessionKvLeaseFactory;

    /// § Fase 2 "tests requeridos: lease acquire/renew/**expire**" / PR 3 "tests de
    /// contention y **expiry**". A held lease blocks a concurrent holder; once the
    /// lease's KV key TTL elapses the key is auto-deleted, so a new holder can acquire
    /// WITHOUT any explicit release. This exercises the REAL `SessionKvLease`
    /// (`try_acquire` = `create_with_ttl`) over a KV mock that simulates the key's TTL
    /// lifecycle (held -> expired). The distinguishing assertion from a plain release
    /// test: no `delete` is ever issued — the lease is reclaimed purely by TTL expiry.
    #[tokio::test]
    async fn lease_expires_by_ttl_and_is_reacquirable_without_release() {
        let kv = MockJetStreamKvStore::new();
        let config = SessionKernelConfig::default();
        let manager = SessionLeaseManager::new(SessionKvLeaseFactory::new(kv.clone(), &config), "node-a");
        let session_id = SessionId::new("sess_expire").unwrap();

        // Holder A acquires: the lease key is created bound to the configured TTL,
        // which is the mechanism that makes it auto-expire when the holder dies.
        kv.set_create_with_ttl_result(Ok(1));
        let guard_a = manager
            .acquire_session_lease(&session_id, SessionMutatingOperation::PromptTurn)
            .await
            .expect("first holder acquires the free lease");
        let ttl_calls = kv.create_with_ttl_calls();
        assert_eq!(ttl_calls.len(), 1, "acquire must create the lease key exactly once");
        assert_eq!(
            ttl_calls[0].2,
            config.lease_ttl(),
            "the lease key must be created with the TTL that drives expiry"
        );

        // While A still holds the key, a second holder is blocked (key already exists
        // -> AlreadyExists -> SessionBusy). This is contention, not expiry.
        kv.set_create_with_ttl_result(Err(kv::CreateErrorKind::AlreadyExists));
        let busy = manager
            .acquire_session_lease(&session_id, SessionMutatingOperation::PromptTurn)
            .await;
        assert!(
            matches!(busy, Err(SessionKernelError::SessionBusy { .. })),
            "a held lease must block a concurrent holder"
        );

        // A's process dies WITHOUT releasing. The KV key's TTL elapses -> the key is
        // auto-deleted -> a new holder acquires. This is EXPIRY, not release.
        kv.set_create_with_ttl_result(Ok(2));
        let _guard_b = manager
            .acquire_session_lease(&session_id, SessionMutatingOperation::PromptTurn)
            .await
            .expect("after the lease TTL expires, a new holder acquires without an explicit release");

        // The lease was reclaimed purely by TTL expiry: A never released, so NO
        // delete/release was ever issued against the KV key (distinguishes expiry from
        // the explicit-release path).
        assert!(
            kv.delete_calls().is_empty(),
            "expiry must reclaim the lease via TTL, never via an explicit release/delete"
        );

        // guard_a is intentionally never released — it was reclaimed by TTL.
        drop(guard_a);
    }
}
