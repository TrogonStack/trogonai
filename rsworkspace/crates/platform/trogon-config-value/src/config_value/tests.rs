use super::*;

fn parse(raw: &str) -> Result<String, ConfigValueError> {
    ConfigValue::parse(raw).map(ConfigValue::into_string)
}

#[test]
fn value_without_prefix_is_used_verbatim() {
    let value = ConfigValue::parse("whsec_dGVzdC1zZWNyZXQ=").unwrap();

    assert_eq!(value.as_str(), "whsec_dGVzdC1zZWNyZXQ=");
    assert_eq!(value.secret_marker(), None);
}

#[test]
fn secret_marker_is_stripped_from_plain_value() {
    let value = ConfigValue::parse("secret:svix:whsec:whsec_dGVzdC1zZWNyZXQ=").unwrap();

    assert_eq!(
        value.secret_marker(),
        Some(&SecretMarker::new("svix", "whsec").unwrap())
    );
    assert_eq!(value.as_str(), "whsec_dGVzdC1zZWNyZXQ=");
}

#[test]
fn secret_marker_allows_empty_namespace_identifiers() {
    assert_eq!(parse("secret:::value:with:colons").unwrap(), "value:with:colons");
}

#[test]
fn secret_marker_with_base64_value_is_decoded() {
    assert_eq!(
        parse("secret:gcp:service-account:base64:eyJ0eXBlIjoic2VydmljZV9hY2NvdW50In0=").unwrap(),
        "{\"type\":\"service_account\"}"
    );
}

#[test]
fn value_not_matching_secret_marker_syntax_is_used_verbatim() {
    for raw in [
        "secret:GCP:key:value",
        "secret:gcp:key:",
        "secret:gcp",
        "secret:gcp_x:key:value",
    ] {
        let value = ConfigValue::parse(raw).unwrap();

        assert_eq!(value.as_str(), raw);
        assert_eq!(value.secret_marker(), None);
    }
}

#[test]
fn decoded_value_is_not_parsed_again() {
    // base64 of "secret:a:b:base64:aGk="
    let value = ConfigValue::parse("secret:x:y:base64:c2VjcmV0OmE6YjpiYXNlNjQ6YUdrPQ==").unwrap();

    assert_eq!(value.secret_marker(), Some(&SecretMarker::new("x", "y").unwrap()));
    assert_eq!(value.as_str(), "secret:a:b:base64:aGk=");
}

#[test]
fn base64_prefix_is_decoded() {
    assert_eq!(
        parse("base64:eyJsaW5lIjoiYVxuYiJ9Cm11bHRpCmxpbmUK").unwrap(),
        "{\"line\":\"a\\nb\"}\nmulti\nline\n"
    );
}

#[test]
fn base64_prefix_ignores_line_wrapping() {
    assert_eq!(parse("base64:aGVs\nbG8g\r\nd29y bGQ=\n").unwrap(), "hello world");
}

#[test]
fn invalid_base64_is_rejected() {
    assert_eq!(parse("base64:not*base64"), Err(ConfigValueError::InvalidBase64));
}

#[test]
fn invalid_base64_inside_secret_marker_is_rejected() {
    assert_eq!(parse("secret:x:y:base64:%%%"), Err(ConfigValueError::InvalidBase64));
}

#[test]
fn base64_of_non_utf8_is_rejected() {
    assert_eq!(parse("base64:/w=="), Err(ConfigValueError::InvalidUtf8));
}

#[test]
fn debug_redacts_secret_value() {
    let value = ConfigValue::parse("secret:x:y:hunter2").unwrap();

    assert!(!format!("{value:?}").contains("hunter2"));
}

#[test]
fn plain_value_has_no_secret_marker() {
    assert_eq!(ConfigValue::parse("plain").unwrap().secret_marker(), None);
}

#[test]
fn debug_shows_plain_value() {
    let value = ConfigValue::parse("visible").unwrap();

    assert!(format!("{value:?}").contains("visible"));
}
