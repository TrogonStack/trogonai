use super::*;

#[test]
fn snapshot_type_name_rejects_empty_value() {
    assert_eq!(
        SnapshotTypeName::new("").unwrap_err(),
        InvalidSnapshotTypeNameError::Empty
    );
}

#[test]
fn snapshot_type_name_rejects_control_characters() {
    assert_eq!(
        SnapshotTypeName::new("test.snapshot\nv1").unwrap_err(),
        InvalidSnapshotTypeNameError::ContainsControlCharacter
    );
}

#[test]
fn snapshot_type_name_serializes_as_string() {
    let snapshot_type = SnapshotTypeName::new("test.snapshot.v1").unwrap();

    assert_eq!(serde_json::to_string(&snapshot_type).unwrap(), "\"test.snapshot.v1\"");
}

#[test]
fn snapshot_type_name_deserializes_with_validation() {
    let snapshot_type: SnapshotTypeName = serde_json::from_str("\"test.snapshot.v1\"").unwrap();

    assert_eq!(snapshot_type.as_str(), "test.snapshot.v1");
    assert!(serde_json::from_str::<SnapshotTypeName>("\"\"").is_err());
}

#[test]
fn snapshot_type_name_supports_string_conversions_and_views() {
    let parsed = "test.snapshot.v1".parse::<SnapshotTypeName>().unwrap();
    let from_string = SnapshotTypeName::try_from("test.snapshot.v1".to_string()).unwrap();
    let from_str = SnapshotTypeName::try_from("test.snapshot.v1").unwrap();

    assert_eq!(parsed, from_string);
    assert_eq!(from_str.as_ref(), "test.snapshot.v1");
    assert_eq!(std::borrow::Borrow::<str>::borrow(&parsed), "test.snapshot.v1");
    assert_eq!(parsed.to_string(), "test.snapshot.v1");
}

#[test]
fn invalid_snapshot_type_name_displays_reason() {
    assert_eq!(
        InvalidSnapshotTypeNameError::Empty.to_string(),
        "snapshot type name cannot be empty"
    );
    assert_eq!(
        InvalidSnapshotTypeNameError::ContainsControlCharacter.to_string(),
        "snapshot type name cannot contain control characters"
    );
}
