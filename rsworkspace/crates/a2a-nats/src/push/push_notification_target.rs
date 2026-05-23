use std::fmt;

use crate::push::nats_push_subject::{NatsPushSubject, NatsPushSubjectError};
use crate::push::target::{WebhookUrl, WebhookUrlError};

const SUBJECT_SCHEME_PREFIX: &str = "subject:";
const JETSTREAM_SCHEME_PREFIX: &str = "jetstream:";

/// Parsed push notification delivery target from `TaskPushNotificationConfig.url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushNotificationTarget {
    Http(WebhookUrl),
    Nats(NatsPushSubject),
    JetStream(NatsPushSubject),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PushNotificationTargetError {
    Empty,
    UnknownScheme { raw: String },
    Http(WebhookUrlError),
    Nats(NatsPushSubjectError),
    JetStream(NatsPushSubjectError),
}

impl PushNotificationTarget {
    pub fn parse(raw: impl Into<String>) -> Result<Self, PushNotificationTargetError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(PushNotificationTargetError::Empty);
        }
        if raw.starts_with("https://") || raw.starts_with("http://") {
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

impl fmt::Display for PushNotificationTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("push notification URL must not be empty"),
            Self::UnknownScheme { raw } => {
                write!(
                    f,
                    "push notification URL must start with http://, https://, {SUBJECT_SCHEME_PREFIX}, or {JETSTREAM_SCHEME_PREFIX}: {raw}"
                )
            }
            Self::Http(e) => write!(f, "{e}"),
            Self::Nats(e) => write!(f, "{e}"),
            Self::JetStream(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PushNotificationTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Empty | Self::UnknownScheme { .. } => None,
            Self::Http(e) => Some(e),
            Self::Nats(e) | Self::JetStream(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
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
        match target {
            PushNotificationTarget::Nats(subject) => {
                assert_eq!(subject.as_str(), "a2a.push.acme.caller-42.task-9");
            }
            PushNotificationTarget::Http(_) | PushNotificationTarget::JetStream(_) => {
                panic!("expected NATS subject target")
            }
        }
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
        match target {
            PushNotificationTarget::JetStream(subject) => {
                assert_eq!(subject.as_str(), "a2a.push.acme.caller-42.task-9");
            }
            _ => panic!("expected JetStream subject target"),
        }
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
        assert!(err.to_string().contains("nats://broker"));
    }
}
