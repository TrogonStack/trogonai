use serde::{Deserialize, Serialize};

/// Why a guest call was refused by protocol admission.
///
/// It is serialized as the body of a refusal response.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GuestProtocolError {
    /// The peer speaks a protocol older than this side still supports.
    #[error(
        "peer protocol version {peer_version} is below the minimum {min_supported} (this side speaks {this_version})"
    )]
    VersionTooOld {
        peer_version: u32,
        min_supported: u32,
        this_version: u32,
    },
    /// The version header was present but not an unsigned integer.
    #[error("protocol version header is not an unsigned integer")]
    VersionMalformed,
    /// The agent knows the verb and does not implement it.
    ///
    /// This is the only refusal a caller may answer by falling back to an older path.
    #[error("the agent does not implement this verb")]
    Unimplemented,
}
