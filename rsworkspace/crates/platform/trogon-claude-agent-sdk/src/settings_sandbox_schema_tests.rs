#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;

use super::*;

#[test]
fn undefined_parses_to_nothing_and_a_wrong_type_is_rejected() {
    for schema in SandboxConfigSchema::ALL {
        assert_eq!(schema.parse(None), Ok(None));
        assert!(schema.accepts(None));
        assert_eq!(schema.parse(Some(&json!(1))), Err(SandboxConfigRejected(schema)));
        assert!(!schema.accepts(Some(&json!(1))));
    }
}

#[test]
fn a_valid_document_parses_to_itself_without_unknown_keys() {
    let parsed = SandboxConfigSchema::Filesystem
        .parse(Some(&json!({"disabled": true, "extra": 1})))
        .expect("accepted");
    assert_eq!(parsed, Some(json!({"disabled": true})));
}

#[test]
fn every_schema_has_a_distinct_name_and_root() {
    let names: Vec<_> = SandboxConfigSchema::ALL.iter().map(ToString::to_string).collect();
    assert_eq!(names.len(), 3);
    assert_ne!(names[0], names[1]);
    assert_ne!(
        SandboxConfigSchema::Network.root(),
        SandboxConfigSchema::Credentials.root()
    );
}
