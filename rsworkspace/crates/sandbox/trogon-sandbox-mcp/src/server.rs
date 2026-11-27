//! The MCP server: one tool per broker operation, filtered by the role policy.

mod admin_tools;
mod command_output;
mod command_tools;
mod evidence;
mod file_op_paths;
mod file_tools;
mod lease_tools;
mod lifecycle_output;
mod lifecycle_tools;
mod terminal_outcome;
mod terminal_tools;

use std::sync::Arc;

use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, IntoContents, Tool};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, tool_handler};
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{FileOp, FileOpResponse, LeaseId, SandboxUserName};

use crate::parse;
use crate::policy::{Probe, verify_no_symlinks};
use crate::reply::{JsonReply, json_reply};
use crate::server_state::ServerState;
use crate::terminal::Sessions;
use crate::tool_error::ToolError;
use file_op_paths::file_op_paths_mut;

/// The MCP server one process runs for one tenant.
#[derive(Clone)]
pub struct SandboxMcpServer {
    state: Arc<ServerState>,
    terminals: Arc<Sessions>,
    tool_router: ToolRouter<Self>,
}

impl SandboxMcpServer {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self {
            state,
            terminals: Arc::new(Sessions::default()),
            tool_router: Self::admin_router()
                + Self::command_router()
                + Self::file_router()
                + Self::lease_router()
                + Self::lifecycle_router()
                + Self::terminal_router(),
        }
    }

    /// The registered tools the server policy lets a client see.
    pub fn visible_tools(&self) -> Vec<Tool> {
        self.tool_router
            .list_all()
            .into_iter()
            .filter(|tool| self.state.policy().tools.permits(&tool.name))
            .collect()
    }

    /// Filter patterns that match none of the registered tools.
    pub fn unmatched_tool_patterns(&self) -> Vec<String> {
        let registered: Vec<String> = self
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        self.state
            .policy()
            .tools
            .unmatched_patterns(&registered)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    fn client(&self) -> Result<&SandboxClient, ToolError> {
        self.state.sandbox_client()
    }

    /// Applies the root policy to one sandbox path and returns the path to send. With no roots configured the path
    /// passes through untouched. The lexical check runs before any client is needed; the symlink check then probes
    /// the sandbox with `stat`.
    async fn confine_path(
        &self,
        lease_id: &LeaseId,
        as_user: &Option<SandboxUserName>,
        path: &str,
    ) -> Result<String, ToolError> {
        let roots = &self.state.policy().roots;
        if roots.is_empty() {
            return Ok(path.to_string());
        }
        let confined = roots.confine(path).map_err(ToolError::PathDenied)?;
        let client = self.client()?;
        verify_no_symlinks(&confined, |probe_path| async move {
            let op = FileOp::Stat { path: probe_path };
            match client.file_op(lease_id, op, as_user.clone()).await {
                Ok(FileOpResponse::Stat { stat }) => Ok(Probe::Kind(stat.kind)),
                Ok(_) => Err(ToolError::UnexpectedReply(
                    "a stat probe was answered with another reply",
                )),
                Err(trogon_sandbox_client::ClientError::FileNotFound(_)) => Ok(Probe::Missing),
                Err(e) => Err(ToolError::from(e)),
            }
        })
        .await
        .map_err(ToolError::PathDenied)?;
        Ok(confined.path)
    }

    /// Runs one structured file operation and returns the typed reply as JSON. A refusal the provider answers
    /// (unsupported, not found, failed) becomes a tool error carrying its message, while an edit whose occurrence
    /// count differs comes back as an `edit_rejected` outcome the caller can act on.
    async fn run_file_op(&self, lease_id: String, user: Option<String>, op: FileOp) -> JsonReply {
        self.state.require_auth()?;
        let as_user = parse::optional_username(user)?;
        let lease_id = LeaseId::new(lease_id);
        let mut op = op;
        for path in file_op_paths_mut(&mut op) {
            *path = self.confine_path(&lease_id, &as_user, path).await?;
        }
        let response = self.client()?.file_op(&lease_id, op, as_user).await?;
        json_reply(&response)
    }
}

#[tool_handler]
impl rmcp::ServerHandler for SandboxMcpServer {
    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        let supports_cache_hints = context
            .protocol_version()
            .is_some_and(|version| version >= rmcp::model::ProtocolVersion::V_2026_07_28);
        Ok(rmcp::model::ListToolsResult {
            result_type: Some(rmcp::model::ResultType::COMPLETE),
            tools: self.visible_tools(),
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(rmcp::model::CacheScope::Public),
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if !self.state.policy().tools.permits(&request.name) {
            let denied = ToolError::ToolDenied(request.name.to_string());
            return Ok(CallToolResult::error(denied.into_contents()).into());
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        if !self.state.policy().tools.permits(name) {
            return None;
        }
        self.tool_router.get(name).cloned()
    }

    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(evidence::advertised_capabilities()).with_instructions(format!(
            "Sandbox MCP server (tenant: {}). Exposes lease, command, file, terminal and snapshot tools backed by NATS.",
            self.state.tenant().as_str()
        ))
    }
}

#[cfg(test)]
mod tests;
