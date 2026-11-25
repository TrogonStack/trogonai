use serde::{Deserialize, Serialize};

use crate::refused_request_wire::RefusedRequestWire;

/// The request a filter refused, as the violation line names it: the method, the target with its query and
/// fragment cut off, and why.
///
/// The cut applies on construction and on deserialization, so a value read from the wire never carries a query
/// token either.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(from = "RefusedRequestWire", into = "RefusedRequestWire")]
pub struct RefusedRequest {
    method: String,
    target: String,
    reason: String,
}

impl RefusedRequest {
    /// Builds the record from the request's full `url`, keeping everything up to and including the first `?` or
    /// `#` and replacing the rest with an ellipsis, so a query token never reaches the violation.
    pub fn new(method: &str, url: &str, reason: &str) -> Self {
        let target = match url.find(['?', '#']) {
            Some(cut) => format!("{}\u{2026}", &url[..=cut]),
            None => url.to_string(),
        };
        Self {
            method: method.to_string(),
            target,
            reason: reason.to_string(),
        }
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    /// The target with its query and fragment cut off.
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl From<RefusedRequestWire> for RefusedRequest {
    fn from(wire: RefusedRequestWire) -> Self {
        Self::new(&wire.method, &wire.target, &wire.reason)
    }
}

impl From<RefusedRequest> for RefusedRequestWire {
    fn from(request: RefusedRequest) -> Self {
        Self {
            method: request.method,
            target: request.target,
            reason: request.reason,
        }
    }
}
