use std::collections::{BTreeMap, BTreeSet};

use buffa::{MessageField, map_codec::Map as ProtoMap};
use trogonai_proto::agents::agents::{state_v1, v1};

use super::ProvisionAgent;
use super::domain::{
    AgentCharter, AgentDefinition, AgentId, AgentIdError, AgentName, AgentNameError, Annotations, AnnotationsError,
    ContentDigest, ContentDigestError, DelegateSelectors, DelegateSelectorsError, Labels, LabelsError, ModelId,
    ModelIdError, ModelParameters, ModelParametersError, ParentRef, ParentRefError, Principal, PrincipalError,
    RevisionNumber, RevisionNumberError, RuntimeId, RuntimeIdError, ToolSelectors, ToolSelectorsError,
};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CommandWireError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("duplicate value '{value}' in field '{field}'")]
    DuplicateValue { field: &'static str, value: String },
    #[error("initial revision must be 1, got {actual}")]
    InvalidInitialRevision { actual: RevisionNumber },
    #[error("invalid agent id: {0}")]
    InvalidAgentId(#[from] AgentIdError),
    #[error("invalid agent name: {0}")]
    InvalidAgentName(#[from] AgentNameError),
    #[error("invalid parent reference: {0}")]
    InvalidParent(#[from] ParentRefError),
    #[error("invalid principal: {0}")]
    InvalidPrincipal(#[from] PrincipalError),
    #[error("invalid labels: {0}")]
    InvalidLabels(#[from] LabelsError),
    #[error("invalid annotations: {0}")]
    InvalidAnnotations(#[from] AnnotationsError),
    #[error("invalid runtime id: {0}")]
    InvalidRuntime(#[from] RuntimeIdError),
    #[error("invalid model id: {0}")]
    InvalidModel(#[from] ModelIdError),
    #[error("invalid model parameters: {0}")]
    InvalidModelParameters(#[from] ModelParametersError),
    #[error("invalid tool selectors: {0}")]
    InvalidToolSelectors(#[from] ToolSelectorsError),
    #[error("invalid delegate selectors: {0}")]
    InvalidDelegateSelectors(#[from] DelegateSelectorsError),
    #[error("invalid content digest: {0}")]
    InvalidContentDigest(#[from] ContentDigestError),
    #[error("invalid revision number: {0}")]
    InvalidRevisionNumber(#[from] RevisionNumberError),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ProvisionedAgentIdentity {
    pub agent_id: AgentId,
    pub definition: AgentDefinition,
    pub content_digest: ContentDigest,
}

impl TryFrom<v1::ProvisionAgent> for ProvisionAgent {
    type Error = CommandWireError;

    fn try_from(value: v1::ProvisionAgent) -> Result<Self, Self::Error> {
        let charter = value
            .charter
            .as_option()
            .ok_or(CommandWireError::MissingField("charter"))?;
        Ok(Self {
            agent_id: AgentId::parse(&value.agent_id)?,
            definition: definition_from_wire(AgentDefinitionWire {
                name: &value.name,
                parent: &value.parent,
                owner: &value.owner,
                labels: &value.labels,
                annotations: &value.annotations,
                charter,
            })?,
            principal: Principal::parse(&value.principal)?,
            content_digest: ContentDigest::parse(&value.content_digest)?,
            transient_annotations: annotations_from_wire(&value.transient_annotations)?,
        })
    }
}

pub(super) fn provisioned_agent_from_event(
    value: &v1::AgentProvisioned,
) -> Result<ProvisionedAgentIdentity, CommandWireError> {
    let charter = value
        .charter
        .as_option()
        .ok_or(CommandWireError::MissingField("charter"))?;
    let revision = value
        .revision
        .as_option()
        .ok_or(CommandWireError::MissingField("revision"))?;
    let revision_number = RevisionNumber::new(revision.number)?;
    if revision_number != RevisionNumber::GENESIS {
        return Err(CommandWireError::InvalidInitialRevision {
            actual: revision_number,
        });
    }
    Principal::parse(&value.provisioned_by)?;
    annotations_from_wire(&value.transient_annotations)?;
    Ok(ProvisionedAgentIdentity {
        agent_id: AgentId::parse(&value.agent_id)?,
        definition: definition_from_wire(AgentDefinitionWire {
            name: &value.name,
            parent: &value.parent,
            owner: &value.owner,
            labels: &value.labels,
            annotations: &value.annotations,
            charter,
        })?,
        content_digest: ContentDigest::parse(&revision.content_digest)?,
    })
}

pub(super) fn provisioned_agent_from_state(
    value: &state_v1::ProvisionedAgent,
) -> Result<ProvisionedAgentIdentity, CommandWireError> {
    let charter = value
        .charter
        .as_option()
        .ok_or(CommandWireError::MissingField("charter"))?;
    Ok(ProvisionedAgentIdentity {
        agent_id: AgentId::parse(&value.agent_id)?,
        definition: definition_from_wire(AgentDefinitionWire {
            name: &value.name,
            parent: &value.parent,
            owner: &value.owner,
            labels: &value.labels,
            annotations: &value.annotations,
            charter,
        })?,
        content_digest: ContentDigest::parse(&value.content_digest)?,
    })
}

pub(super) fn provisioned_agent_to_state(value: &ProvisionedAgentIdentity) -> state_v1::ProvisionedAgent {
    let definition = &value.definition;
    state_v1::ProvisionedAgent {
        agent_id: value.agent_id.as_str().to_string(),
        name: definition.name().as_str().to_string(),
        parent: definition.parent().as_str().to_string(),
        owner: definition.owner().as_str().to_string(),
        labels: labels_to_wire(definition.labels()),
        annotations: annotations_to_wire(definition.annotations()),
        charter: MessageField::some(charter_to_wire(definition.charter())),
        content_digest: value.content_digest.as_str().to_string(),
    }
}

struct AgentDefinitionWire<'a> {
    name: &'a str,
    parent: &'a str,
    owner: &'a str,
    labels: &'a [v1::Label],
    annotations: &'a ProtoMap<String, String>,
    charter: &'a v1::Charter,
}

fn definition_from_wire(value: AgentDefinitionWire<'_>) -> Result<AgentDefinition, CommandWireError> {
    Ok(AgentDefinition::new(
        AgentName::parse(value.name)?,
        ParentRef::parse(value.parent)?,
        Principal::parse(value.owner)?,
        Labels::new(string_entries(
            value.labels.iter().map(|entry| (&entry.key, &entry.value)),
            "labels",
        )?)?,
        annotations_from_wire(value.annotations)?,
        charter_from_wire(value.charter)?,
    ))
}

fn annotations_from_wire(values: &ProtoMap<String, String>) -> Result<Annotations, CommandWireError> {
    Ok(Annotations::new(
        values.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
    )?)
}

fn charter_from_wire(value: &v1::Charter) -> Result<AgentCharter, CommandWireError> {
    let model = value
        .model
        .as_option()
        .ok_or(CommandWireError::MissingField("charter.model"))?;
    let tools = value
        .tools
        .as_option()
        .ok_or(CommandWireError::MissingField("charter.tools"))?;
    let delegates = value
        .delegates
        .as_option()
        .ok_or(CommandWireError::MissingField("charter.delegates"))?;
    Ok(AgentCharter::new(
        RuntimeId::parse(&value.runtime)?,
        ModelId::parse(&model.id)?,
        ModelParameters::new(string_entries(
            model.params.iter().map(|entry| (&entry.key, &entry.value)),
            "charter.model.params",
        )?)?,
        ToolSelectors::new(
            unique_strings(&tools.required, "charter.tools.required")?,
            unique_strings(&tools.optional, "charter.tools.optional")?,
        )?,
        DelegateSelectors::new(
            unique_strings(&delegates.required, "charter.delegates.required")?,
            unique_strings(&delegates.optional, "charter.delegates.optional")?,
        )?,
    ))
}

fn string_entries<'a>(
    values: impl IntoIterator<Item = (&'a String, &'a String)>,
    field: &'static str,
) -> Result<BTreeMap<String, String>, CommandWireError> {
    let mut result = BTreeMap::new();
    for (key, value) in values {
        if result.insert(key.clone(), value.clone()).is_some() {
            return Err(CommandWireError::DuplicateValue {
                field,
                value: key.clone(),
            });
        }
    }
    Ok(result)
}

fn unique_strings(values: &[String], field: &'static str) -> Result<BTreeSet<String>, CommandWireError> {
    let mut result = BTreeSet::new();
    for value in values {
        if !result.insert(value.clone()) {
            return Err(CommandWireError::DuplicateValue {
                field,
                value: value.clone(),
            });
        }
    }
    Ok(result)
}

pub(super) fn charter_to_wire(value: &AgentCharter) -> v1::Charter {
    v1::Charter {
        runtime: value.runtime().as_str().to_string(),
        model: buffa::MessageField::some(v1::Model {
            id: value.model_id().as_str().to_string(),
            params: value
                .model_params()
                .as_map()
                .iter()
                .map(|(key, value)| v1::ModelParameter {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
        }),
        tools: buffa::MessageField::some(v1::ToolDependencies {
            required: value.tools().required().iter().cloned().collect(),
            optional: value.tools().optional().iter().cloned().collect(),
        }),
        delegates: buffa::MessageField::some(v1::DelegateDependencies {
            required: value.delegates().required().iter().cloned().collect(),
            optional: value.delegates().optional().iter().cloned().collect(),
        }),
    }
}

pub(super) fn labels_to_wire(value: &Labels) -> Vec<v1::Label> {
    value
        .as_map()
        .iter()
        .map(|(key, value)| v1::Label {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

pub(super) fn annotations_to_wire(value: &Annotations) -> ProtoMap<String, String> {
    value
        .as_map()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests;
