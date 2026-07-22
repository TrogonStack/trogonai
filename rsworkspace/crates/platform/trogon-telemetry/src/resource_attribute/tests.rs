use super::*;

#[test]
fn acp_prefix_maps_to_expected_key_value() {
    assert_eq!(
        KeyValue::from(ResourceAttribute::acp_prefix("test")),
        KeyValue::new(crate::constants::RESOURCE_ATTRIBUTE_ACP_PREFIX, "test")
    );
}

#[test]
fn mcp_prefix_maps_to_expected_key_value() {
    assert_eq!(
        KeyValue::from(ResourceAttribute::mcp_prefix("test")),
        KeyValue::new(crate::constants::RESOURCE_ATTRIBUTE_MCP_PREFIX, "test")
    );
}
