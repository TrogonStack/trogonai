use crate::{
    decoder::{DecodedPlan, RegisteredPlanDecoder},
    error::PlanError,
    lease::PlanAdmissionLease,
    owner::{DelegationOwnerContext, DelegationSource, PlanOwnerContext, PlanOwners},
};
use std::{future::Future, sync::Arc};
use trogon_identity_types::SubjectPrincipal;
use trogon_session_store::{
    ChildAdmission, CreateAdmission, DispatchAdmission, SessionAdmission, SessionAdmissionLease, SessionKey,
};

pub struct PlanAdmission<O> {
    decoder: RegisteredPlanDecoder,
    owners: Arc<O>,
}

impl<O> PlanAdmission<O> {
    pub fn new(decoder: RegisteredPlanDecoder, owners: Arc<O>) -> Self {
        Self { decoder, owners }
    }
}

impl<O: PlanOwners> PlanAdmission<O> {
    async fn admit(
        &self,
        caller: &SubjectPrincipal,
        key: &SessionKey,
        plan: &DecodedPlan,
    ) -> Result<O::PlanLease, PlanError> {
        let source = self.owners.admit_plan(PlanOwnerContext { caller, key, plan }).await?;
        crate::source::verify(&self.decoder, plan, &source)?;
        Ok(source)
    }
}

impl<O: PlanOwners> SessionAdmission for PlanAdmission<O> {
    type Error = PlanError;
    type Lease = PlanAdmissionLease<O::SourceLease, O::PlanLease>;

    async fn authorize_source(&self, caller: &SubjectPrincipal, key: &SessionKey) -> Result<Self::Lease, Self::Error> {
        let lease = self.owners.authorize_source(caller, key).await?;
        crate::source::live(&lease)?;
        Ok(PlanAdmissionLease::source(lease))
    }

    async fn authorize_create(&self, material: CreateAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        self.decoder
            .require_original(material.original_command, material.command)?;
        let plan = self.decoder.decode(
            material
                .command
                .execution_plan
                .as_option()
                .ok_or(PlanError::InvalidPlan)?,
        )?;
        if material.command.session_id != material.key.session_id() {
            return Err(PlanError::OriginalSourceMismatch);
        }
        crate::source::projection(
            &plan,
            material.key,
            material
                .command
                .checkout_ref
                .as_option()
                .ok_or(PlanError::InvalidPlan)?,
            material.command.candidate_ref.as_option(),
        )?;
        let source = self.admit(material.caller, material.key, &plan).await?;
        Ok(PlanAdmissionLease::plan(source))
    }

    async fn authorize_dispatch(&self, material: DispatchAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        self.decoder
            .require_original(material.original_command, material.command)?;
        self.decoder
            .require_original(material.original_parent, material.parent)?;
        let parent = self.decoder.decode(
            material
                .parent
                .execution_plan
                .as_option()
                .ok_or(PlanError::InvalidPlan)?,
        )?;
        let child_creation = material
            .command
            .child_creation
            .as_option()
            .ok_or(PlanError::InvalidPlan)?;
        let child = self.decoder.decode(
            child_creation
                .execution_plan
                .as_option()
                .ok_or(PlanError::InvalidPlan)?,
        )?;
        let child_key = SessionKey::new(
            material.key.namespace().clone(),
            material.command.child_session_id.clone(),
        )
        .map_err(|_| PlanError::InvalidPlan)?;
        crate::shape::bounded(&material.command.operation_id, 128)?;
        crate::shape::bounded(&material.command.turn_id, 128)?;
        if matches!(
            material.command.cascade_policy.as_known(),
            None | Some(crate::wire::session::CascadePolicy::Unspecified)
        ) {
            return Err(PlanError::InvalidPlan);
        }
        if material.parent.session_id != material.key.session_id()
            || material.command.session_id != material.key.session_id()
            || child_key == *material.key
        {
            return Err(PlanError::OriginalSourceMismatch);
        }
        crate::source::projection(
            &parent,
            material.key,
            material.parent.checkout_ref.as_option().ok_or(PlanError::InvalidPlan)?,
            material.parent.candidate_ref.as_option(),
        )?;
        crate::source::projection(
            &child,
            &child_key,
            child_creation.checkout_ref.as_option().ok_or(PlanError::InvalidPlan)?,
            child_creation.candidate_ref.as_option(),
        )?;
        let parent_source = self.admit(material.caller, material.key, &parent).await?;
        let child_source = guard(&parent_source, self.admit(material.caller, &child_key, &child)).await?;
        let binding = crate::source::selected_delegate(
            &self.decoder,
            &parent,
            &child,
            &child_source,
            material
                .command
                .delegate_binding
                .as_option()
                .ok_or(PlanError::InvalidPlan)?,
        )?;
        let policy = guard(
            &parent_source,
            guard(
                &child_source,
                self.owners.authorize_delegation(DelegationOwnerContext {
                    caller: material.caller,
                    parent_key: material.key,
                    child_key: &child_key,
                    parent: &parent,
                    child: &child,
                    binding,
                    original_parent: material.original_parent,
                    parent_source: material.parent,
                    attempt: material.attempt,
                    source: DelegationSource::Dispatch {
                        command: material.command,
                        original_command: material.original_command,
                    },
                }),
            ),
        )
        .await?;
        crate::source::live(&policy)?;
        Ok(PlanAdmissionLease::delegation(parent_source, child_source, policy))
    }

