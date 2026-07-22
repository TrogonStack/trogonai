use std::collections::{BTreeMap, BTreeSet};

use buffa::MessageField;
use trogonai_proto::agents::agents::v1;

use super::ProvisionAgent;
use super::domain::{
    AgentCharter, AgentDefinition, AgentId, AgentName, Annotations, ContentDigest, DelegateSelectors, Labels, ModelId,
    ModelParameters, ParentRef, Principal, RuntimeId, ToolSelectors,
};

pub fn agent_id() -> AgentId {
    AgentId::parse("agent-1").unwrap()
}

fn operator() -> Principal {
    Principal::parse("operator@tenant").unwrap()
}

fn definition() -> AgentDefinition {
    AgentDefinition::new(
        AgentName::parse("reviewer").unwrap(),
        ParentRef::parse("folder-backend").unwrap(),
        Principal::parse("owner@tenant").unwrap(),
        Labels::new(BTreeMap::from([("family".to_string(), "reviewer".to_string())])).unwrap(),
        Annotations::new(BTreeMap::from([("vertical".to_string(), "software".to_string())])).unwrap(),
        AgentCharter::new(
            RuntimeId::parse("claude-code").unwrap(),
            ModelId::parse("anthropic/claude-opus").unwrap(),
            ModelParameters::default(),
            ToolSelectors::new(BTreeSet::from(["read".to_string()]), BTreeSet::new()).unwrap(),
            DelegateSelectors::default(),
        ),
    )
}

pub fn digest(character: char) -> ContentDigest {
    ContentDigest::parse(&format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

pub fn provision_command() -> ProvisionAgent {
    ProvisionAgent {
        agent_id: agent_id(),
        definition: definition(),
        principal: operator(),
        content_digest: digest('a'),
        transient_annotations: Annotations::new(BTreeMap::from([("request".to_string(), "first".to_string())]))
            .unwrap(),
    }
}

pub fn provisioned_event() -> v1::AgentEvent {
    let command = provision_command();
    let definition = command.definition;
    v1::AgentEvent {
        event: Some(
            v1::AgentProvisioned {
                agent_id: command.agent_id.as_str().to_string(),
                name: definition.name().as_str().to_string(),
                parent: definition.parent().as_str().to_string(),
                owner: definition.owner().as_str().to_string(),
                labels: vec![v1::Label {
                    key: "family".to_string(),
                    value: "reviewer".to_string(),
                }],
                annotations: [("vertical".to_string(), "software".to_string())].into_iter().collect(),
                charter: MessageField::some(v1::Charter {
                    runtime: "claude-code".to_string(),
                    model: MessageField::some(v1::Model {
                        id: "anthropic/claude-opus".to_string(),
                        params: Vec::new(),
                    }),
                    tools: MessageField::some(v1::ToolDependencies {
                        required: vec!["read".to_string()],
                        optional: Vec::new(),
                    }),
                    delegates: MessageField::some(v1::DelegateDependencies {
                        required: Vec::new(),
                        optional: Vec::new(),
                    }),
                }),
                provisioned_by: operator().as_str().to_string(),
                revision: MessageField::some(v1::RevisionRef {
                    number: 1,
                    content_digest: digest('a').as_str().to_string(),
                }),
                transient_annotations: [("request".to_string(), "first".to_string())].into_iter().collect(),
            }
            .into(),
        ),
    }
}
