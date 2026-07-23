use std::collections::{BTreeMap, BTreeSet};

use buffa::MessageField;

use super::*;
use crate::commands::domain::{DelegateSelectorsError, ToolSelectorsError};
use crate::commands::test_support::digest;

fn charter() -> v1::Charter {
    v1::Charter {
        runtime: "claude-code".to_string(),
        model: MessageField::some(v1::Model {
            id: "anthropic/claude-opus".to_string(),
            params: vec![v1::ModelParameter {
                key: "speed".to_string(),
                value: "standard".to_string(),
            }],
        }),
        tools: MessageField::some(v1::ToolDependencies {
            required: vec!["read".to_string()],
            optional: vec!["search".to_string()],
        }),
        delegates: MessageField::some(v1::DelegateDependencies {
            required: Vec::new(),
            optional: vec!["family=researcher".to_string()],
        }),
    }
}

fn provision_proto() -> v1::ProvisionAgent {
    v1::ProvisionAgent {
        agent_id: "agent-1".to_string(),
        name: "reviewer".to_string(),
        parent: "folder-backend".to_string(),
        owner: "owner@tenant".to_string(),
        labels: vec![v1::Label {
            key: "family".to_string(),
            value: "reviewer".to_string(),
        }],
        annotations: [("vertical".to_string(), "software".to_string())].into_iter().collect(),
        charter: MessageField::some(charter()),
        principal: "operator@tenant".to_string(),
        content_digest: digest('a').as_str().to_string(),
        transient_annotations: [("request".to_string(), "first".to_string())].into_iter().collect(),
    }
}

fn provisioned_proto() -> v1::AgentProvisioned {
    v1::AgentProvisioned {
        agent_id: "agent-1".to_string(),
        name: "reviewer".to_string(),
        parent: "folder-backend".to_string(),
        owner: "owner@tenant".to_string(),
        labels: vec![v1::Label {
            key: "family".to_string(),
            value: "reviewer".to_string(),
        }],
        annotations: [("vertical".to_string(), "software".to_string())].into_iter().collect(),
        charter: MessageField::some(charter()),
        provisioned_by: "operator@tenant".to_string(),
        revision: MessageField::some(v1::RevisionRef {
            number: 1,
            content_digest: digest('a').as_str().to_string(),
        }),
        transient_annotations: [("request".to_string(), "first".to_string())].into_iter().collect(),
    }
}

#[test]
fn converts_a_complete_provision_command_once() {
    let command = ProvisionAgent::try_from(provision_proto()).unwrap();

    assert_eq!(command.agent_id.as_str(), "agent-1");
    assert_eq!(command.definition.owner().as_str(), "owner@tenant");
    assert_eq!(command.definition.labels().get("family"), Some("reviewer"));
    assert_eq!(command.definition.charter().runtime().as_str(), "claude-code");
    assert_eq!(command.content_digest, digest('a'));
    assert_eq!(command.transient_annotations.get("request"), Some("first"));
}

#[test]
fn rejects_a_non_sha256_provision_digest() {
    let mut proto = provision_proto();
    proto.content_digest = "digest-1".to_string();

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::InvalidContentDigest(_))
    ));
}

#[test]
fn rejects_duplicate_label_keys() {
    let mut proto = provision_proto();
    proto.labels.push(proto.labels[0].clone());

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::DuplicateValue { field: "labels", .. })
    ));
}

#[test]
fn rejects_invalid_annotation_keys() {
    let mut proto = provision_proto();
    proto.annotations.insert("bad key".to_string(), "value".to_string());

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::InvalidAnnotations(_))
    ));
}

#[test]
fn rejects_invalid_transient_annotation_keys() {
    let mut proto = provision_proto();
    proto
        .transient_annotations
        .insert("bad key".to_string(), "value".to_string());

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::InvalidAnnotations(_))
    ));
}

