use crate::{error::PlanError, owner::PlanSourceLease};
use trogon_session_store::SessionAdmissionLease;

pub struct PlanAdmissionLease<S, P> {
    inner: LeaseKind<S, P>,
}

enum LeaseKind<S, P> {
    Source(S),
    Plan(P),
    Delegation { parent: P, child: P, policy: S },
}

impl<S, P> PlanAdmissionLease<S, P> {
    pub(crate) fn source(source: S) -> Self {
        Self {
            inner: LeaseKind::Source(source),
        }
    }
    pub(crate) fn plan(plan: P) -> Self {
        Self {
            inner: LeaseKind::Plan(plan),
        }
    }
    pub(crate) fn delegation(parent: P, child: P, policy: S) -> Self {
        Self {
            inner: LeaseKind::Delegation { parent, child, policy },
        }
    }
}
impl<S: SessionAdmissionLease, P: PlanSourceLease> SessionAdmissionLease for PlanAdmissionLease<S, P> {
    type Error = PlanError;
    fn require_live(&self) -> Result<(), Self::Error> {
        match &self.inner {
            LeaseKind::Source(source) => source.require_live().map_err(|_| PlanError::AuthorityWithdrawn),
            LeaseKind::Plan(plan) => plan.require_live().map_err(|_| PlanError::AuthorityWithdrawn),
            LeaseKind::Delegation { parent, child, policy } => {
                parent.require_live().map_err(|_| PlanError::AuthorityWithdrawn)?;
                child.require_live().map_err(|_| PlanError::AuthorityWithdrawn)?;
                policy.require_live().map_err(|_| PlanError::AuthorityWithdrawn)
            }
        }
    }
    fn deadline(&self) -> std::time::Instant {
        match &self.inner {
            LeaseKind::Source(source) => source.deadline(),
            LeaseKind::Plan(plan) => plan.deadline(),
            LeaseKind::Delegation { parent, child, policy } => {
                parent.deadline().min(child.deadline()).min(policy.deadline())
            }
        }
    }
    async fn withdrawn(&self) {
        match &self.inner {
            LeaseKind::Source(source) => source.withdrawn().await,
            LeaseKind::Plan(plan) => plan.withdrawn().await,
            LeaseKind::Delegation { parent, child, policy } => {
                tokio::select! {()=parent.withdrawn()=>{},()=child.withdrawn()=>{},()=policy.withdrawn()=>{}}
            }
        }
    }
}
