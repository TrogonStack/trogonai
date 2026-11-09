use super::fixtures::{FAKE_EXTERNAL_ACCOUNT, FAKE_SERVICE_ACCOUNT};
use super::*;

#[test]
fn parses_service_account_document() {
    let credentials = GoogleCredentials::parse(FAKE_SERVICE_ACCOUNT).unwrap();
    assert_eq!(credentials.kind(), GoogleCredentialsKind::ServiceAccount);
}

#[test]
fn parses_external_account_document() {
    let credentials = GoogleCredentials::parse(FAKE_EXTERNAL_ACCOUNT).unwrap();
    assert_eq!(credentials.kind(), GoogleCredentialsKind::ExternalAccount);
}

#[test]
fn surrounding_whitespace_is_ignored() {
    let padded = format!("\n  {FAKE_SERVICE_ACCOUNT}  \n");
    assert!(GoogleCredentials::parse(padded).is_ok());
}

#[tokio::test]
async fn builds_credentials_inside_a_runtime_for_both_types() {
    for document in [FAKE_SERVICE_ACCOUNT, FAKE_EXTERNAL_ACCOUNT] {
        let credentials = GoogleCredentials::parse(document).unwrap();
        credentials.build().unwrap();
        credentials.build().unwrap();
    }
}

#[test]
fn rejects_empty_document() {
    assert!(matches!(
        GoogleCredentials::parse("  \n").unwrap_err(),
        GoogleCredentialsError::Empty
    ));
}

#[test]
fn rejects_invalid_json() {
    assert!(matches!(
        GoogleCredentials::parse("{ not json").unwrap_err(),
        GoogleCredentialsError::InvalidJson(_)
    ));
}

#[test]
fn invalid_json_keeps_the_parser_error() {
    let error = GoogleCredentials::parse(r#"{"private_key": leaky-marker}"#).unwrap_err();
    let source = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<serde_json::Error>())
        .expect("parser error is kept");

    assert_eq!(source.classify(), serde_json::error::Category::Syntax);
    assert_eq!((source.line(), source.column()), (1, 17));
}

#[test]
fn rejects_non_object_json() {
    assert!(matches!(
        GoogleCredentials::parse("[1, 2]").unwrap_err(),
        GoogleCredentialsError::NotAnObject
    ));
}

#[test]
fn rejects_missing_or_non_string_type() {
    for document in [r#"{"client_email":"a"}"#, r#"{"type":7}"#] {
        assert!(matches!(
            GoogleCredentials::parse(document).unwrap_err(),
            GoogleCredentialsError::MissingType
        ));
    }
}

#[test]
fn rejects_authorized_user_and_unknown_types() {
    for kind in [
        "authorized_user",
        "impersonated_service_account",
        "gdch_service_account",
        "other",
    ] {
        let document = format!(r#"{{"type":"{kind}","refresh_token":"fake-refresh-token"}}"#);
        assert!(matches!(
            GoogleCredentials::parse(document).unwrap_err(),
            GoogleCredentialsError::UnsupportedType
        ));
    }
}

#[tokio::test]
async fn build_rejects_malformed_service_account() {
    let credentials = GoogleCredentials::parse(r#"{"type":"service_account","client_email":"a"}"#).unwrap();
    assert!(matches!(
        credentials.build().unwrap_err(),
        GoogleCredentialsError::MalformedServiceAccount(_)
    ));
}

#[tokio::test]
async fn build_rejects_malformed_external_account() {
    let credentials = GoogleCredentials::parse(r#"{"type":"external_account","audience":"a"}"#).unwrap();
    assert!(matches!(
        credentials.build().unwrap_err(),
        GoogleCredentialsError::MalformedExternalAccount(_)
    ));
}

#[tokio::test]
async fn build_errors_keep_the_auth_library_error() {
    for document in [
        r#"{"type":"service_account","client_email":"a"}"#,
        r#"{"type":"external_account","audience":"a"}"#,
    ] {
        let error = GoogleCredentials::parse(document).unwrap().build().unwrap_err();
        let source = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<build_errors::Error>())
            .expect("auth library error is kept");

        assert!(source.is_parsing());
    }
}

#[test]
fn debug_output_is_redacted() {
    for (document, label) in [
        (FAKE_SERVICE_ACCOUNT, "service_account"),
        (FAKE_EXTERNAL_ACCOUNT, "external_account"),
    ] {
        let rendered = format!("{:?}", GoogleCredentials::parse(document).unwrap());
        assert_eq!(rendered, format!("GoogleCredentials({label}, ****)"));
        assert!(!rendered.contains("FAKEFAKEFAKE"));
    }
}
