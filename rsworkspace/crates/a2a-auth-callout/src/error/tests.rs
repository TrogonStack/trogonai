use std::error::Error;

use super::*;

#[test]
fn display_connect() {
    assert_eq!(
        AuthCalloutError::Connect("refused".into()).to_string(),
        "NATS connect failed: refused"
    );
}

#[test]
fn display_subscribe() {
    use std::error::Error;
    let e = AuthCalloutError::Subscribe(async_nats::SubscribeError::new(
        async_nats::SubscribeErrorKind::InvalidSubject,
    ));
    assert_eq!(e.to_string(), "subscribe to auth callout subject failed");
    assert!(Error::source(&e).is_some());
}

#[test]
fn display_credential_verification() {
    let e = AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials("bad token".into()));
    assert_eq!(e.to_string(), "credential verification failed: bad token");
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn display_jwt_variant() {
    let e = AuthCalloutError::Jwt(crate::jwt::JwtError::Encode("key missing".into()));
    assert_eq!(e.to_string(), "JWT operation failed");
    assert!(Error::source(&e).is_some());
}

#[test]
fn display_missing_env_var_unknown_source_and_vault() {
    assert_eq!(
        AuthCalloutError::MissingEnvVar("X").to_string(),
        "required environment variable X is not set"
    );
    assert_eq!(
        AuthCalloutError::UnknownSigningKeySource("foo".into()).to_string(),
        "unknown AUTH_CALLOUT_SIGNING_KEY_SOURCE: foo (expected env, file, or vault)"
    );
    assert_eq!(
        AuthCalloutError::VaultNotConfigured.to_string(),
        "AUTH_CALLOUT_SIGNING_KEY_SOURCE=vault is not wired yet"
    );
}

#[test]
fn display_and_source_for_key_load_variants() {
    let io = AuthCalloutError::KeyLoadIo {
        path: std::path::PathBuf::from("/tmp/x"),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
    };
    assert_eq!(io.to_string(), "failed to read signing key at /tmp/x");
    assert!(Error::source(&io).is_some());

    let bad: Vec<u8> = vec![0xff, 0xfe];
    let utf8 = AuthCalloutError::KeyLoadUtf8(std::str::from_utf8(&bad).unwrap_err());
    assert_eq!(utf8.to_string(), "signing key file must be UTF-8 NKey seed");
    assert!(Error::source(&utf8).is_some());
}

#[test]
fn source_for_deserialize() {
    let e = AuthCalloutError::Deserialize(serde_json::from_str::<String>("x").unwrap_err());
    assert!(e.source().is_some());
}

#[test]
fn source_for_connect_is_none() {
    assert!(AuthCalloutError::Connect("x".into()).source().is_none());
}

#[test]
fn display_covers_remaining_variants() {
    let de = serde_json::from_str::<String>("x").unwrap_err();
    assert_eq!(
        AuthCalloutError::Deserialize(de).to_string(),
        "failed to deserialize auth callout request"
    );
    let se = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
    assert_eq!(
        AuthCalloutError::Serialize(se).to_string(),
        "failed to serialize auth callout response"
    );
    let reply_err = AuthCalloutError::Reply(async_nats::PublishError::new(
        async_nats::client::PublishErrorKind::InvalidSubject,
    ));
    assert_eq!(reply_err.to_string(), "failed to publish auth callout reply");
    assert_eq!(
        AuthCalloutError::WireFormat("x".into()).to_string(),
        "auth callout wire format error: x"
    );
    assert_eq!(
        AuthCalloutError::Internal("x".into()).to_string(),
        "internal error: x"
    );
}
