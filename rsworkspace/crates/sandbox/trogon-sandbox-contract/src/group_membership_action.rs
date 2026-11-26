use serde::{Deserialize, Serialize};

/// Whether a membership request adds a user to a group or removes it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupMembershipAction {
    Add,
    Remove,
}
