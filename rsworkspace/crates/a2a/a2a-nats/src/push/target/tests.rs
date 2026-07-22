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
    let WebhookUrlError::Parse { ref raw, ref reason } = parse_err else {
        panic!("expected Parse variant, got {parse_err:?}");
    };
    assert_eq!(
        parse_err.to_string(),
        format!("webhook URL is not a valid URL ({reason}): {raw}")
    );

    let scheme_err = WebhookUrl::new("ftp://bad").unwrap_err();
    let WebhookUrlError::UnsupportedScheme { ref raw, ref scheme } = scheme_err else {
        panic!("expected UnsupportedScheme variant, got {scheme_err:?}");
    };
    assert_eq!(
        scheme_err.to_string(),
        format!("webhook URL must use http:// or https://, got {scheme}://: {raw}")
    );
}

#[test]
fn as_str_returns_inner_value() {
    let url = WebhookUrl::new("https://example.com/hook").unwrap();
    assert_eq!(url.as_str(), "https://example.com/hook");
}
