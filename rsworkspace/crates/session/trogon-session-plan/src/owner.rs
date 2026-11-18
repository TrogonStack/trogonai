use crate::{
    decoder::DecodedPlan,
    error::PlanError,
    wire::{agent, plan, session},
};
use std::future::Future;
use trogon_identity_types::SubjectPrincipal;
use trogon_session_store::{AttemptSource, SessionAdmissionLease, SessionKey};

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
    Dispatch {
        command: &'a session::DispatchDelegation,
        original_command: &'a [u8],
    },
    Recovery {
        dispatch: &'a session::DelegationDispatched,
        original_dispatch: &'a [u8],
        creation: &'a session::CreateChildSession,
        dispatch_event_id: trogon_decider_runtime::EventId,
        dispatch_ordinal: u64,
        parent_position: trogon_decider_runtime::StreamPosition,
    },
}

pub trait RuntimePlanInterpreter: Send + Sync {
    fn implementation(&self) -> &agent::AgentImplementation;
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
    fn original_configuration(&self) -> Result<&[u8], PlanError>;
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
