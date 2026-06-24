use super::*;

#[test]
fn accepts_https() {
    assert!(WebhookUrl::new("https://example.com/hook").is_ok());
}

#[test]
fn accepts_http_for_local_testing() {
    assert!(WebhookUrl::new("http://localhost:8080/hook").is_ok());
}

#[test]
fn rejects_non_http_schemes() {
    let err = WebhookUrl::new("ftp://example.com/hook").unwrap_err();
    assert!(matches!(err, WebhookUrlError::UnsupportedScheme { .. }));
    let err = WebhookUrl::new("nats://example.com").unwrap_err();
    assert!(matches!(err, WebhookUrlError::UnsupportedScheme { .. }));
}

#[test]
fn rejects_unparseable_strings() {
    let err = WebhookUrl::new("").unwrap_err();
    assert!(matches!(err, WebhookUrlError::Parse { .. }));
    let err = WebhookUrl::new("not a url").unwrap_err();
    assert!(matches!(err, WebhookUrlError::Parse { .. }));
    let err = WebhookUrl::new("http://").unwrap_err();
    assert!(matches!(err, WebhookUrlError::Parse { .. }));
}

#[test]
fn display_roundtrips_url() {
    let url = WebhookUrl::new("https://example.com/hook").unwrap();
    assert_eq!(url.to_string(), "https://example.com/hook");
}

#[test]
fn error_display_covers_every_variant() {
    let parse_err = WebhookUrl::new("not a url").unwrap_err();
    assert!(parse_err.to_string().contains("not a valid URL"));

    let scheme_err = WebhookUrl::new("ftp://bad").unwrap_err();
    assert!(scheme_err.to_string().contains("ftp://"));
    assert!(scheme_err.to_string().contains("must use http"));
}

#[test]
fn as_str_returns_inner_value() {
    let url = WebhookUrl::new("https://example.com/hook").unwrap();
    assert_eq!(url.as_str(), "https://example.com/hook");
}
