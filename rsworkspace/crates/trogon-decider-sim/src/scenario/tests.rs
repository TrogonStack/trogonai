use super::*;

fn envelope(type_: &str, payload: &[u8]) -> host::AnyEnvelope {
    host::AnyEnvelope {
        type_: type_.to_string(),
        payload: payload.to_vec(),
    }
}

#[test]
fn events_with_different_types_never_match() {
    assert!(!events_match(&envelope("a.Type", b"x"), &envelope("b.Type", b"x")));
}

#[test]
fn unknown_types_fall_back_to_byte_comparison() {
    assert!(events_match(
        &envelope("unknown.Type", b"raw"),
        &envelope("unknown.Type", b"raw")
    ));
    assert!(!events_match(
        &envelope("unknown.Type", b"raw"),
        &envelope("unknown.Type", b"different")
    ));
}
