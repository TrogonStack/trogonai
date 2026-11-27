use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// How to reach a published port when a bare `host:port` is not enough.
///
/// An example is a provider that fronts the port with an HTTPS URL and requires a token header. `headers` may carry
/// a secret and belongs to the lease's tenant only, so `Debug` shows the header names and never their values.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PortAccess {
    /// Base URL of the port, scheme included.
    pub base_url: String,
    /// Headers every request to `base_url` must carry.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

impl fmt::Debug for PortAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: BTreeMap<&String, &str> = self.headers.keys().map(|name| (name, "[REDACTED]")).collect();
        formatter
            .debug_struct("PortAccess")
            .field("base_url", &self.base_url)
            .field("headers", &headers)
            .finish()
    }
}
