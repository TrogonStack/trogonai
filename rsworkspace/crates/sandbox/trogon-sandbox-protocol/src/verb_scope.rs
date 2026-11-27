use trogon_sandbox_contract::{DispatchAttemptId, LeaseEpoch, LeaseId, TenantId, WorkerId};

/// What a signed per-lease request is bound to, besides its subject.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VerbScope {
    /// The tenant that owns the lease.
    pub tenant: TenantId,
    /// The lease the request names; empty for a request that has no lease.
    pub lease_id: LeaseId,
    /// The worker the request is addressed to, when it names one.
    pub worker_id: Option<WorkerId>,
    /// The attempt the request is for, when it names one.
    pub attempt_id: Option<DispatchAttemptId>,
    /// The lease epoch, signed only when present.
    pub epoch: Option<LeaseEpoch>,
}
