use std::fmt;

use trogon_nats::{DottedNatsToken, SubjectTokenViolationError};

/// Validated NATS subject for push notification delivery.
///
/// Wire form: `subject:{subject}` in [`TaskPushNotificationConfig`](a2a::types::TaskPushNotificationConfig)::url.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NatsPushSubject(DottedNatsToken);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("invalid NATS push subject: {0}")]
pub struct NatsPushSubjectError(#[source] SubjectTokenViolationError);

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

#[cfg(test)]
mod tests;
