use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};

use super::SandboxMcpServer;
use crate::ack::Ack;
use crate::inputs::admin_inputs::{
    CreateGroupInput, CreateUserInput, ForkInput, GetLeaseByNameInput, GetSandboxDomainInput,
    ModifyGroupMembershipInput, NativeHandleInput, SetSnapshotTagInput, SnapshotDigestInput, SnapshotTagInput,
    SshAccessInput,
};
use crate::parse;
use crate::reply::{JsonReply, json_reply};
use rmcp::handler::server::wrapper::Json;

use crate::tool_error::ToolError;

#[tool_router(router = admin_router, vis = "pub(super)")]
impl SandboxMcpServer {
    #[tool(
        description = "Fork a running lease into a new lease from its current state. Mirrors trogon_sandbox_contract::LeaseForkRequest."
    )]
    async fn sandbox_fork(&self, params: Parameters<ForkInput>) -> JsonReply {
        self.state.require_auth()?;
        let p = params.0;
        let source = parse::lease_id(p.source_lease_id);
        let granted = self
            .client()?
            .fork(self.state.tenant().clone(), &source, p.ttl_seconds, p.idempotency_key)
            .await?;
        json_reply(&granted)
    }

    #[tool(
        description = "Create a per-sandbox Linux user inside a running lease's sandbox. Returns outcome \"created\", \"already_exists\" or \"missing_dependency\". Mirrors trogon_sandbox_contract::CreateSandboxUserRequest."
    )]
    async fn sandbox_create_user(&self, params: Parameters<CreateUserInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let username = parse::username(params.0.username)?;
        json_reply(&self.client()?.create_user(&lease_id, username).await?)
    }

    #[tool(
        description = "Create a per-sandbox Linux group inside a running lease's sandbox. Returns outcome \"created\", \"already_exists\" or \"missing_dependency\". Mirrors trogon_sandbox_contract::CreateSandboxGroupRequest."
    )]
    async fn sandbox_create_group(&self, params: Parameters<CreateGroupInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        let groupname = parse::group_name(params.0.groupname)?;
        json_reply(&self.client()?.create_group(&lease_id, groupname).await?)
    }

    #[tool(
        description = "Add or remove a sandbox user's membership in a sandbox group inside a running lease's sandbox. Mirrors trogon_sandbox_contract::ModifyGroupMembershipRequest."
    )]
    async fn sandbox_modify_group_membership(
        &self,
        params: Parameters<ModifyGroupMembershipInput>,
    ) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let p = params.0;
        let lease_id = parse::lease_id(p.lease_id);
        let username = parse::username(p.username)?;
        let groupname = parse::group_name(p.groupname)?;
        let action = parse::membership_action(p.action)?;
        self.client()?
            .modify_group_membership(&lease_id, username, groupname, action)
            .await?;
        Ok(Ack::reply())
    }

    #[tool(
        description = "Resolve a port from a running lease's exposed ports into a reachable endpoint. Mirrors trogon_sandbox_contract::GetSandboxDomainRequest."
    )]
    async fn sandbox_get_domain(&self, params: Parameters<GetSandboxDomainInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        json_reply(&self.client()?.get_sandbox_domain(&lease_id, params.0.port).await?)
    }

    #[tool(
        description = "Read-only vendor handle for a running lease: the provider's own sandbox id and, when it has one, a URL. Needs the tenant to be allowed native passthrough. Returns outcome \"unsupported\" when the provider has no such handle. Mirrors trogon_sandbox_contract::NativeHandleRequest."
    )]
    async fn sandbox_native_handle(&self, params: Parameters<NativeHandleInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        json_reply(&self.client()?.native_handle(&lease_id).await?)
    }

    #[tool(
        description = "Direct SSH access (host, port, user and, when the provider supplies one, a ready-to-run command) to a running lease, valid for about ttl_seconds. The result can carry an access token, so keep it out of logs. Needs the tenant to be allowed native passthrough. Returns outcome \"unsupported\" when the provider has no SSH endpoint of its own. Mirrors trogon_sandbox_contract::SshAccessRequest."
    )]
    async fn sandbox_ssh_access(&self, params: Parameters<SshAccessInput>) -> JsonReply {
        self.state.require_auth()?;
        let lease_id = parse::lease_id(params.0.lease_id);
        json_reply(&self.client()?.ssh_access(&lease_id, params.0.ttl_seconds).await?)
    }

    #[tool(
        description = "Look up a running lease by its name, scoped to the server's tenant. Mirrors trogon_sandbox_contract::GetLeaseByNameRequest."
    )]
    async fn sandbox_get_lease_by_name(&self, params: Parameters<GetLeaseByNameInput>) -> JsonReply {
        self.state.require_auth()?;
        let name = parse::lease_name(params.0.name)?;
        json_reply(
            &self
                .client()?
                .get_lease_by_name(self.state.tenant().clone(), name)
                .await?,
        )
    }

    #[tool(
        description = "Look up a single snapshot by digest, scoped to the server's tenant. Mirrors trogon_sandbox_contract::SnapshotGetRequest."
    )]
    async fn sandbox_snapshot_get(&self, params: Parameters<SnapshotDigestInput>) -> JsonReply {
        self.state.require_auth()?;
        let digest = parse::digest(params.0.digest)?;
        json_reply(&self.client()?.snapshot_get(self.state.tenant().clone(), digest).await?)
    }

    #[tool(
        description = "Walk a snapshot digest's lineage tree, scoped to the server's tenant. Mirrors trogon_sandbox_contract::SnapshotTreeRequest."
    )]
    async fn sandbox_snapshot_tree(&self, params: Parameters<SnapshotDigestInput>) -> JsonReply {
        self.state.require_auth()?;
        let digest = parse::digest(params.0.digest)?;
        json_reply(
            &self
                .client()?
                .snapshot_tree(self.state.tenant().clone(), digest)
                .await?,
        )
    }

    #[tool(
        description = "Point a named snapshot tag at a digest. Mirrors trogon_sandbox_contract::SetSnapshotTagRequest."
    )]
    async fn sandbox_set_snapshot_tag(&self, params: Parameters<SetSnapshotTagInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let name = parse::snapshot_tag_name(params.0.name)?;
        let digest = parse::digest(params.0.digest)?;
        self.client()?
            .set_snapshot_tag(self.state.tenant().clone(), name, digest)
            .await?;
        Ok(Ack::reply())
    }

    #[tool(
        description = "Remove a named snapshot tag. Returns outcome \"applied\" or \"not_found\". Mirrors trogon_sandbox_contract::DeleteSnapshotTagRequest."
    )]
    async fn sandbox_delete_snapshot_tag(&self, params: Parameters<SnapshotTagInput>) -> JsonReply {
        self.state.require_auth()?;
        let name = parse::snapshot_tag_name(params.0.name)?;
        json_reply(
            &self
                .client()?
                .delete_snapshot_tag(self.state.tenant().clone(), name)
                .await?,
        )
    }

    #[tool(
        description = "List the history of digests a named snapshot tag has pointed at. Mirrors trogon_sandbox_contract::SnapshotTagHistoryRequest."
    )]
    async fn sandbox_snapshot_tag_history(&self, params: Parameters<SnapshotTagInput>) -> JsonReply {
        self.state.require_auth()?;
        let name = parse::snapshot_tag_name(params.0.name)?;
        json_reply(
            &self
                .client()?
                .snapshot_tag_history(self.state.tenant().clone(), name)
                .await?,
        )
    }
}
