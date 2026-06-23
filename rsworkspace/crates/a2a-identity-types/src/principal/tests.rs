use super::*;

#[test]
fn new_wraps_subject_in_json_object() {
    let principal = SpiceDbPrincipal::new("user:alice");
    assert_eq!(principal.0["spicedb_subject"], json!("user:alice"));
}

#[test]
fn extracts_subject_when_present() {
    let principal = SpiceDbPrincipal::new("user:alice");
    assert_eq!(principal.spicedb_subject().unwrap().as_str(), "user:alice");
}

#[test]
fn missing_subject_yields_none() {
    let principal = SpiceDbPrincipal(json!({ "other": "value" }));
    assert!(principal.spicedb_subject().is_none());
}

#[test]
fn empty_subject_yields_none() {
    let principal = SpiceDbPrincipal(json!({ "spicedb_subject": "" }));
    assert!(principal.spicedb_subject().is_none());
}

#[test]
fn non_string_subject_yields_none() {
    let principal = SpiceDbPrincipal(json!({ "spicedb_subject": 42 }));
    assert!(principal.spicedb_subject().is_none());
}

#[test]
fn subject_helpers_roundtrip() {
    let subject = SpiceDbSubject::new("user:bob");
    assert_eq!(subject.as_str(), "user:bob");
}