#[test]
fn rejects_a_missing_charter() {
    let mut proto = provision_proto();
    proto.charter = MessageField::none();

    assert_eq!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::MissingField("charter"))
    );
}

#[test]
fn rejects_duplicate_model_parameter_keys() {
    let mut proto = provision_proto();
    proto
        .charter
        .as_option_mut()
        .unwrap()
        .model
        .as_option_mut()
        .unwrap()
        .params
        .push(v1::ModelParameter {
            key: "speed".to_string(),
            value: "turbo".to_string(),
        });

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::DuplicateValue {
            field: "charter.model.params",
            ..
        })
    ));
}

#[test]
fn rejects_duplicate_required_tool_selectors() {
    let mut proto = provision_proto();
    let tools = proto.charter.as_option_mut().unwrap().tools.as_option_mut().unwrap();
    tools.required.push(tools.required[0].clone());

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::DuplicateValue {
            field: "charter.tools.required",
            ..
        })
    ));
}

#[test]
fn rejects_duplicate_optional_delegate_selectors() {
    let mut proto = provision_proto();
    let delegates = proto
        .charter
        .as_option_mut()
        .unwrap()
        .delegates
        .as_option_mut()
        .unwrap();
    delegates.optional.push(delegates.optional[0].clone());

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::DuplicateValue {
            field: "charter.delegates.optional",
            ..
        })
    ));
}

#[test]
fn rejects_a_tool_selector_in_both_sets() {
    let mut proto = provision_proto();
    let tools = proto.charter.as_option_mut().unwrap().tools.as_option_mut().unwrap();
    tools.required = vec!["shared".to_string()];
    tools.optional = vec!["shared".to_string()];

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::InvalidToolSelectors(
            ToolSelectorsError::Overlap { .. }
        ))
    ));
}

#[test]
fn rejects_a_delegate_selector_in_both_sets() {
    let mut proto = provision_proto();
    let delegates = proto
        .charter
        .as_option_mut()
        .unwrap()
        .delegates
        .as_option_mut()
        .unwrap();
    delegates.required = vec!["shared".to_string()];
    delegates.optional = vec!["shared".to_string()];

    assert!(matches!(
        ProvisionAgent::try_from(proto),
        Err(CommandWireError::InvalidDelegateSelectors(
            DelegateSelectorsError::Overlap { .. }
        ))
    ));
}

#[test]
fn rejects_a_non_genesis_provisioned_revision() {
    let mut proto = provisioned_proto();
    proto.revision.as_option_mut().unwrap().number = 2;

    assert!(matches!(
        provisioned_agent_from_event(&proto),
        Err(CommandWireError::InvalidInitialRevision { actual })
            if actual == RevisionNumber::new(2).unwrap()
    ));
}

#[test]
fn round_trips_only_persistent_retry_identity_through_state() {
    let identity = provisioned_agent_from_event(&provisioned_proto()).unwrap();
    let state = provisioned_agent_to_state(&identity);

    assert_eq!(state.agent_id, "agent-1");
    assert_eq!(state.annotations.get("vertical").map(String::as_str), Some("software"));
    assert_eq!(state.content_digest, digest('a').as_str());
    assert_eq!(provisioned_agent_from_state(&state).unwrap(), identity);
}

#[test]
fn converts_model_parameters_to_wire_entries() {
    let charter = AgentCharter::new(
        RuntimeId::parse("claude-code").unwrap(),
        ModelId::parse("anthropic/claude-opus").unwrap(),
        ModelParameters::new(BTreeMap::from([("speed".to_string(), "standard".to_string())])).unwrap(),
        ToolSelectors::new(BTreeSet::from(["read".to_string()]), BTreeSet::new()).unwrap(),
        DelegateSelectors::default(),
    );

    let wire = charter_to_wire(&charter);

    assert_eq!(
        wire.model.params,
        vec![v1::ModelParameter {
            key: "speed".to_string(),
            value: "standard".to_string(),
        }]
    );
}
