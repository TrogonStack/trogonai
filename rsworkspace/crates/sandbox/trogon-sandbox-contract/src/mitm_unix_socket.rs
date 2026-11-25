use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::DomainPattern;

/// Where the proxy sends the hosts a MITM proxy listening on a unix socket should see.
///
/// It serializes as an object with `socket_path` and `domains`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct MitmUnixSocket {
    pub socket_path: PathBuf,
    pub domains: Vec<DomainPattern>,
}
