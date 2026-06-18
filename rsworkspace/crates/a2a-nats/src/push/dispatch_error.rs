//! Error types for the push dispatcher.
//!
//! Extracted into its own module so `dlq.rs` (and other consumers of
//! `DispatchError`) can land independently of the dispatcher loop itself.
//! The dispatcher implementation + retry helpers + idempotency-key
//! derivation land in follow-up PRs.

use std::fmt;

use crate::push::authentication_header::AuthenticationHeaderBuildError;
use crate::push::nats_push_subject::NatsPushSubject;
use crate::push::push_notification_config_id::PushNotificationConfigIdError;
use crate::push::push_notification_target::PushNotificationTargetError;
use crate::push::target::{WebhookUrl, WebhookUrlError};

#[derive(Clone, Debug)]
pub enum DispatchPrepError {
    PushConfigId(PushNotificationConfigIdError),
}

impl fmt::Display for DispatchPrepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PushConfigId(inner) => std::fmt::Display::fmt(inner, f),
        }
    }
}

impl std::error::Error for DispatchPrepError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PushConfigId(inner) => Some(inner),
        }
    }
}

#[derive(Debug)]
pub enum DispatchError {
    Prep(DispatchPrepError),
    InvalidTarget(PushNotificationTargetError),
    InvalidAuthorization(AuthenticationHeaderBuildError),
    InvalidHeader(Box<dyn std::error::Error + Send + Sync>),
    /// Boxed transport error from the webhook HTTP client. The dispatcher
    /// boxes the concrete `reqwest::Error` into this variant so this
    /// module can carry every dispatch-error shape without taking a direct
    /// dependency on the HTTP client crate.
    Http(Box<dyn std::error::Error + Send + Sync>),
    UnexpectedStatus {
        status: u16,
        url: WebhookUrl,
    },
    NatsPublish(NatsPublishDispatchError),
    JetStreamPublish(JetStreamPublishDispatchError),
}

#[derive(Debug)]
pub struct NatsPublishDispatchError {
    subject: NatsPushSubject,
    source: Box<dyn std::error::Error + Send + Sync>,
}

#[derive(Debug)]
pub struct JetStreamPublishDispatchError {
    subject: NatsPushSubject,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl NatsPublishDispatchError {
    pub fn new(subject: NatsPushSubject, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            subject,
            source: Box::new(source),
        }
    }

    pub fn subject(&self) -> &NatsPushSubject {
        &self.subject
    }
}

impl JetStreamPublishDispatchError {
    pub fn new(subject: NatsPushSubject, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            subject,
            source: Box::new(source),
        }
    }

    pub fn subject(&self) -> &NatsPushSubject {
        &self.subject
    }
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prep(inner) => std::fmt::Display::fmt(inner, f),
            Self::InvalidTarget(e) => write!(f, "invalid push notification URL: {e}"),
            Self::InvalidAuthorization(e) => write!(f, "invalid push notification authorization: {e}"),
            Self::InvalidHeader(e) => write!(f, "invalid push notification outbound header value: {e}"),
            Self::Http(e) => write!(f, "HTTP push request failed: {e}"),
            Self::UnexpectedStatus { status, url } => {
                write!(f, "push notification to {url} returned status {status}")
            }
            Self::NatsPublish(e) => write!(f, "NATS publish to {} failed: {}", e.subject, e.source),
            Self::JetStreamPublish(e) => {
                write!(f, "JetStream publish to {} failed: {}", e.subject, e.source)
            }
        }
    }
}

impl fmt::Display for NatsPublishDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "publish to {} failed: {}", self.subject, self.source)
    }
}

impl fmt::Display for JetStreamPublishDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "publish to {} failed: {}", self.subject, self.source)
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Prep(inner) => Some(inner),
            Self::InvalidTarget(e) => Some(e),
            Self::InvalidAuthorization(e) => Some(e),
            Self::InvalidHeader(e) => Some(&**e),
            Self::Http(e) => Some(&**e),
            Self::UnexpectedStatus { .. } => None,
            Self::NatsPublish(e) => Some(&*e.source),
            Self::JetStreamPublish(e) => Some(&*e.source),
        }
    }
}

