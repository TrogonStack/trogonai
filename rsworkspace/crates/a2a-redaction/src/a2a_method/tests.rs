use super::*;

#[test]
fn roundtrip_display_parse() {
    for method in A2aMethod::all() {
        let parsed = method.as_str().parse::<A2aMethod>().unwrap();
        assert_eq!(parsed, *method);
    }
}

#[test]
fn serde_uses_canonical_wire_strings() {
    let method = A2aMethod::MessageSend;
    let wire = serde_json::to_string(&method).expect("serialize");
    assert_eq!(wire, "\"message/send\"");
    let back: A2aMethod = serde_json::from_str(&wire).expect("deserialize");
    assert_eq!(back, method);
}

#[test]
fn parse_error_includes_unknown_value() {
    let err = "not/real".parse::<A2aMethod>().unwrap_err();
    assert_eq!(err.value, "not/real");
}
