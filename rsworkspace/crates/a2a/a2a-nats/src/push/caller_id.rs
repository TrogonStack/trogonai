use std::fmt;

use a2a_identity_types::{CallerId as ValidatedCallerId, SpiceDbPrincipal};
use tracing::warn;

use crate::constants::DEFAULT_PUSH_DLQ_CALLER_SEGMENT;

// `CallerId::from_user_jwt_claims` lives with the `a2a-auth-callout` crate
// that owns the `UserJwtClaims` struct — it's a thin convenience wrapper over
// `from_principal(&claims.data)` and lands in the auth-callout PR alongside
// the minted-JWT integration tests.

/// Push DLQ `{caller_id}` subject segment, built with an injective
/// percent-encoding so distinct `spicedb_subject` values can never collapse
/// onto the same segment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallerId(String);

/// Characters forbidden in a `a2a_identity_types::CallerId` segment, plus the
/// escape character itself so the encoding stays injective.
fn needs_percent_encoding(c: char) -> bool {
    matches!(c, '.' | '*' | '>' | '%') || c.is_whitespace() || matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}')
}

fn percent_encode_caller_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut utf8_buf = [0u8; 4];
    for c in raw.chars() {
        if needs_percent_encoding(c) {
            for byte in c.encode_utf8(&mut utf8_buf).as_bytes() {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

impl CallerId {
    fn from_raw_subject(raw: &str) -> Self {
        ValidatedCallerId::new(percent_encode_caller_segment(raw.trim()))
            .map_or_else(|_| Self::default(), |id| Self(id.as_str().to_owned()))
    }

    pub fn from_principal(principal: &SpiceDbPrincipal) -> Self {
        match principal.spicedb_subject() {
            Some(subject) => Self::from_raw_subject(subject.as_str()),
            None => Self::default(),
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
    match p.spicedb_subject() {
        Some(s) if !s.as_str().trim().is_empty() => CallerId::from_raw_subject(s.as_str()),
        _ => {
            warn!(%fallback, "push DLQ caller_id: principal present but spicedb_subject absent/blank; using fallback segment");
            fallback.clone()
        }
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
        Self::from_raw_subject(s)
    }
}

#[cfg(test)]
mod tests;
