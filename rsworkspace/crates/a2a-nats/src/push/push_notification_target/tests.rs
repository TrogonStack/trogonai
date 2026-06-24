use super::*;

#[test]
fn parses_https_webhook() {
    let target = PushNotificationTarget::parse("https://example.com/hook").unwrap();
    assert!(matches!(target, PushNotificationTarget::Http(_)));
}

#[test]
fn parses_http_webhook() {
    let target = PushNotificationTarget::parse("http://localhost:8080/hook").unwrap();
    assert!(matches!(target, PushNotificationTarget::Http(_)));
}

#[test]
fn parses_nats_subject_scheme() {
    let target = PushNotificationTarget::parse("subject:a2a.push.acme.caller-42.task-9").unwrap();
    assert!(matches!(
        &target,
        PushNotificationTarget::Nats(subject) if subject.as_str() == "a2a.push.acme.caller-42.task-9"
    ));
}

#[test]
fn rejects_bare_subject_without_prefix() {
    assert!(PushNotificationTarget::parse("a2a.push.acme.caller.task").is_err());
}

#[test]
fn rejects_nats_uri_scheme() {
    assert!(PushNotificationTarget::parse("nats://example.com").is_err());
}

#[test]
fn parses_jetstream_subject_scheme() {
    let target = PushNotificationTarget::parse("jetstream:a2a.push.acme.caller-42.task-9").unwrap();
    assert!(matches!(
        &target,
        PushNotificationTarget::JetStream(subject) if subject.as_str() == "a2a.push.acme.caller-42.task-9"
    ));
}

#[test]
fn rejects_other_schemes() {
    assert!(PushNotificationTarget::parse("ftp://example.com/hook").is_err());
    assert!(PushNotificationTarget::parse("").is_err());
    assert!(PushNotificationTarget::parse("subject:").is_err());
    assert!(PushNotificationTarget::parse("jetstream:").is_err());
}

#[test]
fn display_roundtrips_nats_target() {
    let target = PushNotificationTarget::parse("subject:a2a.push.t.caller.task").unwrap();
    assert_eq!(target.to_string(), "subject:a2a.push.t.caller.task");
}

#[test]
fn display_roundtrips_jetstream_target() {
    let target = PushNotificationTarget::parse("jetstream:a2a.push.t.caller.task").unwrap();
    assert_eq!(target.to_string(), "jetstream:a2a.push.t.caller.task");
}

#[test]
fn error_display_unknown_scheme_includes_raw_value() {
    let err = PushNotificationTarget::parse("nats://broker").unwrap_err();
    assert_eq!(
        err.to_string(),
        "push notification URL must start with http://, https://, subject:, or jetstream:: nats://broker"
    );
}

#[test]
fn parses_https_uppercase_scheme() {
    let target = PushNotificationTarget::parse("HTTPS://example.com/hook").unwrap();
    assert!(matches!(target, PushNotificationTarget::Http(_)));
}

#[test]
fn parses_mixed_case_http_scheme() {
    let target = PushNotificationTarget::parse("HtTp://localhost:8080/hook").unwrap();
    assert!(matches!(target, PushNotificationTarget::Http(_)));
}

#[test]
fn display_roundtrips_http_target() {
    let target = PushNotificationTarget::parse("https://example.com/hook").unwrap();
    assert_eq!(target.to_string(), "https://example.com/hook");
}

#[test]
fn error_display_covers_every_variant() {
    use std::error::Error as _;
    let empty = PushNotificationTargetError::Empty;
    assert_eq!(
        empty.to_string(),
        "push notification URL must not be empty"
    );
    assert!(empty.source().is_none());

    let unknown = PushNotificationTarget::parse("ftp://example.com").unwrap_err();
    assert!(matches!(unknown, PushNotificationTargetError::UnknownScheme { .. }));
    assert_eq!(
        unknown.to_string(),
        "push notification URL must start with http://, https://, subject:, or jetstream:: ftp://example.com"
    );
    assert!(unknown.source().is_none());

    let http_err = WebhookUrl::new("ftp://x").unwrap_err();
    let raw = PushNotificationTargetError::Http(http_err);
    assert_eq!(
        raw.to_string(),
        "webhook URL must use http:// or https://, got ftp://: ftp://x"
    );
    assert!(raw.source().is_some());

    let nats_err = PushNotificationTarget::parse("subject:bad subject").unwrap_err();
    assert!(matches!(nats_err, PushNotificationTargetError::Nats(_)));
    assert_eq!(
        nats_err.to_string(),
        "invalid NATS push subject: subject token contains invalid character ' '"
    );
    assert!(nats_err.source().is_some());

    let js_err = PushNotificationTarget::parse("jetstream:bad subject").unwrap_err();
    assert!(matches!(js_err, PushNotificationTargetError::JetStream(_)));
    assert_eq!(
        js_err.to_string(),
        "invalid NATS push subject: subject token contains invalid character ' '"
    );
    assert!(js_err.source().is_some());
}
