use super::*;
use crate::commands::domain::{DelegateSelectors, ModelId, ModelParameters, RuntimeId, ToolSelectors};

#[test]
fn composes_only_validated_domain_values() {
    let definition = AgentDefinition::new(
        AgentName::parse("reviewer").unwrap(),
        ParentRef::parse("org/root").unwrap(),
        Principal::parse("owner@tenant").unwrap(),
        Labels::default(),
        Annotations::default(),
        AgentCharter::new(
            RuntimeId::parse("runtime").unwrap(),
            ModelId::parse("model").unwrap(),
            ModelParameters::default(),
            ToolSelectors::default(),
            DelegateSelectors::default(),
        ),
    );
    assert_eq!(definition.name().as_str(), "reviewer");
    assert_eq!(definition.parent().as_str(), "org/root");
}
