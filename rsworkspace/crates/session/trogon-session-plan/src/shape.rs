use crate::{
    PlanError,
    wire::{agent, plan},
};
use sha2::{Digest as _, Sha256};

pub(crate) fn digest(algorithm: &str, value: &[u8]) -> Result<(), PlanError> {
    if algorithm != "sha256" || value.len() != 32 {
        return Err(PlanError::DigestMismatch);
    }
    Ok(())
}

pub(crate) fn contract(value: &agent::ConfigurationContractRef) -> Result<(), PlanError> {
    bounded(&value.message_type, 512)?;
    bounded(&value.version, 128)?;
    let hash = value.definition_digest.as_option().ok_or(PlanError::InvalidPlan)?;
    digest(&hash.algorithm, &hash.value)
}

pub(crate) fn bounded(value: &str, maximum: usize) -> Result<(), PlanError> {
    if value.is_empty() || value.len() > maximum {
        return Err(PlanError::InvalidPlan);
    }
    Ok(())
}

pub(crate) fn native(value: &agent::NativeConfiguration) -> Result<(), PlanError> {
    contract(value.contract.as_option().ok_or(PlanError::InvalidPlan)?)?;
    if value.configuration_bytes.len() > 32768 {
        return Err(PlanError::Limit);
    }
    let hash = value.configuration_digest.as_option().ok_or(PlanError::InvalidPlan)?;
    digest(&hash.algorithm, &hash.value)?;
    let actual: [u8; 32] = Sha256::digest(&value.configuration_bytes).into();
    if hash.value.as_slice() != actual {
        return Err(PlanError::DigestMismatch);
    }
    Ok(())
}

pub(crate) fn plan(value: &plan::SessionExecutionPlan) -> Result<(), PlanError> {
    bounded(value.session_id.as_deref().ok_or(PlanError::InvalidPlan)?, 128)?;
    bounded(&value.project_id.as_option().ok_or(PlanError::InvalidPlan)?.value, 128)?;
    let revision = value.agent_revision.as_option().ok_or(PlanError::InvalidPlan)?;
    bounded(&revision.agent_id, 128)?;
    if revision.revision_number == 0 {
        return Err(PlanError::InvalidPlan);
    }
    let effective = value
        .effective_configuration
        .as_option()
        .ok_or(PlanError::InvalidPlan)?;
    let config_hash = effective
        .configuration_digest
        .as_option()
        .ok_or(PlanError::InvalidPlan)?;
    digest(&config_hash.algorithm, &config_hash.value)?;
    let base_hash = revision
        .configuration
        .configuration_digest
        .as_option()
        .ok_or(PlanError::InvalidPlan)?;
    digest(&base_hash.algorithm, &base_hash.value)?;
    if !value.candidate.is_set() && revision.configuration.as_option() != Some(effective) {
        return Err(PlanError::InvalidPlan);
    }
    if let Some(candidate) = value.candidate.as_option() {
        bounded(&candidate.candidate_id.value, 128)?;
        let hash = candidate.content_digest.as_option().ok_or(PlanError::InvalidPlan)?;
        digest(&hash.algorithm, &hash.value)?;
    }
    let implementation = value.implementation.as_option().ok_or(PlanError::InvalidPlan)?;
    if implementation.implementation.is_none() {
        return Err(PlanError::InvalidPlan);
    }
    let hash = implementation
        .definition_digest
        .as_option()
        .ok_or(PlanError::InvalidPlan)?;
    digest(&hash.algorithm, &hash.value)?;
    let settings = value.runtime_settings.as_option().ok_or(PlanError::InvalidPlan)?;
    let effective = value
        .effective_runtime_configuration
        .as_option()
        .ok_or(PlanError::InvalidPlan)?;
    native(settings)?;
    native(effective)?;
    if settings
        .configuration_bytes
        .len()
        .saturating_add(effective.configuration_bytes.len())
        > 49152
    {
        return Err(PlanError::Limit);
    }
    contract(value.execution_contract.as_option().ok_or(PlanError::InvalidPlan)?)?;
    let model = value.model_access.as_option().ok_or(PlanError::InvalidPlan)?;
    let compaction = value.compaction.as_option().ok_or(PlanError::InvalidPlan)?;
    if model.profile.is_none() || compaction.profile.is_none() {
        return Err(PlanError::InvalidPlan);
    }
    if matches!(
        implementation.implementation,
        Some(agent::agent_implementation::Implementation::PlatformHarness(_))
    ) && (!matches!(model.profile, Some(plan::model_access_profile::Profile::Managed(_)))
        || !matches!(compaction.profile, Some(plan::compaction_profile::Profile::Platform(_))))
    {
        return Err(PlanError::UnsupportedRuntime);
    }
    if value.dependencies.len() > 256 {
        return Err(PlanError::Limit);
    }
    let mut names = Vec::new();
    for dependency in &value.dependencies {
        let owner = dependency
            .declaring_resource
            .as_option()
            .ok_or(PlanError::InvalidPlan)?;
        if owner.resource.is_none() || dependency.declaration.is_none() || dependency.resolution.is_none() {
            return Err(PlanError::InvalidPlan);
        }
        let binding = dependency.binding_name.as_deref().ok_or(PlanError::InvalidPlan)?;
        bounded(binding, 128)?;
        if names
            .iter()
            .any(|(previous_owner, previous_binding)| *previous_owner == owner && *previous_binding == binding)
        {
            return Err(PlanError::InvalidPlan);
        }
        names.push((owner, binding));
    }
    if let Some(work) = value.work.as_option() {
        contract(work.contract.as_option().ok_or(PlanError::InvalidPlan)?)?;
        if work.inputs.len() > 128 {
            return Err(PlanError::Limit);
        }
    }
    let variables = value.variables.as_option().ok_or(PlanError::InvalidPlan)?;
    if !variables.values.is_set() {
        return Err(PlanError::InvalidPlan);
    }
    let directory = value.working_directory.as_option().ok_or(PlanError::InvalidPlan)?;
    let checkout = directory.checkout.as_option().ok_or(PlanError::InvalidPlan)?;
    bounded(&checkout.checkout_id, 128)?;
    if checkout.uri.len() > 4096 || checkout.revision.as_ref().is_some_and(|revision| revision.len() > 128) {
        return Err(PlanError::Limit);
    }
    contract(
        directory
            .filesystem_contract
            .as_option()
            .ok_or(PlanError::InvalidPlan)?,
    )?;
    let path = directory.relative_path.as_deref().ok_or(PlanError::InvalidPlan)?;
    if path.len() > 4096
        || (!path.is_empty()
            && (path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
                || path.contains(['\\', '%', ':', '\0'])))
    {
        return Err(PlanError::InvalidPlan);
    }
    Ok(())
}