impl std::error::Error for NatsPublishDispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl std::error::Error for JetStreamPublishDispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

impl From<PushNotificationTargetError> for DispatchError {
    fn from(e: PushNotificationTargetError) -> Self {
        Self::InvalidTarget(e)
    }
}

impl From<WebhookUrlError> for DispatchError {
    fn from(e: WebhookUrlError) -> Self {
        Self::InvalidTarget(PushNotificationTargetError::Http(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_err() -> PushNotificationTargetError {
        PushNotificationTargetError::UnknownScheme { raw: "ftp://x".into() }
    }

    fn config_id_err() -> PushNotificationConfigIdError {
        PushNotificationConfigId::new("").unwrap_err()
    }

    use crate::push::push_notification_config_id::PushNotificationConfigId;

    #[test]
    fn dispatch_prep_error_display_and_source() {
        use std::error::Error as _;
        let err = DispatchPrepError::PushConfigId(config_id_err());
        assert!(!err.to_string().is_empty());
        assert!(err.source().is_some());
    }

    #[test]
    fn dispatch_error_invalid_target_routes_through_from() {
        let err: DispatchError = target_err().into();
        assert!(matches!(err, DispatchError::InvalidTarget(_)));
        assert!(err.to_string().contains("invalid push notification URL"));
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
        let s = err.to_string();
        assert!(s.contains("503"));
        assert!(s.contains("https://example.com/hook"));
    }

    #[test]
    fn nats_publish_dispatch_error_round_trips_subject_and_source() {
        use std::error::Error as _;
        let inner = std::io::Error::other("nats down");
        let err = NatsPublishDispatchError::new(subject(), inner);
        assert_eq!(err.subject().as_str(), "a2a.push.t.caller.task");
        assert!(err.to_string().contains("publish to a2a.push.t.caller.task failed"));
        assert!(err.to_string().contains("nats down"));
        assert!(err.source().is_some());
    }

    #[test]
    fn jetstream_publish_dispatch_error_round_trips_subject_and_source() {
        use std::error::Error as _;
        let inner = std::io::Error::other("jetstream down");
        let err = JetStreamPublishDispatchError::new(subject(), inner);
        assert_eq!(err.subject().as_str(), "a2a.push.t.caller.task");
        assert!(err.to_string().contains("jetstream down"));
        assert!(err.source().is_some());
    }

    #[test]
    fn dispatch_error_display_and_source_cover_every_variant() {
        use std::error::Error as _;
        let prep = DispatchError::Prep(DispatchPrepError::PushConfigId(config_id_err()));
        assert!(!prep.to_string().is_empty());
        assert!(prep.source().is_some());

        let auth = DispatchError::InvalidAuthorization(AuthenticationHeaderBuildError::MissingScheme);
        assert!(auth.to_string().contains("invalid push notification authorization"));
        assert!(auth.source().is_some());

        let header_err: Box<dyn std::error::Error + Send + Sync> = "header bad".into();
        let header = DispatchError::InvalidHeader(header_err);
        assert!(header.to_string().contains("invalid push notification outbound header"));
        assert!(header.source().is_some());

        let nats = DispatchError::NatsPublish(NatsPublishDispatchError::new(subject(), std::io::Error::other("oops")));
        assert!(nats.to_string().contains("NATS publish to a2a.push.t.caller.task"));
        assert!(nats.source().is_some());

        let js = DispatchError::JetStreamPublish(JetStreamPublishDispatchError::new(
            subject(),
            std::io::Error::other("oops"),
        ));
        assert!(js.to_string().contains("JetStream publish to a2a.push.t.caller.task"));
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
        assert!(http.to_string().contains("HTTP push request failed"));
        assert!(http.to_string().contains("connect refused"));
        assert!(http.source().is_some());
    }
}
