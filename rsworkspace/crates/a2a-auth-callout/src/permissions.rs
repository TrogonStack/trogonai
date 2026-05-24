use std::fmt;

use serde::{Deserialize, Serialize};

use crate::jwt::CallerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubjectPattern(String);

#[derive(Debug)]
pub enum SubjectPatternError {
    Empty,
    Whitespace,
}

impl fmt::Display for SubjectPatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("subject pattern must be non-empty"),
            Self::Whitespace => f.write_str("subject pattern must not contain whitespace"),
        }
    }
}

impl std::error::Error for SubjectPatternError {}

impl SubjectPattern {
    pub fn new(pattern: impl Into<String>) -> Result<Self, SubjectPatternError> {
        let s = pattern.into();
        if s.is_empty() {
            return Err(SubjectPatternError::Empty);
        }
        if s.chars().any(char::is_whitespace) {
            return Err(SubjectPatternError::Whitespace);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedPermissions {
    pub publish_allow: Vec<SubjectPattern>,
    pub subscribe_allow: Vec<SubjectPattern>,
}

impl IssuedPermissions {
    /// Phase 0 ACL template: caller can publish into the gateway namespace,
    /// receive replies on its own `_INBOX`, and read push deliveries on its
    /// own caller-scoped push subject.
    ///
    /// The exact JSON layout the NATS server expects is pinned by the
    /// auth-callout wire format (Round 3 of the build TODO); this value object
    /// captures the *intent* and is serialized into the JWT today as
    /// `nats_permissions` while the wire format is illustrative.
    pub fn default_for_caller(caller_id: &CallerId) -> Self {
        let inbox = format!("_INBOX.{}.>", caller_id.as_str());
        let push = format!("a2a.push.{}.>", caller_id.as_str());
        Self {
            publish_allow: vec![SubjectPattern::new("a2a.gateway.>").expect("static literal")],
            subscribe_allow: vec![
                SubjectPattern::new(inbox).expect("derived from validated caller_id"),
                SubjectPattern::new(push).expect("derived from validated caller_id"),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_pattern_rejects_empty() {
        assert!(matches!(
            SubjectPattern::new("").unwrap_err(),
            SubjectPatternError::Empty
        ));
    }

    #[test]
    fn subject_pattern_rejects_whitespace() {
        assert!(matches!(
            SubjectPattern::new("a b").unwrap_err(),
            SubjectPatternError::Whitespace
        ));
    }

    #[test]
    fn default_permissions_scope_publish_to_gateway() {
        let caller = CallerId::new("usr1").unwrap();
        let perms = IssuedPermissions::default_for_caller(&caller);
        assert_eq!(perms.publish_allow.len(), 1);
        assert_eq!(perms.publish_allow[0].as_str(), "a2a.gateway.>");
    }

    #[test]
    fn default_permissions_subscribe_to_caller_namespaces() {
        let caller = CallerId::new("usr1").unwrap();
        let perms = IssuedPermissions::default_for_caller(&caller);
        let subjects: Vec<&str> = perms
            .subscribe_allow
            .iter()
            .map(SubjectPattern::as_str)
            .collect();
        assert!(subjects.contains(&"_INBOX.usr1.>"));
        assert!(subjects.contains(&"a2a.push.usr1.>"));
    }
}
