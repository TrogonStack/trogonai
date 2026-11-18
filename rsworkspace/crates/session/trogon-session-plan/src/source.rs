use crate::{
    decoder::{DecodedPlan, RegisteredPlanDecoder},
    error::PlanError,
    owner::PlanSourceLease,
    shape, variables,
    wire::{agent, plan, session},
};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use trogon_session_store::{SessionAdmissionLease, SessionKey};

pub(crate) fn verify<L: PlanSourceLease>(
    decoder: &RegisteredPlanDecoder,
    decoded: &DecodedPlan,
    source: &L,
) -> Result<(), PlanError> {
    live(source)?;
    let plan = decoded.value();
    if plan.candidate.is_set()
        || plan.work.is_set()
        || !matches!(
            plan.model_access.profile,
            Some(plan::model_access_profile::Profile::NoManagedAccess(_))
        )
        || !matches!(
            plan.compaction.profile,
            Some(plan::compaction_profile::Profile::NoPlatformCompaction(_))
        )
    {
        return Err(PlanError::UnsupportedRuntime);
    }
    if plan.agent_revision.as_option() != Some(source.revision())
        || plan.candidate.as_option() != source.candidate()
        || plan.project_id.value != source.project_id()
    {
        return Err(PlanError::OriginalSourceMismatch);
    }
    let original = source.original_configuration()?;
    if original.is_empty() || original.len() > 65536 {
        return Err(PlanError::Limit);
    }
    let actual: [u8; 32] = Sha256::digest(original).into();
    if plan.effective_configuration.configuration_digest.value.as_slice() != actual {
        return Err(PlanError::OriginalSourceMismatch);
    }
    let configuration = decoder.decode_agent_configuration(original)?;
    let interpreter = source.interpreter();
    if configuration.runtime.as_option() != plan.implementation.as_option()
        || configuration.runtime.as_option() != Some(interpreter.implementation())
        || plan.runtime_settings.contract.as_option() != Some(interpreter.settings_contract())
        || plan.execution_contract.as_option() != Some(interpreter.execution_contract())
    {
        return Err(PlanError::UnsupportedRuntime);
    }
    shape::contract(interpreter.settings_contract())?;
    shape::contract(interpreter.execution_contract())?;
    let original_settings = configuration
        .settings
        .as_option()
        .ok_or(PlanError::OriginalSourceMismatch)?;
    if original_settings.type_url != interpreter.settings_type_url()
        || original_settings.value.as_ref() != plan.runtime_settings.configuration_bytes
    {
        return Err(PlanError::OriginalSourceMismatch);
    }
    declarations(&configuration, plan)?;
    variables::verify(
        configuration.variables_schema.as_option(),
        plan.variables.as_option().ok_or(PlanError::Variables)?,
    )?;
    let expected = interpreter.validate_and_project(&configuration, plan)?;
    shape::native(&expected)?;
    if plan.effective_runtime_configuration.as_option() != Some(&expected) {
        return Err(PlanError::OriginalSourceMismatch);
    }
    live(source)
}

pub(crate) fn projection(
    plan: &DecodedPlan,
    key: &SessionKey,
    checkout: &session::CheckoutRef,
    candidate: Option<&session::CandidateRef>,
) -> Result<(), PlanError> {
    if plan.value().session_id.as_deref() != Some(key.session_id())
        || plan.value().working_directory.checkout.as_option() != Some(checkout)
        || plan.value().candidate.as_option() != candidate
    {
        return Err(PlanError::OriginalSourceMismatch);
    }
    Ok(())
}

pub(crate) fn live<L: SessionAdmissionLease>(lease: &L) -> Result<(), PlanError> {
    lease.require_live().map_err(|_| PlanError::AuthorityWithdrawn)?;
    if std::time::Instant::now() >= lease.deadline() {
        return Err(PlanError::AuthorityWithdrawn);
    }
    Ok(())
}

