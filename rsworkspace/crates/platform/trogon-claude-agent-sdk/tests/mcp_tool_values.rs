#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Value-object tests exercise valid and malformed JSON schema boundaries."
    )
)]

use serde_json::json;
use trogon_claude_agent_sdk::{
    McpToolInputSchema, McpToolInputSchemaError, McpToolName, OutputSchema, OutputSchemaError,
};

#[test]
fn mcp_tool_name_accepts_ascii_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    for value in ["a".to_owned(), "A0_.-z".to_owned(), "x".repeat(128)] {
        let name = McpToolName::from(value.clone());
        assert_eq!(name.as_str(), value);
        assert_eq!(name.to_string(), value);
        assert_eq!(name.clone(), name);
    }
    Ok(())
}

#[test]
fn mcp_tool_name_preserves_any_text_for_the_advertising_boundary() {
    for value in [
        String::new(),
        "two words".to_owned(),
        "tøøl".to_owned(),
        "x".repeat(129),
    ] {
        assert_eq!(McpToolName::from(value.clone()).as_str(), value);
    }
}

#[test]
fn mcp_tool_input_schema_preserves_json_and_validates_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let original = json!({
        "type":"object",
        "required":["value"],
        "properties":{"value":{"$ref":"#/$defs/text"}},
        "$defs":{"text":{"type":"string"}},
        "x-extension":{"nested":[1,null]},
    });
    let schema = McpToolInputSchema::new(serde_json::from_value(original.clone())?);
    assert!(McpToolInputSchema::validated(serde_json::from_value(original.clone())?).is_ok());
    assert_eq!(serde_json::to_value(schema.as_json())?, original);
    assert!(schema.is_valid(&json!({"value":"accepted"})));
    assert!(!schema.is_valid(&json!({"value":1})));
    assert!(!schema.is_valid(&json!({})));
    assert!(!schema.is_valid(&json!("not an object")));
    assert!(schema.clone().is_valid(&json!({"value":"cloned"})));
    Ok(())
}

#[test]
fn validated_mcp_tool_input_schema_rejects_non_object_declarations() -> Result<(), Box<dyn std::error::Error>> {
    for schema in [
        json!({}),
        json!({"type":"string"}),
        json!({"type":["object"]}),
        json!({"type":["object","null"]}),
    ] {
        assert!(matches!(
            McpToolInputSchema::validated(serde_json::from_value(schema.clone())?),
            Err(McpToolInputSchemaError::NotObject)
        ));
        let accepted = McpToolInputSchema::new(serde_json::from_value(schema.clone())?);
        assert_eq!(serde_json::to_value(accepted.as_json())?, schema);
    }
    Ok(())
}

#[test]
fn validated_mcp_tool_input_schema_rejects_malformed_schema() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        McpToolInputSchema::validated(serde_json::from_value(json!({"type":"object","properties":42}))?),
        Err(McpToolInputSchemaError::Schema(OutputSchemaError::Invalid(_)))
    ));
    Ok(())
}

#[test]
fn validated_mcp_tool_input_schema_rejects_external_references() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        McpToolInputSchema::validated(serde_json::from_value(json!({
            "type":"object",
            "properties":{"value":{"$ref":"https://example.com/schema"}},
        }))?),
        Err(McpToolInputSchemaError::Schema(OutputSchemaError::ExternalReference))
    ));
    Ok(())
}

#[test]
fn output_schema_retains_its_general_schema_contract() -> Result<(), Box<dyn std::error::Error>> {
    let schema = OutputSchema::new(serde_json::from_value(json!({"type":"string"}))?);
    assert!(schema.is_valid(&json!("accepted")));
    assert!(!schema.is_valid(&json!({})));
    Ok(())
}

#[test]
fn output_schemas_with_external_references_are_kept_like_both_upstream_sdks() -> Result<(), Box<dyn std::error::Error>>
{
    let declared = json!({"type":"object","properties":{"value":{"$ref":"https://example.com/schema"}}});
    let schema = OutputSchema::new(serde_json::from_value(declared.clone())?);
    assert_eq!(serde_json::to_value(schema.as_json())?, declared);
    assert!(matches!(
        OutputSchema::validated(serde_json::from_value(declared)?),
        Err(OutputSchemaError::ExternalReference)
    ));
    Ok(())
}
