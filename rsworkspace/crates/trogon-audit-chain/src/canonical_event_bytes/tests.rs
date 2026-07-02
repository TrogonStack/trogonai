use super::*;

#[test]
fn key_order_does_not_affect_canonical_bytes() {
    let a = CanonicalEventBytes::from_json_slice(br#"{"b":1,"a":2}"#).expect("valid json");
    let b = CanonicalEventBytes::from_json_slice(br#"{"a":2,"b":1}"#).expect("valid json");
    assert_eq!(a, b);
}

#[test]
fn whitespace_does_not_affect_canonical_bytes() {
    let a = CanonicalEventBytes::from_json_slice(br#"{"a":1}"#).expect("valid json");
    let b = CanonicalEventBytes::from_json_slice(b"{ \"a\" : 1 }").expect("valid json");
    assert_eq!(a, b);
}

#[test]
fn nested_object_keys_are_sorted() {
    let a = CanonicalEventBytes::from_json_slice(br#"{"outer":{"z":1,"a":2}}"#).expect("valid json");
    let b = CanonicalEventBytes::from_json_slice(br#"{"outer":{"a":2,"z":1}}"#).expect("valid json");
    assert_eq!(a, b);
}

#[test]
fn array_element_order_is_preserved() {
    let a = CanonicalEventBytes::from_json_slice(br#"[1,2,3]"#).expect("valid json");
    let b = CanonicalEventBytes::from_json_slice(br#"[3,2,1]"#).expect("valid json");
    assert_ne!(a, b, "array order is semantically meaningful and must not be reordered");
}

#[test]
fn objects_inside_arrays_are_sorted() {
    let a = CanonicalEventBytes::from_json_slice(br#"[{"b":1,"a":2}]"#).expect("valid json");
    let b = CanonicalEventBytes::from_json_slice(br#"[{"a":2,"b":1}]"#).expect("valid json");
    assert_eq!(a, b);
}

#[test]
fn different_values_produce_different_bytes() {
    let a = CanonicalEventBytes::from_json_slice(br#"{"a":1}"#).expect("valid json");
    let b = CanonicalEventBytes::from_json_slice(br#"{"a":2}"#).expect("valid json");
    assert_ne!(a, b);
}

#[test]
fn invalid_json_is_rejected() {
    let err = CanonicalEventBytes::from_json_slice(b"not json").expect_err("invalid json should fail");
    assert!(matches!(err, CanonicalizeError::InvalidJson(_)));
}

#[test]
fn scalar_json_round_trips() {
    let a = CanonicalEventBytes::from_json_slice(b"42").expect("valid json");
    assert_eq!(a.as_bytes(), b"42");
}

#[test]
fn from_json_value_matches_from_json_slice() {
    let value: Value = serde_json::from_slice(br#"{"b":1,"a":2}"#).expect("valid json");
    let from_value = CanonicalEventBytes::from_json_value(&value).expect("valid json");
    let from_slice = CanonicalEventBytes::from_json_slice(br#"{"a":2,"b":1}"#).expect("valid json");
    assert_eq!(from_value, from_slice);
}
