use trogon_sandbox_contract::{EgressViolationReason, RefusedRequest};

/// Why and for which host a connection was denied, carried by
/// `ProxyOutcome::Denied` so callers can emit a structured violation.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Denial {
    pub host: Option<String>,
    pub reason: EgressViolationReason,
    pub request: Option<RefusedRequest>,
}

impl Denial {
    pub fn new(host: Option<String>, reason: EgressViolationReason) -> Self {
        Self {
            host,
            reason,
            request: None,
        }
    }

    pub fn refusing(mut self, request: RefusedRequest) -> Self {
        self.request = Some(request);
        self
    }
}
