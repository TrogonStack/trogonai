use super::*;
use std::error::Error as _;

fn target_err() -> PushNotificationTargetError {
    PushNotificationTargetError::UnknownScheme { raw: "ftp://x".into() }
}

fn config_id_err() -> PushNotificationConfigIdError {
    PushNotificationConfigId::new("").unwrap_err()
}

use crate::push::push_notification_config_id::PushNotificationConfigId;

#[test]
fn dispatch_prep_error_display_and_source() {
    let err = DispatchPrepError::PushConfigId(config_id_err());
    assert!(!err.to_string().is_empty());
    assert!(err.source().is_some());
}

#[test]
fn dispatch_error_invalid_target_routes_through_from() {
    let err: DispatchError = target_err().into();
    assert!(matches!(err, DispatchError::InvalidTarget(_)));
    assert_eq!(
        err.to_string(),
        "invalid push notification URL: push notification URL must start with http://, https://, subject:, or jetstream:: ftp://x"
    );
}

#[test]
fn dispatch_error_webhook_url_error_routes_through_from() {
    let url_err = crate::push::target::WebhookUrl::new("ftp://nope").unwrap_err();
    let err: DispatchError = url_err.into();
    assert!(matches!(
        err,
        DispatchError::InvalidTarget(PushNotificationTargetError::Http(_))
    ));
}

fn subject() -> NatsPushSubject {
    NatsPushSubject::new("a2a.push.t.caller.task").unwrap()
}

fn url() -> WebhookUrl {
    WebhookUrl::new("https://example.com/hook").unwrap()
}

#[test]
fn dispatch_error_unexpected_status_display_contains_url_and_code() {
    let err = DispatchError::UnexpectedStatus {
        status: 503,
        url: url(),
    };
    assert_eq!(
        err.to_string(),
        "push notification to https://example.com/hook returned status 503"
    );
}

#[test]
fn nats_publish_dispatch_error_round_trips_subject_and_source() {
    let inner = std::io::Error::other("nats down");
    let err = NatsPublishDispatchError::new(subject(), inner);
    assert_eq!(err.subject().as_str(), "a2a.push.t.caller.task");
    assert_eq!(
        err.to_string(),
        "NATS publish to a2a.push.t.caller.task failed: nats down"
    );
    assert!(err.source().is_some());
}

#[test]
fn jetstream_publish_dispatch_error_round_trips_subject_and_source() {
    let inner = std::io::Error::other("jetstream down");
    let err = JetStreamPublishDispatchError::new(subject(), inner);
    assert_eq!(err.subject().as_str(), "a2a.push.t.caller.task");
    assert_eq!(
        err.to_string(),
        "JetStream publish to a2a.push.t.caller.task failed: jetstream down"
    );
    assert!(err.source().is_some());
}

#[test]
fn dispatch_error_display_and_source_cover_every_variant() {
    let prep = DispatchError::Prep(DispatchPrepError::PushConfigId(config_id_err()));
    assert!(!prep.to_string().is_empty());
    assert!(prep.source().is_some());

    let auth = DispatchError::InvalidAuthorization(AuthenticationHeaderBuildError::MissingScheme);
    assert_eq!(
        auth.to_string(),
        "invalid push notification authorization: push authentication.scheme must not be empty"
    );
    assert!(auth.source().is_some());

    let header_err: Box<dyn std::error::Error + Send + Sync> = "header bad".into();
    let header = DispatchError::InvalidHeader(header_err);
    assert_eq!(
        header.to_string(),
        "invalid push notification outbound header value: header bad"
    );
    assert!(header.source().is_some());

    let nats = DispatchError::NatsPublish(NatsPublishDispatchError::new(subject(), std::io::Error::other("oops")));
    assert_eq!(nats.to_string(), "NATS publish to a2a.push.t.caller.task failed: oops");
    assert!(nats.source().is_some());

    let js = DispatchError::JetStreamPublish(JetStreamPublishDispatchError::new(
        subject(),
        std::io::Error::other("oops"),
    ));
    assert_eq!(
        js.to_string(),
        "JetStream publish to a2a.push.t.caller.task failed: oops"
    );
    assert!(js.source().is_some());

    let status = DispatchError::UnexpectedStatus {
        status: 500,
        url: url(),
    };
    assert!(status.source().is_none());

    let target = DispatchError::InvalidTarget(target_err());
    assert!(target.source().is_some());

    let http_err: Box<dyn std::error::Error + Send + Sync> = Box::new(std::io::Error::other("connect refused"));
    let http = DispatchError::Http(http_err);
    assert_eq!(http.to_string(), "HTTP push request failed: connect refused");
    assert!(http.source().is_some());
}
