use std::error::Error;

use super::*;

#[test]
fn display_connect() {
    assert!(
        AuthCalloutError::Connect("refused".into())
            .to_string()
            .contains("NATS connect failed")
    );
}

#[test]
fn display_subscribe() {
    use std::error::Error;
    let e = AuthCalloutError::Subscribe(async_nats::SubscribeError::new(
        async_nats::SubscribeErrorKind::InvalidSubject,
    ));
    assert!(e.to_string().contains("auth callout subject"));
    assert!(Error::source(&e).is_some());
}

#[test]
fn display_credential_verification() {
    let e = AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials("bad token".into()));
    assert!(e.to_string().contains("credential verification"));
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
    assert!(
        AuthCalloutError::MissingEnvVar("X")
            .to_string()
            .contains("X is not set")
    );
    assert!(
        AuthCalloutError::UnknownSigningKeySource("foo".into())
            .to_string()
            .contains("foo")
    );
    assert!(
        AuthCalloutError::VaultNotConfigured
            .to_string()
            .contains("not wired yet")
    );
}

#[test]
fn display_and_source_for_key_load_variants() {
    let io = AuthCalloutError::KeyLoadIo {
        path: std::path::PathBuf::from("/tmp/x"),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
    };
    assert!(io.to_string().contains("/tmp/x"));
    assert!(Error::source(&io).is_some());

    let bad: Vec<u8> = vec![0xff, 0xfe];
    let utf8 = AuthCalloutError::KeyLoadUtf8(std::str::from_utf8(&bad).unwrap_err());
    assert!(utf8.to_string().contains("UTF-8"));
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
    assert!(
        AuthCalloutError::Deserialize(de)
            .to_string()
            .contains("deserialize auth callout request")
    );
    let se = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
    assert!(
        AuthCalloutError::Serialize(se)
            .to_string()
            .contains("serialize auth callout response")
    );
    let reply_err = AuthCalloutError::Reply(async_nats::PublishError::new(
        async_nats::client::PublishErrorKind::InvalidSubject,
    ));
    assert!(reply_err.to_string().contains("publish auth callout reply"));
    assert!(
        AuthCalloutError::WireFormat("x".into())
            .to_string()
            .contains("wire format")
    );
    assert!(AuthCalloutError::Internal("x".into()).to_string().contains("internal"));
}
