use serde::{Deserialize, Serialize};

/// A capability the host lends to a guest, tagged by `kind`.
///
/// `FetchUrl` lets the guest fetch URLs from `allow_domains`, with a response body of at most `max_body_bytes` and a
/// request that lasts at most `timeout_seconds`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostImport {
    FetchUrl {
        allow_domains: Vec<String>,
        max_body_bytes: u32,
        timeout_seconds: u32,
    },
}
