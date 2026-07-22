use super::*;

fn auth(scheme: &str, credentials: &str) -> AuthenticationInfo {
    AuthenticationInfo {
        scheme: scheme.to_string(),
        credentials: Some(credentials.to_string()),
    }
}

#[test]
fn bearer_normalized() {
    let v = authorization_header_value(Some(&auth("Bearer", "tok")))
        .unwrap()
        .unwrap();
    assert_eq!(v, "Bearer tok");
    let v2 = authorization_header_value(Some(&auth("jwt", "signed.jwt.here")))
        .unwrap()
        .unwrap();
    assert_eq!(v2, "Bearer signed.jwt.here");
}

#[test]
fn basic_passes_through_base64_token() {
    let v = authorization_header_value(Some(&auth("basic", "dXNlcjpwdw==")))
        .unwrap()
        .unwrap();
    assert_eq!(v, "Basic dXNlcjpwdw==");
}

#[test]
fn custom_scheme_formats_token_pair() {
    let v = authorization_header_value(Some(&auth("BearerToken", "abc")))
        .unwrap()
        .unwrap();
    assert_eq!(v, "BearerToken abc");
}

#[test]
fn digest_errors() {
    let err = authorization_header_value(Some(&auth("digest", "opaque"))).unwrap_err();
    assert!(matches!(err, AuthenticationHeaderBuildError::UnsupportedScheme { .. }));
}

#[test]
fn negotiate_and_ntlm_are_rejected_as_unsupported_challenge_schemes() {
    for scheme in ["Negotiate", "ntlm", "NTLM"] {
        let err = authorization_header_value(Some(&auth(scheme, "tok"))).unwrap_err();
        assert!(
            matches!(err, AuthenticationHeaderBuildError::UnsupportedScheme { .. }),
            "{scheme} must round-trip through the unsupported-challenge arm"
        );
    }
}

#[test]
fn none_absent_when_no_authentication_msg() {
    assert!(authorization_header_value(None).unwrap().is_none());
}

#[test]
fn missing_scheme_is_rejected() {
    let err = authorization_header_value(Some(&auth("   ", "tok"))).unwrap_err();
    assert!(matches!(err, AuthenticationHeaderBuildError::MissingScheme));
}

#[test]
fn bearer_without_credentials_is_rejected() {
    let err = authorization_header_value(Some(&auth("Bearer", "   "))).unwrap_err();
    assert!(matches!(err, AuthenticationHeaderBuildError::EmptyCredentials));
}

#[test]
fn basic_without_credentials_is_rejected() {
    let err = authorization_header_value(Some(&auth("Basic", ""))).unwrap_err();
    assert!(matches!(err, AuthenticationHeaderBuildError::EmptyCredentials));
}

#[test]
fn custom_scheme_without_credentials_is_rejected() {
    let err = authorization_header_value(Some(&auth("CustomScheme", ""))).unwrap_err();
    assert!(matches!(err, AuthenticationHeaderBuildError::EmptyCredentials));
}

#[test]
fn error_display_covers_every_variant() {
    assert_eq!(
        AuthenticationHeaderBuildError::MissingScheme.to_string(),
        "push authentication.scheme must not be empty"
    );
    assert_eq!(
        AuthenticationHeaderBuildError::EmptyCredentials.to_string(),
        "push authentication.credentials required for scheme"
    );
    assert_eq!(
        AuthenticationHeaderBuildError::UnsupportedScheme {
            scheme: "Digest".into()
        }
        .to_string(),
        "push authentication scheme \"Digest\" not supported yet"
    );
}
