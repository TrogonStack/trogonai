#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use serde_json::json;
#[test]
fn validated_schemas_reject_invalid_or_remote_references() -> Result<(), Box<dyn std::error::Error>> {
    assert!(OutputSchema::validated(serde_json::from_value(json!({"type":"invalid"}))?).is_err());
    assert!(matches!(
        OutputSchema::validated(serde_json::from_value(json!({"$ref":"https://example.com/schema"}))?),
        Err(OutputSchemaError::ExternalReference)
    ));
    let schema = OutputSchema::validated(serde_json::from_value(
        json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}}}),
    )?)?;
    assert!(schema.is_valid(&json!({"ok":true})));
    assert!(!schema.is_valid(&json!({"ok":"true"})));
    assert_eq!(schema.compile_error(), None);
    Ok(())
}

#[test]
fn schemas_are_kept_verbatim_even_when_they_do_not_compile() -> Result<(), Box<dyn std::error::Error>> {
    for original in [json!({"type":"invalid"}), json!({"$ref":"https://example.com/schema"})] {
        let schema = OutputSchema::new(serde_json::from_value(original.clone())?);
        assert_eq!(Value::Object(schema.as_json().clone()), original);
        assert!(schema.compile_error().is_some());
        assert!(!schema.is_valid(&json!({})));
        assert!(schema.validation_error(&json!({})).is_some());
    }
    Ok(())
}

#[test]
fn a_compiling_schema_describes_why_an_instance_fails() -> Result<(), Box<dyn std::error::Error>> {
    let schema = OutputSchema::new(serde_json::from_value(json!({"type":"object","required":["ok"]}))?);
    assert_eq!(schema.validation_error(&json!({"ok":true})), None);
    let message = schema.validation_error(&json!({})).ok_or("the instance matched")?;
    assert!(message.contains("\"ok\""), "{message}");
    Ok(())
}
