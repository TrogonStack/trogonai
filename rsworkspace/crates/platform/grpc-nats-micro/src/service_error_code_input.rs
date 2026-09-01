//! The `Nats-Service-Error-Code` header exactly as a responder sent it
//! (ADR 0016 §3).

/// Untrusted service error code header text. Carries no guarantee that the
/// value is an integer, a known `google.rpc.Code`, or a code that may appear
/// on an error reply; [`crate::ServiceErrorCode::from_input`] is the single
/// conversion into the domain value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceErrorCodeInput(Box<str>);

impl ServiceErrorCodeInput {
    pub fn new(value: impl Into<Box<str>>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ServiceErrorCodeInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
