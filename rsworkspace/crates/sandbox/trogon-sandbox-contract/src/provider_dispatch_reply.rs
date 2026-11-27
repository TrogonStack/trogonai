use serde::{Deserialize, Serialize};

use crate::ProviderLease;

/// What a worker replies to a lease dispatch: the granted lease, or why it could not grant one.
///
/// The variants are untagged and tried in order, so the reply is told apart by which fields it carries.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProviderDispatchReply {
    Granted(ProviderLease),
    LaunchFailed {
        provider_error: String,
    },
    DispatchRejected {
        decode_error: String,
    },
    /// The worker refused the lease's vendor-specific options.
    NativeRejected {
        native_error: String,
    },
}
