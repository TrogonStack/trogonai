//! Parsing of the plain tool inputs into the typed values of the contract.

use std::collections::BTreeMap;

use trogon_sandbox_contract::{
    Digest, ExecutionId, GroupMembershipAction, LeaseId, LeaseName, LeaseStatus, SandboxGroupName, SandboxTags,
    SandboxUserName, SnapshotTagName, SuspendRequestLevel, UnixSignal,
};

use crate::tool_error::ToolError;

pub(crate) fn lease_id(value: String) -> LeaseId {
    LeaseId::new(value)
}

pub(crate) fn execution_id(value: String) -> ExecutionId {
    ExecutionId::new(value)
}

pub(crate) fn tags(tags: BTreeMap<String, String>) -> Result<SandboxTags, ToolError> {
    SandboxTags::new(tags).map_err(|e| ToolError::invalid("tags", e))
}

pub(crate) fn status_filter(status: Option<String>) -> Result<Option<LeaseStatus>, ToolError> {
    status
        .map(|s| {
            serde_json::from_value(serde_json::Value::String(s)).map_err(|e| ToolError::invalid("status_filter", e))
        })
        .transpose()
}

/// The `level` of a suspend, `auto` when absent.
pub(crate) fn suspend_level(level: Option<String>) -> Result<SuspendRequestLevel, ToolError> {
    match level.as_deref() {
        None | Some("auto") => Ok(SuspendRequestLevel::Auto),
        Some("memory") => Ok(SuspendRequestLevel::Memory),
        Some("disk") => Ok(SuspendRequestLevel::Disk),
        Some(other) => Err(ToolError::InvalidChoice {
            field: "level",
            got: other.to_string(),
            expected: "auto, memory or disk",
        }),
    }
}

/// The optional `user` input of a command or file tool, parsed.
pub(crate) fn optional_username(user: Option<String>) -> Result<Option<SandboxUserName>, ToolError> {
    user.map(|u| SandboxUserName::new(u).map_err(|e| ToolError::invalid("user", e)))
        .transpose()
}

/// The `signal` of a kill, `term` when absent.
pub(crate) fn signal(signal: Option<String>) -> Result<UnixSignal, ToolError> {
    let Some(name) = signal else {
        return Ok(UnixSignal::default());
    };
    match name.to_lowercase().as_str() {
        "term" => Ok(UnixSignal::Term),
        "kill" => Ok(UnixSignal::Kill),
        "int" => Ok(UnixSignal::Int),
        "hup" => Ok(UnixSignal::Hup),
        "quit" => Ok(UnixSignal::Quit),
        "usr1" => Ok(UnixSignal::Usr1),
        "usr2" => Ok(UnixSignal::Usr2),
        _ => Err(ToolError::InvalidChoice {
            field: "signal",
            got: name,
            expected: "term, kill, int, hup, quit, usr1 or usr2",
        }),
    }
}

/// The required `username` input of an account tool.
pub(crate) fn username(value: String) -> Result<SandboxUserName, ToolError> {
    SandboxUserName::new(value).map_err(|e| ToolError::invalid("username", e))
}

/// The `groupname` input of an account tool.
pub(crate) fn group_name(value: String) -> Result<SandboxGroupName, ToolError> {
    SandboxGroupName::new(value).map_err(|e| ToolError::invalid("groupname", e))
}

/// The `action` of a group membership change.
pub(crate) fn membership_action(action: String) -> Result<GroupMembershipAction, ToolError> {
    match action.as_str() {
        "add" => Ok(GroupMembershipAction::Add),
        "remove" => Ok(GroupMembershipAction::Remove),
        _ => Err(ToolError::InvalidChoice {
            field: "action",
            got: action,
            expected: "add or remove",
        }),
    }
}

/// The `name` of a lease.
pub(crate) fn lease_name(value: String) -> Result<LeaseName, ToolError> {
    LeaseName::new(value).map_err(|e| ToolError::invalid("name", e))
}

/// The `name` of a snapshot tag.
pub(crate) fn snapshot_tag_name(value: String) -> Result<SnapshotTagName, ToolError> {
    SnapshotTagName::new(value).map_err(|e| ToolError::invalid("name", e))
}

/// The `digest` of a snapshot.
pub(crate) fn digest(value: String) -> Result<Digest, ToolError> {
    Digest::new(value).map_err(|e| ToolError::invalid("digest", e))
}

#[cfg(test)]
mod tests;
