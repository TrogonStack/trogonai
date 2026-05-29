//! Audience shadow validation errors.

use std::fmt;

/// Errors raised before audience comparison (malformed inputs or missing claim).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudienceShadowError {
    MissingAud,
    MalformedTenant,
    MalformedServerId,
}

impl fmt::Display for AudienceShadowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAud => write!(f, "JWT aud claim is missing or empty"),
            Self::MalformedTenant => write!(f, "tenant must be non-empty and must not contain ':'"),
            Self::MalformedServerId => write!(f, "server_id must be non-empty and must not contain ':'"),
        }
    }
}

impl std::error::Error for AudienceShadowError {}
