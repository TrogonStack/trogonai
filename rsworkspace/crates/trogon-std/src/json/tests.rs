use super::*;

#[test]
fn std_json_serialize_serializes_value() {
    let s = StdJsonSerialize;
    let out = s.to_vec(&serde_json::json!({"k": 1})).unwrap();
    assert_eq!(out, br#"{"k":1}"#);
}

#[test]
fn fail_next_serialize_clone_shares_remaining_count() {
    let a = FailNextSerialize::new(1);
    let b = a.clone();
    let _ = a.to_vec(&serde_json::json!({"x": 1}));
    let result = b.to_vec(&serde_json::json!({"x": 1}));
    assert!(result.is_ok());
}
