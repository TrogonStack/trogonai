use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;
use trogon_workload_identity::IdentityStore;

use crate::audit::AuditSink;
use crate::audit_readiness::AuditReadiness;
use crate::live_policy::KvSnapshot;
use crate::readiness_status::ReadinessStatus;
use crate::runtime_phase::RuntimePhase;

#[derive(Clone)]
pub struct Readiness {
    inner: Arc<ReadinessState>,
}

struct ReadinessState {
    identity: Option<IdentityStore>,
    policy: Option<Arc<KvSnapshot>>,
    audit: Arc<dyn AuditSink>,
    phase: Mutex<RuntimePhase>,
}

impl Readiness {
    pub(crate) fn new(
        identity: Option<IdentityStore>,
        policy: Option<Arc<KvSnapshot>>,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        Self {
            inner: Arc::new(ReadinessState {
                identity,
                policy,
                audit,
                phase: Mutex::new(RuntimePhase::Starting),
            }),
        }
    }

    pub(crate) fn set_running(&self, shutdown: CancellationToken) {
        if let Ok(mut phase) = self.inner.phase.lock() {
            *phase = RuntimePhase::Running(shutdown);
        }
    }

    pub(crate) fn stopped(&self) {
        if let Ok(mut phase) = self.inner.phase.lock() {
            *phase = RuntimePhase::Stopped;
        }
    }

    /// Checks current authority before and after a fresh durable audit probe.
    pub async fn probe(&self) -> ReadinessStatus {
        if let Err(status) = self.current() {
            return status;
        }
        match self.inner.audit.readiness().await {
            AuditReadiness::Ready => self.current().map_or_else(|status| status, |()| ReadinessStatus::Ready),
            AuditReadiness::Unavailable => ReadinessStatus::AuditUnavailable,
            AuditReadiness::Degraded => ReadinessStatus::AuditDegraded,
            AuditReadiness::Overloaded => ReadinessStatus::AuditOverloaded,
            AuditReadiness::TimedOut => ReadinessStatus::AuditTimedOut,
        }
    }

    fn current(&self) -> Result<(), ReadinessStatus> {
        match self.inner.phase.lock().as_deref() {
            Ok(RuntimePhase::Running(shutdown)) if !shutdown.is_cancelled() => {}
            Ok(RuntimePhase::Starting) => return Err(ReadinessStatus::Starting),
            _ => return Err(ReadinessStatus::Stopped),
        }
        let identity = self
            .inner
            .identity
            .as_ref()
            .ok_or(ReadinessStatus::UnsupportedProfile)?;
        if identity.material().is_none() {
            return Err(ReadinessStatus::IdentityUnavailable);
        }
        if self
            .inner
            .policy
            .as_ref()
            .and_then(|policy| policy.statistics())
            .is_none()
        {
            return Err(ReadinessStatus::PolicyUnavailable);
        }
        Ok(())
    }
}
