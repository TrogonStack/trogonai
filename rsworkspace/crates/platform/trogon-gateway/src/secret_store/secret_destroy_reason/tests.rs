use super::*;

#[test]
fn exposes_the_reason_through_display_and_debug() {
    let reason = SecretDestroyReason::new("compliance request").unwrap();

    assert_eq!(reason.as_str(), "compliance request");
    assert_eq!(reason.to_string(), "compliance request");
    assert_eq!(format!("{reason:?}"), "SecretDestroyReason(\"compliance request\")");
}

#[test]
fn rejects_empty_reason() {
    assert_eq!(SecretDestroyReason::new(""), Err(SecretDestroyReasonError::Empty));
}

#[test]
fn rejects_reason_over_the_maximum_length() {
    assert_eq!(
        SecretDestroyReason::new("a".repeat(513)),
        Err(SecretDestroyReasonError::TooLong)
    );
}

#[test]
fn accepts_reason_at_the_maximum_length() {
    assert!(SecretDestroyReason::new("a".repeat(512)).is_ok());
}