fn declarations(configuration: &agent::AgentConfiguration, plan: &plan::SessionExecutionPlan) -> Result<(), PlanError> {
    let declarations = configuration.dependencies.as_option();
    if declarations.is_some_and(|dependencies| {
        !dependencies.skills.is_empty()
            || !dependencies.tools.is_empty()
            || !dependencies.memories.is_empty()
            || !dependencies.connections.is_empty()
    }) {
        return Err(PlanError::UnsupportedRuntime);
    }
    let delegates = declarations
        .map(|dependencies| dependencies.delegates.as_slice())
        .unwrap_or_default();
    if delegates.len() != plan.dependencies.len() {
        return Err(PlanError::OriginalSourceMismatch);
    }
    let mut names = BTreeSet::new();
    for declaration in delegates {
        shape::bounded(&declaration.binding_name, 128)?;
        if !names.insert(declaration.binding_name.as_str())
            || !matches!(
                declaration.requirement.as_known(),
                Some(agent::DependencyRequirement::Required | agent::DependencyRequirement::Optional)
            )
        {
            return Err(PlanError::OriginalSourceMismatch);
        }
        let resolved = plan
            .dependencies
            .iter()
            .find(|dependency| dependency.binding_name.as_deref() == Some(declaration.binding_name.as_str()))
            .ok_or(PlanError::OriginalSourceMismatch)?;
        if !matches!(resolved.declaring_resource.resource.as_ref(),Some(plan::dependency_declarer::Resource::Agent(owner)) if plan.agent_revision.as_option()==Some(owner.as_ref()))
            || !matches!(resolved.declaration.as_ref(),Some(plan::resolved_dependency::Declaration::DelegateDeclaration(original)) if original.as_ref()==declaration)
        {
            return Err(PlanError::OriginalSourceMismatch);
        }
        match (declaration.selection.as_ref(), resolved.resolution.as_ref()) {
            (
                Some(agent::delegate_declaration::Selection::ExactPin(expected)),
                Some(plan::resolved_dependency::Resolution::Delegate(actual)),
            ) if expected.as_ref() == actual.as_ref() => {
                delegate_pin(actual)?;
            }
            (
                Some(agent::delegate_declaration::Selection::Selector(selector)),
                Some(plan::resolved_dependency::Resolution::Delegate(actual)),
            ) if !selector.labels.is_empty() => {
                if selector
                    .labels
                    .iter()
                    .any(|(name, value)| name.is_empty() || name.len() > 128 || value.len() > 4096)
                {
                    return Err(PlanError::InvalidPlan);
                }
                delegate_pin(actual)?;
            }
            (Some(_), Some(plan::resolved_dependency::Resolution::Unavailable(_))) => {
                return Err(PlanError::UnsupportedRuntime);
            }
            _ => return Err(PlanError::OriginalSourceMismatch),
        }
    }
    Ok(())
}

pub(crate) fn selected_delegate<'a, L: PlanSourceLease>(
    decoder: &RegisteredPlanDecoder,
    parent: &'a DecodedPlan,
    child: &DecodedPlan,
    child_source: &L,
    binding: &session::DelegationBindingRef,
) -> Result<&'a agent::DelegateDeclaration, PlanError> {
    shape::digest(
        &binding.parent_execution_plan_digest.algorithm,
        &binding.parent_execution_plan_digest.value,
    )?;
    if binding.parent_execution_plan_digest.value.as_slice() != parent.digest() {
        return Err(PlanError::OriginalSourceMismatch);
    }
    let binding_name = binding.binding_name.as_deref().ok_or(PlanError::InvalidPlan)?;
    let dependency = parent
        .value()
        .dependencies
        .iter()
        .find(|dependency| dependency.binding_name.as_deref() == Some(binding_name))
        .ok_or(PlanError::OriginalSourceMismatch)?;
    if !matches!(dependency.declaring_resource.resource.as_ref(),Some(plan::dependency_declarer::Resource::Agent(owner)) if parent.value().agent_revision.as_option()==Some(owner.as_ref()))
    {
        return Err(PlanError::UnsupportedRuntime);
    }
    let declaration = match dependency.declaration.as_ref() {
        Some(plan::resolved_dependency::Declaration::DelegateDeclaration(declaration)) => declaration.as_ref(),
        _ => return Err(PlanError::UnsupportedRuntime),
    };
    let target = match dependency.resolution.as_ref() {
        Some(plan::resolved_dependency::Resolution::Delegate(target)) => target.as_ref(),
        _ => return Err(PlanError::OriginalSourceMismatch),
    };
    let revision = child.value().agent_revision.as_option().ok_or(PlanError::InvalidPlan)?;
    if target.agent_id != revision.agent_id
        || target.revision != revision.revision_number
        || target.configuration_digest != revision.configuration.configuration_digest
    {
        return Err(PlanError::OriginalSourceMismatch);
    }
    if let Some(agent::delegate_declaration::Selection::Selector(selector)) = declaration.selection.as_ref() {
        live(child_source)?;
        let original = decoder.decode_agent_configuration(child_source.original_configuration()?)?;
        if selector.labels.is_empty()
            || !selector
                .labels
                .iter()
                .all(|(name, value)| original.selectable_labels.get(name) == Some(value))
        {
            return Err(PlanError::OriginalSourceMismatch);
        }
        live(child_source)?;
    }
    Ok(declaration)
}

fn delegate_pin(pin: &agent::ExactDelegateRevisionPin) -> Result<(), PlanError> {
    shape::bounded(&pin.agent_id, 128)?;
    if pin.revision == 0 {
        return Err(PlanError::InvalidPlan);
    }
    shape::digest(&pin.configuration_digest.algorithm, &pin.configuration_digest.value)
}
