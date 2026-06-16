use std::fmt;

use trogon_nats::{DottedNatsToken, SubjectTokenViolation};

/// Validated NATS subject for push notification delivery.
///
/// Wire form: `subject:{subject}` in [`TaskPushNotificationConfig`](a2a_types::TaskPushNotificationConfig)::url.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NatsPushSubject(DottedNatsToken);

#[derive(Debug, Clone, PartialEq)]
pub struct NatsPushSubjectError(SubjectTokenViolation);

impl NatsPushSubject {
    pub fn new(subject: impl AsRef<str>) -> Result<Self, NatsPushSubjectError> {
        DottedNatsToken::new(subject).map(Self).map_err(NatsPushSubjectError)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn wire_form(&self) -> String {
        format!("subject:{}", self.as_str())
    }
}

impl fmt::Display for NatsPushSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for NatsPushSubjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid NATS push subject: {}", self.0)
    }
}

impl std::error::Error for NatsPushSubjectError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_dotted_subject() {
        let subject = NatsPushSubject::new("a2a.push.acme.caller-42.task-9").unwrap();
        assert_eq!(subject.as_str(), "a2a.push.acme.caller-42.task-9");
    }

    #[test]
    fn rejects_empty() {
        assert!(NatsPushSubject::new("").is_err());
    }

    #[test]
    fn rejects_wildcards() {
        assert!(NatsPushSubject::new("a2a.push.*").is_err());
    }

    #[test]
    fn wire_form_prefixes_subject() {
        let subject = NatsPushSubject::new("a2a.push.t.caller.task").unwrap();
        assert_eq!(subject.wire_form(), "subject:a2a.push.t.caller.task");
    }

    #[test]
    fn error_display_includes_violation() {
        let err = NatsPushSubject::new("").unwrap_err();
        assert!(err.to_string().contains("invalid NATS push subject"));
    }
}
