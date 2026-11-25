use serde::{Deserialize, Serialize};

/// The request a filter refused, as the violation line names it: the method, the target with its query and
/// fragment cut off, and why.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RefusedRequest {
    pub method: String,
    pub target: String,
    pub reason: String,
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
}
