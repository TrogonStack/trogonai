use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ForkInput {
    pub source_lease_id: String,
    pub ttl_seconds: u32,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CreateUserInput {
    pub lease_id: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct CreateGroupInput {
    pub lease_id: String,
    pub groupname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ModifyGroupMembershipInput {
    pub lease_id: String,
    pub username: String,
    pub groupname: String,
    /// Either `"add"` or `"remove"`.
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct GetSandboxDomainInput {
    pub lease_id: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct NativeHandleInput {
    pub lease_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SshAccessInput {
    pub lease_id: String,
    /// How long the access should stay valid; the provider's default when absent.
    #[serde(default)]
    pub ttl_seconds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct GetLeaseByNameInput {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SnapshotDigestInput {
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SetSnapshotTagInput {
    pub name: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub(crate) struct SnapshotTagInput {
    pub name: String,
}
