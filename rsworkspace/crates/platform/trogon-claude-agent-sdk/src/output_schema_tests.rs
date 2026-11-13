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
fn schema_construction_rejects_invalid_or_remote_references() -> Result<(), Box<dyn std::error::Error>> {
    assert!(OutputSchema::new(serde_json::from_value(json!({"type":"invalid"}))?).is_err());
    assert!(matches!(
        OutputSchema::new(serde_json::from_value(json!({"$ref":"https://example.com/schema"}))?),
        Err(OutputSchemaError::ExternalReference)
    ));
    let schema = OutputSchema::new(serde_json::from_value(
        json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}}}),
    )?)?;
    assert!(schema.is_valid(&json!({"ok":true})));
    assert!(!schema.is_valid(&json!({"ok":"true"})));
    Ok(())
}
