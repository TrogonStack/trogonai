use serde::{Deserialize, Serialize};

use crate::LeaseError;

/// A worker's answer to a [`ModifyGroupMembershipRequest`](crate::ModifyGroupMembershipRequest),
/// tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ModifyGroupMembershipResponse {
    Applied,
    /// A required binary, such as `usermod` or `gpasswd`, or `/bin/bash` is absent from the image.
    MissingDependency {
        tool: String,
    },
    Denied(LeaseError),
}
