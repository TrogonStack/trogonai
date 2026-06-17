use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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

/// Caller identity payload carried in the JWT's `data` field. Wraps an opaque
/// JSON document but exposes the `spicedb_subject` extraction the rest of the
/// stack relies on for authorization lookups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpiceDbPrincipal(pub Value);

impl SpiceDbPrincipal {
    pub fn new(subject: impl Into<String>) -> Self {
        Self(json!({ "spicedb_subject": subject.into() }))
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
mod tests {
    use super::*;

    #[test]
    fn new_wraps_subject_in_json_object() {
        let principal = SpiceDbPrincipal::new("user:alice");
        assert_eq!(principal.0["spicedb_subject"], json!("user:alice"));
    }

    #[test]
    fn extracts_subject_when_present() {
        let principal = SpiceDbPrincipal::new("user:alice");
        assert_eq!(principal.spicedb_subject().unwrap().as_str(), "user:alice");
    }

    #[test]
    fn missing_subject_yields_none() {
        let principal = SpiceDbPrincipal(json!({ "other": "value" }));
        assert!(principal.spicedb_subject().is_none());
    }

    #[test]
    fn empty_subject_yields_none() {
        let principal = SpiceDbPrincipal(json!({ "spicedb_subject": "" }));
        assert!(principal.spicedb_subject().is_none());
    }

    #[test]
    fn non_string_subject_yields_none() {
        let principal = SpiceDbPrincipal(json!({ "spicedb_subject": 42 }));
        assert!(principal.spicedb_subject().is_none());
    }

    #[test]
    fn subject_helpers_roundtrip() {
        let subject = SpiceDbSubject::new("user:bob");
        assert_eq!(subject.as_str(), "user:bob");
    }
}
