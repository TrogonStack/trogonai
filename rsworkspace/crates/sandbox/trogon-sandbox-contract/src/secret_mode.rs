use serde::{Deserialize, Serialize};

/// How a binding treats its credential.
///
/// `Mask` replaces the real value inside the sandbox with a stand-in that the egress proxy swaps back on matching
/// hosts. `Deny` withholds the credential from the sandbox entirely and never injects it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretMode {
    Mask,
    Deny,
}
