use super::*;

#[test]
fn rejects_empty() {
    assert_eq!(DenialReason::from_wire("").unwrap_err(), DenialReasonError::Empty);
}

#[test]
fn rejects_over_long() {
    let s = "a".repeat(MAX_LEN + 1);
    assert_eq!(DenialReason::from_wire(&s).unwrap_err(), DenialReasonError::TooLong);
}

#[test]
fn accepts_category_string() {
    let r = DenialReason::new(DenialCategory::InvalidCredentials).unwrap();
    assert_eq!(r.as_str(), "invalid_credentials");
}

#[test]
fn display_renders_both_error_variants() {
    assert_eq!(DenialReasonError::Empty.to_string(), "denial reason must be non-empty");
    assert!(DenialReasonError::TooLong.to_string().contains("must be at most"));
}
