use super::*;

#[test]
fn empty_request_rejected() {
    assert!(matches!(
        RequestedAccount::new("").unwrap_err(),
        AccountResolverError::EmptyRequest
    ));
}

#[test]
fn error_into_auth_callout_error_preserves_message() {
    let err: AuthCalloutError = AccountResolverError::Unknown("x".into()).into();
    assert_eq!(err.to_string(), "requested account \"x\" not allowlisted");
}
