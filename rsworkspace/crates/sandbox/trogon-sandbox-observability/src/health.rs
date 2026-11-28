//! Process health for the operational paths `/-/liveness` and `/-/readiness`.
//!
//! One [`Health`] handle is shared by everything in a process. The process moves through
//! three phases: starting, running and shutting down. Liveness answers only whether the
//! process finished its own initialization, and never looks at a dependency, so a broken
//! dependency cannot restart every pod at once. Readiness additionally reads the state the
//! process already holds about the services every request needs, and fails as soon as
//! shutdown begins.

mod dependency;
mod phase;
mod router;
mod status;

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub use dependency::{Dependency, DependencyState, NatsDependency};
use phase::Phase;
pub use status::{Liveness, NotReady, Readiness};

use crate::constants::DEPENDENCY_GRACE;

struct Tracked {
    dependency: Box<dyn Dependency>,
    down_since: Mutex<Option<Instant>>,
}

struct Inner {
    phase: AtomicU8,
    grace: Duration,
    dependencies: Mutex<Vec<Arc<Tracked>>>,
}

/// Shared health state of one process.
#[derive(Clone)]
pub struct Health {
    inner: Arc<Inner>,
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Health {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Health").field("phase", &self.phase()).finish()
    }
}

impl Health {
    /// A process that has not finished initializing and requires no dependency yet.
    pub fn new() -> Self {
        Self::with_grace(DEPENDENCY_GRACE)
    }

    /// Like [`Health::new`], with a custom grace period for down dependencies.
    pub fn with_grace(grace: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                phase: AtomicU8::new(Phase::Starting.into()),
                grace,
                dependencies: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Require `dependency` for readiness. Liveness never reads it.
    pub fn require(&self, dependency: impl Dependency + 'static) {
        let tracked = Arc::new(Tracked {
            dependency: Box::new(dependency),
            down_since: Mutex::new(None),
        });
        self.inner
            .dependencies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tracked);
    }

    /// Require the NATS connection for readiness.
    pub fn require_nats(&self, client: &async_nats::Client) {
        self.require(NatsDependency::new(client.clone()));
    }

    /// Require the NATS connection and mark the process started: the usual end of a NATS worker's setup.
    pub fn started_with_nats(&self, client: &async_nats::Client) {
        self.require_nats(client);
        self.mark_started();
    }

    /// Begin shutdown once `trigger` completes, for example a cancellation token being cancelled.
    ///
    /// Must be called from within a Tokio runtime.
    pub fn shutdown_when(&self, trigger: impl Future<Output = ()> + Send + 'static) {
        let health = self.clone();
        tokio::spawn(async move {
            trigger.await;
            health.begin_shutdown();
        });
    }

    /// Mark the process's own initialization as finished. Does nothing once shutdown began.
    pub fn mark_started(&self) {
        let _ = self.inner.phase.compare_exchange(
            Phase::Starting.into(),
            Phase::Running.into(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Fail readiness from now on, while liveness keeps passing until the process exits.
    pub fn begin_shutdown(&self) {
        self.inner.phase.store(Phase::ShuttingDown.into(), Ordering::Release);
    }

    fn phase(&self) -> Phase {
        Phase::from(self.inner.phase.load(Ordering::Acquire))
    }

    /// Whether the process can make progress.
    pub fn liveness(&self) -> Liveness {
        match self.phase() {
            Phase::Starting => Liveness::Starting,
            Phase::Running | Phase::ShuttingDown => Liveness::Alive,
        }
    }

    /// Whether requests should reach the process.
    pub fn readiness(&self) -> Readiness {
        self.readiness_at(Instant::now())
    }

    fn readiness_at(&self, now: Instant) -> Readiness {
        match self.phase() {
            Phase::Starting => return Readiness::NotReady(NotReady::Starting),
            Phase::ShuttingDown => return Readiness::NotReady(NotReady::ShuttingDown),
            Phase::Running => {}
        }
        let dependencies = self
            .inner
            .dependencies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for tracked in dependencies {
            if tracked.expired(now, self.inner.grace) {
                return Readiness::NotReady(NotReady::DependencyDown(tracked.dependency.name()));
            }
        }
        Readiness::Ready
    }
}

impl Tracked {
    /// Records the dependency's state and reports whether it has been down past `grace`.
    fn expired(&self, now: Instant, grace: Duration) -> bool {
        let mut down_since = self.down_since.lock().unwrap_or_else(PoisonError::into_inner);
        match self.dependency.state() {
            DependencyState::Up => {
                *down_since = None;
                false
            }
            DependencyState::Down => {
                let since = *down_since.get_or_insert(now);
                now.saturating_duration_since(since) >= grace
            }
        }
    }
}

#[cfg(test)]
mod tests;