    async fn authorize_child(&self, material: ChildAdmission<'_>) -> Result<Self::Lease, Self::Error> {
        self.decoder
            .require_original(material.original_parent, material.parent)?;
        self.decoder
            .require_original(material.original_dispatch, material.dispatch)?;
        crate::shape::bounded(&material.dispatch.operation_id, 128)?;
        crate::shape::bounded(&material.dispatch.turn_id, 128)?;
        if material.dispatch_ordinal == 0
            || matches!(
                material.dispatch.cascade_policy.as_known(),
                None | Some(crate::wire::session::CascadePolicy::Unspecified)
            )
        {
            return Err(PlanError::InvalidPlan);
        }
        let retained = material
            .dispatch
            .child_creation
            .as_option()
            .ok_or(PlanError::InvalidPlan)?;
        if material.parent.session_id != material.parent_key.session_id()
            || material.dispatch.session_id != material.parent_key.session_id()
            || material.dispatch.child_session_id != material.child_key.session_id()
            || material.creation.session_id != material.child_key.session_id()
            || material.creation.parent_session_id != material.parent_key.session_id()
            || material.parent_key.namespace() != material.child_key.namespace()
            || material.parent_key == material.child_key
            || material.creation.execution_plan != retained.execution_plan
            || material.creation.checkout_ref != retained.checkout_ref
            || material.creation.candidate_ref != retained.candidate_ref
            || material.creation.delegate_binding != material.dispatch.delegate_binding
            || material.creation.operation_id != material.dispatch.operation_id
            || material.creation.cascade_policy != material.dispatch.cascade_policy
            || material.creation.parent_dispatched_at.value != material.dispatch_ordinal
        {
            return Err(PlanError::OriginalSourceMismatch);
        }
        let parent = self.decoder.decode(
            material
                .parent
                .execution_plan
                .as_option()
                .ok_or(PlanError::InvalidPlan)?,
        )?;
        let child = self
            .decoder
            .decode(retained.execution_plan.as_option().ok_or(PlanError::InvalidPlan)?)?;
        crate::source::projection(
            &parent,
            material.parent_key,
            material.parent.checkout_ref.as_option().ok_or(PlanError::InvalidPlan)?,
            material.parent.candidate_ref.as_option(),
        )?;
        crate::source::projection(
            &child,
            material.child_key,
            retained.checkout_ref.as_option().ok_or(PlanError::InvalidPlan)?,
            retained.candidate_ref.as_option(),
        )?;
        let parent_source = self.admit(material.caller, material.parent_key, &parent).await?;
        let child_source = guard(&parent_source, self.admit(material.caller, material.child_key, &child)).await?;
        let binding = crate::source::selected_delegate(
            &self.decoder,
            &parent,
            &child,
            &child_source,
            material
                .dispatch
                .delegate_binding
                .as_option()
                .ok_or(PlanError::InvalidPlan)?,
        )?;
        let policy = guard(
            &parent_source,
            guard(
                &child_source,
                self.owners.authorize_delegation(DelegationOwnerContext {
                    caller: material.caller,
                    parent_key: material.parent_key,
                    child_key: material.child_key,
                    parent: &parent,
                    child: &child,
                    binding,
                    original_parent: material.original_parent,
                    parent_source: material.parent,
                    attempt: material.attempt,
                    source: DelegationSource::Recovery {
                        dispatch: material.dispatch,
                        original_dispatch: material.original_dispatch,
                        creation: material.creation,
                        dispatch_event_id: material.dispatch_event_id,
                        dispatch_ordinal: material.dispatch_ordinal,
                        parent_position: material.parent_position,
                    },
                }),
            ),
        )
        .await?;
        crate::source::live(&policy)?;
        Ok(PlanAdmissionLease::delegation(parent_source, child_source, policy))
    }
}

async fn guard<L: SessionAdmissionLease, T>(
    lease: &L,
    future: impl Future<Output = Result<T, PlanError>>,
) -> Result<T, PlanError> {
    crate::source::live(lease)?;
    let result = tokio::select! {
        biased;
        ()=lease.withdrawn()=>return Err(PlanError::AuthorityWithdrawn),
        ()=tokio::time::sleep_until(tokio::time::Instant::from_std(lease.deadline()))=>return Err(PlanError::AuthorityWithdrawn),
        result=future=>result,
    }?;
    crate::source::live(lease)?;
    Ok(result)
}
