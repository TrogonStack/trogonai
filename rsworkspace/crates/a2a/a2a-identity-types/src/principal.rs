use serde::{Deserialize, Serialize};
use serde_json::Value;

/// SpiceDB subject string parsed out of a [`SpiceDbPrincipal`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpiceDbSubject(String);

impl SpiceDbSubject {
    pub fn new(subject: impl Into<String>) -> Self {
        Self(subject.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The one claim [`SpiceDbPrincipal`] mints on its own; an inbound principal
/// carries whatever else the issuer put in the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SpiceDbSubjectClaim {
    spicedb_subject: String,
}

/// Caller identity payload carried in the JWT's `data` field. Wraps an opaque
/// JSON document but exposes the `spicedb_subject` extraction the rest of the
/// stack relies on for authorization lookups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpiceDbPrincipal(pub Value);

impl SpiceDbPrincipal {
    pub fn new(subject: impl Into<String>) -> Self {
        let claim = SpiceDbSubjectClaim {
            spicedb_subject: subject.into(),
        };
        // The claim is a single String; serde_json::to_value cannot fail at
        // runtime, and an authorization payload must never panic.
        Self(serde_json::to_value(claim).unwrap_or(Value::Null))
    }

    pub fn spicedb_subject(&self) -> Option<SpiceDbSubject> {
        self.0
            .get("spicedb_subject")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(SpiceDbSubject::new)
    }
}

#[cfg(test)]
mod tests;
