use serde::{Deserialize, Serialize};

use crate::{GroupMembershipAction, LeaseId, SandboxGroupName, SandboxUserName};

/// Broker-to-worker request to add or remove a user's membership in a group inside a running
/// lease's sandbox. Only the worker holding `lease_id` replies.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ModifyGroupMembershipRequest {
    pub lease_id: LeaseId,
    pub username: SandboxUserName,
    pub groupname: SandboxGroupName,
    pub action: GroupMembershipAction,
}
