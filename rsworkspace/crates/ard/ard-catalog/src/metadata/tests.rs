use serde_json::json;

use super::*;

#[test]
fn accepts_scalar_metadata_values() {
    let metadata = Metadata::new(json!({
        "tier": "internal",
        "enabled": true,
        "score": 1,
        "owner": null
    }))
    .unwrap();

    assert_eq!(metadata.as_value()["tier"], "internal");
}

#[test]
fn rejects_nested_metadata_values() {
    assert_eq!(
        Metadata::new(json!({
            "nested": { "not": "allowed" }
        })),
        Err(MetadataError::InvalidValue {
            key: "nested".to_owned()
        })
    );
}
