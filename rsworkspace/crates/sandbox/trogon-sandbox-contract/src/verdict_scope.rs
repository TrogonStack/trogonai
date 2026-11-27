use serde::{Deserialize, Serialize};

/// How long a decider's verdict lasts.
///
/// `Once` covers the asking connection, and those that asked the same question while it was pending. `Persist`
/// writes the verdict into the lease's live network policy.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictScope {
    Once,
    Persist,
}
