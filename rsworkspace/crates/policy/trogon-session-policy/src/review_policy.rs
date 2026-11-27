use serde::{Deserialize, Serialize};

/// Review can narrow deterministic authority, never replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPolicy {
    RequiredVeto,
    DeterministicOnly,
}
