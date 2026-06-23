use std::fmt;

use crate::push::nats_push_subject::{NatsPushSubject, NatsPushSubjectError};
use crate::push::target::{WebhookUrl, WebhookUrlError};

const SUBJECT_SCHEME_PREFIX: &str = "subject:";
const JETSTREAM_SCHEME_PREFIX: &str = "jetstream:";

/// Case-insensitive scheme prefix check — `WebhookUrl::new` (via `url::Url`)
/// normalises the scheme anyway, so `HTTPS://…` should reach the webhook arm
/// instead of falling through to `UnknownScheme`.
fn has_http_scheme_prefix(raw: &str) -> bool {
    let lc = raw.get(..8).unwrap_or("").to_ascii_lowercase();
    lc.starts_with("https://") || lc.starts_with("http://")
}

/// Parsed push notification delivery target from `TaskPushNotificationConfig.url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushNotificationTarget {
    Http(WebhookUrl),
    Nats(NatsPushSubject),
    JetStream(NatsPushSubject),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PushNotificationTargetError {
    #[error("push notification URL must not be empty")]
    Empty,
    #[error(
        "push notification URL must start with http://, https://, {SUBJECT_SCHEME_PREFIX}, or {JETSTREAM_SCHEME_PREFIX}: {raw}"
    )]
    UnknownScheme { raw: String },
    #[error("{0}")]
    Http(#[source] WebhookUrlError),
    #[error("{0}")]
    Nats(#[source] NatsPushSubjectError),
    #[error("{0}")]
    JetStream(#[source] NatsPushSubjectError),
}

impl PushNotificationTarget {
    pub fn parse(raw: impl Into<String>) -> Result<Self, PushNotificationTargetError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(PushNotificationTargetError::Empty);
        }
        if has_http_scheme_prefix(&raw) {
            return WebhookUrl::new(raw)
                .map(Self::Http)
                .map_err(PushNotificationTargetError::Http);
        }
        if let Some(subject) = raw.strip_prefix(SUBJECT_SCHEME_PREFIX) {
            return NatsPushSubject::new(subject)
                .map(Self::Nats)
                .map_err(PushNotificationTargetError::Nats);
        }
        if let Some(subject) = raw.strip_prefix(JETSTREAM_SCHEME_PREFIX) {
            return NatsPushSubject::new(subject)
                .map(Self::JetStream)
                .map_err(PushNotificationTargetError::JetStream);
        }
        Err(PushNotificationTargetError::UnknownScheme { raw })
    }
}

impl fmt::Display for PushNotificationTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(url) => write!(f, "{url}"),
            Self::Nats(subject) => write!(f, "subject:{subject}"),
            Self::JetStream(subject) => write!(f, "jetstream:{subject}"),
        }
    }
}

#[cfg(test)]
mod tests;
