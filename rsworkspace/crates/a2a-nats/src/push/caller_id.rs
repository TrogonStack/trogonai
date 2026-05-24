use std::borrow::Cow;
use std::fmt;

use a2a_auth_callout::{SpiceDbPrincipal, UserJwtClaims};
use tracing::warn;

use crate::constants::DEFAULT_PUSH_DLQ_CALLER_SEGMENT;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallerId(String);

pub(crate) fn sanitize_subject_token(raw: &str) -> Cow<'_, str> {
    const fn forbidden(c: char) -> bool {
        matches!(c, '.' | '*' | '>' | ' ' | '\t' | '\n' | '\r' | '\0'..='\x1f')
    }

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Cow::Borrowed(DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }

    if !trimmed.chars().any(forbidden) {
        Cow::Borrowed(trimmed)
    } else {
        Cow::Owned(trimmed.chars().map(|c| if forbidden(c) { '_' } else { c }).collect())
    }
}

impl CallerId {
    pub fn from_principal(principal: &SpiceDbPrincipal) -> Self {
        match principal.spicedb_subject() {
            Some(subject) => Self(sanitize_subject_token(subject.as_str()).into_owned()),
            None => Self(DEFAULT_PUSH_DLQ_CALLER_SEGMENT.to_string()),
        }
    }

    pub fn from_user_jwt_claims(claims: &UserJwtClaims) -> Self {
        Self::from_principal(&claims.data)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Resolves the push DLQ `{caller_id}` segment from an optional gateway principal.
pub fn resolve_push_dlq_caller_id(principal: Option<&SpiceDbPrincipal>, fallback: &CallerId) -> CallerId {
    match principal {
        None => fallback.clone(),
        Some(p) => match p.spicedb_subject() {
            Some(_) => CallerId::from_principal(p),
            None => {
                warn!(
                    fallback = %fallback.as_str(),
                    "push DLQ caller_id: principal present but spicedb_subject absent; using fallback segment"
                );
                fallback.clone()
            }
        },
    }
}

impl Default for CallerId {
    fn default() -> Self {
        Self(DEFAULT_PUSH_DLQ_CALLER_SEGMENT.to_string())
    }
}

impl fmt::Display for CallerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for CallerId {
    fn from(s: &str) -> Self {
        Self(sanitize_subject_token(s).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_principal_reads_spicedb_subject_and_sanitizes() {
        let p = SpiceDbPrincipal(json!({"spicedb_subject": "user/al.ice"}));
        assert_eq!(CallerId::from_principal(&p).as_str(), "user/al_ice");
    }

    #[test]
    fn from_principal_without_subject_claim_is_placeholder() {
        let p = SpiceDbPrincipal(json!({}));
        assert_eq!(CallerId::from_principal(&p).as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }

    #[test]
    fn sanitization_recovers_single_segment_from_spaces_and_dots() {
        assert_eq!(
            CallerId::from_principal(&SpiceDbPrincipal(json!({"spicedb_subject": " u1.id "}))).as_str(),
            "u1_id"
        );
    }

    #[test]
    fn from_principal_sanitizes_ascii_control_chars() {
        let p = SpiceDbPrincipal(json!({"spicedb_subject": "a\u{1}b"}));
        assert_eq!(CallerId::from_principal(&p).as_str(), "a_b");
    }

    #[test]
    fn from_str_behaves_like_sanitizer() {
        assert_eq!(CallerId::from("_").as_str(), "_");
    }

    #[test]
    fn from_user_jwt_claims_delegates_to_principal_data() {
        let claims = UserJwtClaims {
            sub: a2a_auth_callout::jwt::ExternalSubject::new("ext").unwrap(),
            aud: a2a_auth_callout::AudienceAccount::new("tenant-x"),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "p.q"})),
            caller_id: a2a_auth_callout::jwt::CallerId::new("ok").unwrap(),
        };
        assert_eq!(CallerId::from_user_jwt_claims(&claims).as_str(), "p_q");
    }

    #[test]
    fn default_matches_env_placeholder_literal() {
        assert_eq!(CallerId::default().as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }

    #[test]
    fn resolve_push_dlq_caller_id_absent_principal_uses_fallback() {
        let fallback = CallerId::from("env-seg");
        assert_eq!(
            resolve_push_dlq_caller_id(None, &fallback).as_str(),
            "env-seg"
        );
    }

    #[test]
    fn resolve_push_dlq_caller_id_with_subject_uses_sanitized_segment() {
        let p = SpiceDbPrincipal(json!({"spicedb_subject": "p.q"}));
        assert_eq!(
            resolve_push_dlq_caller_id(Some(&p), &CallerId::default()).as_str(),
            "p_q"
        );
    }

    #[test]
    fn resolve_push_dlq_caller_id_without_subject_uses_fallback() {
        let p = SpiceDbPrincipal(json!({}));
        assert_eq!(
            resolve_push_dlq_caller_id(Some(&p), &CallerId::default()).as_str(),
            DEFAULT_PUSH_DLQ_CALLER_SEGMENT
        );
    }
}
