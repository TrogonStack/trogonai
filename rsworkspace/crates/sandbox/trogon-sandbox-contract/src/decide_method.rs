use serde::{Deserialize, Serialize};

use crate::HttpMethod;

/// How the sandboxed client named the destination a decider is asked about.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecideMethod {
    Connect,
    Socks5,
    TlsSni,
    Http(HttpMethod),
}
