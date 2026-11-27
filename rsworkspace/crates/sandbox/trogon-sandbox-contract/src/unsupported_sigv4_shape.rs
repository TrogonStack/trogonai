use serde::{Deserialize, Serialize};

/// A SigV4 request shape the egress proxy cannot re-sign after substituting a masked AWS credential.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedSigv4Shape {
    Streaming,
    Presigned,
    Sigv4a,
}
