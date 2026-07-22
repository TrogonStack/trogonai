use super::*;

#[test]
fn composes_independently_validated_charter_values() {
    let charter = AgentCharter::new(
        RuntimeId::parse("claude-code").unwrap(),
        ModelId::parse("anthropic/opus").unwrap(),
        ModelParameters::default(),
        ToolSelectors::default(),
        DelegateSelectors::default(),
    );
    assert_eq!(charter.runtime().as_str(), "claude-code");
    assert_eq!(charter.model_id().as_str(), "anthropic/opus");
}
