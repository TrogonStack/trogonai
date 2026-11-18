use std::{error::Error, future::Future, time::Instant};

use trogon_decider_runtime::{EventId, StreamPosition};
use trogon_identity_types::SubjectPrincipal;

use crate::{SessionKey, wire};

pub struct CreateAdmission<'a> {
    pub caller: &'a SubjectPrincipal,
    pub key: &'a SessionKey,
    pub command: &'a wire::CreateSession,
    pub original_command: &'a [u8],
}

pub struct DispatchAdmission<'a> {
    pub caller: &'a SubjectPrincipal,
    pub key: &'a SessionKey,
    pub parent: &'a wire::SessionStarted,
    pub original_parent: &'a [u8],
    pub command: &'a wire::DispatchDelegation,
    pub original_command: &'a [u8],
    pub attempt: Option<AttemptSource<'a>>,
}

pub struct ChildAdmission<'a> {
    pub caller: &'a SubjectPrincipal,
    pub parent_key: &'a SessionKey,
    pub child_key: &'a SessionKey,
    pub parent: &'a wire::SessionStarted,
    pub original_parent: &'a [u8],
    pub dispatch: &'a wire::DelegationDispatched,
    pub original_dispatch: &'a [u8],
    pub creation: &'a wire::CreateChildSession,
    pub dispatch_event_id: EventId,
    pub dispatch_ordinal: u64,
    pub parent_position: StreamPosition,
    pub attempt: Option<AttemptSource<'a>>,
}

pub struct AttemptSource<'a> {
    pub started: &'a wire::ExecutionAttemptStarted,
    pub original_started: &'a [u8],
    pub ready: Option<&'a wire::ExecutionAttemptReady>,
    pub original_ready: Option<&'a [u8]>,
}

/// Real resource owners supply current source, plan, lifecycle and caller authority.
/// A decoded plan or claimed Ready reference cannot replace those retained authorities.
pub trait SessionAdmission: Send + Sync {
    type Error: Error + Send + Sync + 'static;
    type Lease: SessionAdmissionLease;

    fn authorize_source(
        &self,
        caller: &SubjectPrincipal,
        key: &SessionKey,
    ) -> impl Future<Output = Result<Self::Lease, Self::Error>> + Send;

    fn authorize_create(
        &self,
        material: CreateAdmission<'_>,
    ) -> impl Future<Output = Result<Self::Lease, Self::Error>> + Send;

    fn authorize_dispatch(
        &self,
        material: DispatchAdmission<'_>,
    ) -> impl Future<Output = Result<Self::Lease, Self::Error>> + Send;

    fn authorize_child(
        &self,
        material: ChildAdmission<'_>,
    ) -> impl Future<Output = Result<Self::Lease, Self::Error>> + Send;
}

pub trait SessionAdmissionLease: Send + Sync {
    type Error: Error + Send + Sync + 'static;

    fn require_live(&self) -> Result<(), Self::Error>;

    fn deadline(&self) -> Instant;

    fn withdrawn(&self) -> impl Future<Output = ()> + Send;
}
