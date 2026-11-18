use std::{future::Future, sync::Arc};

use trogon_identity_types::SubjectPrincipal;
use trogon_session_store::{
    AttemptSource, ChildAdmission, CreateAdmission, DispatchAdmission, SessionAdmission, SessionAdmissionLease, SessionKey,
};

use crate::{
    DecodedPlan, PlanError, RegisteredPlanDecoder,
    wire::{agent, plan, session},
};

pub struct PlanOwnerContext<'a> {
    pub caller: &'a SubjectPrincipal,
    pub key: &'a SessionKey,
    pub plan: &'a DecodedPlan,
}

pub struct DelegationOwnerContext<'a> {
    pub caller: &'a SubjectPrincipal,
    pub parent_key: &'a SessionKey,
    pub child_key: &'a SessionKey,
    pub parent: &'a DecodedPlan,
    pub child: &'a DecodedPlan,
    pub binding: &'a agent::DelegateDeclaration,
    pub original_parent: &'a [u8],
    pub parent_source: &'a session::SessionStarted,
    pub attempt: Option<AttemptSource<'a>>,
    pub source: DelegationSource<'a>,
}

pub enum DelegationSource<'a> {
    Dispatch { command: &'a session::DispatchDelegation, original_command: &'a [u8] },
    Recovery { dispatch: &'a session::DelegationDispatched, original_dispatch: &'a [u8], creation: &'a session::CreateChildSession },
}

pub trait RuntimePlanInterpreter: Send + Sync {
    fn settings_contract(&self) -> &agent::ConfigurationContractRef;
    fn settings_type_url(&self) -> &str;
    fn execution_contract(&self) -> &agent::ConfigurationContractRef;
    fn validate_and_project(
        &self,
        original: &agent::AgentConfiguration,
        plan: &plan::SessionExecutionPlan,
    ) -> Result<agent::NativeConfiguration, PlanError>;
}

/// Implemented at the actual resource owners. Source bytes and registration
/// metadata remain usable only while this original authority remains live.
pub trait PlanSourceLease: SessionAdmissionLease {
    fn revision(&self) -> &agent::AgentRevisionRef;
    fn original_configuration(&self) -> &[u8];
    fn candidate(&self) -> Option<&session::CandidateRef>;
    fn project_id(&self) -> &str;
    fn interpreter(&self) -> &dyn RuntimePlanInterpreter;
}

pub trait PlanOwners: Send + Sync {
    type SourceLease: SessionAdmissionLease;
    type PlanLease: PlanSourceLease;

    fn authorize_source(
        &self,
        caller: &SubjectPrincipal,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Self::SourceLease, PlanError>> + Send;
    /// Loads authenticated original registry/configuration sources and admits
    /// the complete resolution, checkout and capability closure under current
    /// lifecycle and policy. Unsupported owners must refuse the request.
    fn admit_plan(
        &self,
        context: PlanOwnerContext<'_>,
    ) -> impl Future<Output = Result<Self::PlanLease, PlanError>> + Send;
    fn authorize_delegation(
        &self,
        context: DelegationOwnerContext<'_>,
    ) -> impl Future<Output = Result<Self::SourceLease, PlanError>> + Send;
}

pub struct PlanAdmission<O> {
    decoder: RegisteredPlanDecoder,
    owners: Arc<O>,
}

pub struct PlanAdmissionLease<S, P> {
    inner: LeaseKind<S, P>,
}

#[allow(dead_code)]
enum LeaseKind<S, P> {
    Source(S),
    Plan(P),
    Delegation { parent: P, child: P, policy: S },
}

impl<O> PlanAdmission<O> {
    pub fn new(decoder: RegisteredPlanDecoder, owners: Arc<O>) -> Self {
        Self { decoder, owners }
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

impl<O: PlanOwners> SessionAdmission for PlanAdmission<O> {
    type Error = PlanError;
    type Lease = PlanAdmissionLease<O::SourceLease, O::PlanLease>;

    async fn authorize_source(&self, caller: &SubjectPrincipal, key: &SessionKey) -> Result<Self::Lease, Self::Error> {
        let _ = (&self.decoder, &self.owners, caller, key);
        Err(PlanError::UnsupportedRuntime)
    }
    async fn authorize_create(&self, _material: CreateAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Err(PlanError::UnsupportedRuntime)
    }
    async fn authorize_dispatch(&self, _material: DispatchAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Err(PlanError::UnsupportedRuntime)
    }
    async fn authorize_child(&self, _material: ChildAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        Err(PlanError::UnsupportedRuntime)
    }
}
