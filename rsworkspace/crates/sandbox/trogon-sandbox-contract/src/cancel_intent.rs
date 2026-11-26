use serde::{Deserialize, Serialize};

/// What a provider cancel asks the worker holding the lease to do with it.
///
/// A worker cannot otherwise tell a stop from a delete: both end the session, but only a
/// delete may discard what the provider kept so the sandbox could be resumed.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelIntent {
    /// End the session and keep whatever makes the sandbox resumable.
    #[default]
    Release,
    /// End the session and discard everything kept for resuming it.
    Purge,
}
