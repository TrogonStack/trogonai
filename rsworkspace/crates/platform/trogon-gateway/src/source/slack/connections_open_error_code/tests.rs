use super::*;

#[test]
fn credential_failures_are_permanent() {
    for code in PERMANENT_CONNECTIONS_OPEN_ERRORS {
        assert!(ConnectionsOpenErrorCode::new(*code).is_permanent(), "{code}");
    }
}

#[test]
fn transient_failures_are_not_permanent() {
    for code in ["ratelimited", "internal_error", "missing url", "unknown error"] {
        assert!(!ConnectionsOpenErrorCode::new(code).is_permanent(), "{code}");
    }
}

#[test]
fn displays_the_code_as_slack_sent_it() {
    assert_eq!(
        ConnectionsOpenErrorCode::new("invalid_auth").to_string(),
        "invalid_auth"
    );
}
