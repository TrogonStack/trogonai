use super::*;

#[test]
fn empty_request_rejected() {
    assert!(matches!(
        RequestedAccount::new("").unwrap_err(),
        AccountResolverError::EmptyRequest
    ));
}

#[test]
fn static_resolver_allows_known_account() {
    let resolver = StaticAccountResolver::new(["tenant-acme", "tenant-foo"]);
    let resolved = resolver
        .resolve(&RequestedAccount::new("tenant-acme").unwrap())
        .unwrap();
    assert_eq!(resolved.as_str(), "tenant-acme");
}

#[test]
fn static_resolver_denies_unknown_account() {
    let resolver = StaticAccountResolver::new(["tenant-acme"]);
    let err = resolver
        .resolve(&RequestedAccount::new("tenant-evil").unwrap())
        .unwrap_err();
    assert!(matches!(err, AccountResolverError::Unknown(_)));
}

#[test]
fn error_into_auth_callout_error_preserves_message() {
    let err: AuthCalloutError = AccountResolverError::Unknown("x".into()).into();
    assert!(err.to_string().contains("\"x\""));
}
