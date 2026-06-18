use std::borrow::Cow;
use std::fmt;

use a2a_identity_types::SpiceDbPrincipal;
use tracing::warn;

use crate::constants::DEFAULT_PUSH_DLQ_CALLER_SEGMENT;

// `CallerId::from_user_jwt_claims` lives with the `a2a-auth-callout` crate
// that owns the `UserJwtClaims` struct — it's a thin convenience wrapper over
// `from_principal(&claims.data)` and lands in the auth-callout PR alongside
// the minted-JWT integration tests.

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

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Resolves the push DLQ `{caller_id}` segment from an optional gateway principal.
pub fn resolve_push_dlq_caller_id(principal: Option<&SpiceDbPrincipal>, fallback: &CallerId) -> CallerId {
    let Some(p) = principal else {
        return fallback.clone();
    };
    // A whitespace-only `spicedb_subject` is treated as absent — letting it
    // through to `from_principal` would silently sanitise to the
    // DEFAULT_PUSH_DLQ_CALLER_SEGMENT instead of honouring the operator's
    // configured fallback.
    let has_subject = p
        .spicedb_subject()
        .map(|s| !s.as_str().trim().is_empty())
        .unwrap_or(false);
    if has_subject {
        CallerId::from_principal(p)
    } else {
        warn!(%fallback, "push DLQ caller_id: principal present but spicedb_subject absent/blank; using fallback segment");
        fallback.clone()
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
    fn default_matches_env_placeholder_literal() {
        assert_eq!(CallerId::default().as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }

    #[test]
    fn resolve_push_dlq_caller_id_absent_principal_uses_fallback() {
        let fallback = CallerId::from("env-seg");
        assert_eq!(resolve_push_dlq_caller_id(None, &fallback).as_str(), "env-seg");
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

    #[test]
    fn sanitize_subject_token_blank_string_returns_default_segment() {
        assert_eq!(sanitize_subject_token("").as_ref(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
        assert_eq!(sanitize_subject_token("   ").as_ref(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }

    #[test]
    fn default_caller_id_matches_segment_constant() {
        assert_eq!(CallerId::default().to_string(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }

    #[test]
    fn resolve_push_dlq_caller_id_whitespace_only_subject_uses_fallback() {
        let fallback = CallerId::from("env-seg");
        for blank in ["   ", "\t", "\n"] {
            let p = SpiceDbPrincipal(json!({"spicedb_subject": blank}));
            assert_eq!(
                resolve_push_dlq_caller_id(Some(&p), &fallback).as_str(),
                "env-seg",
                "whitespace-only subject {blank:?} must route to the configured fallback"
            );
        }
    }
}
