use super::*;

#[test]
fn accepts_32_lowercase_hex_digits() {
    let id = TraceId::new("4bf92f3577b34da6a3ce929d0e0e4736").unwrap();
    assert_eq!(id.as_str(), "4bf92f3577b34da6a3ce929d0e0e4736");
}

#[test]
fn rejects_malformed() {
    for value in [
        "",
        "4bf92f3577b34da6a3ce929d0e0e473",
        "4BF92F3577B34DA6A3CE929D0E0E4736",
        "zbf92f3577b34da6a3ce929d0e0e4736",
        "4bf92f3577b34da6a3ce929d0e0e47366",
    ] {
        assert_eq!(TraceId::new(value), Err(TraceIdError::Malformed), "{value}");
    }
}

#[test]
fn rejects_all_zero() {
    assert_eq!(TraceId::new("0".repeat(32)), Err(TraceIdError::AllZero));
}

#[test]
fn error_messages_state_the_rule() {
    assert_eq!(
        TraceIdError::Malformed.to_string(),
        "trace id must be 32 lowercase hex digits"
    );
    assert_eq!(TraceIdError::AllZero.to_string(), "trace id must not be all zeros");
}
