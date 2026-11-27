use rmcp::model::{CallToolRequestParams, CallToolResult, NumberOrString, ProgressToken, RequestMetaObject};
use rmcp::service::RunningService;
use rmcp::{RoleClient, RoleServer, ServiceExt};
use serde_json::Value;

use super::{Recorder, TestResult};
use crate::server::SandboxMcpServer;

/// A server and an MCP client joined by an in-memory pipe.
pub(crate) struct McpSession {
    client: RunningService<RoleClient, Recorder>,
    recorder: Recorder,
    _server: RunningService<RoleServer, SandboxMcpServer>,
}

impl McpSession {
    pub(crate) async fn start(server: SandboxMcpServer) -> Result<Self, Box<dyn std::error::Error>> {
        let (server_io, client_io) = tokio::io::duplex(1024 * 1024);
        let recorder = Recorder::default();
        let (served, client) = tokio::join!(server.serve(server_io), recorder.clone().serve(client_io));
        Ok(Self {
            client: client?,
            recorder,
            _server: served?,
        })
    }

    /// What the server sent as progress and logging notifications.
    pub(crate) fn recorder(&self) -> &Recorder {
        &self.recorder
    }

    /// Calls `name` with `args`, which must be a JSON object or null.
    pub(crate) async fn call(
        &self,
        name: &'static str,
        args: Value,
    ) -> Result<CallToolResult, Box<dyn std::error::Error>> {
        self.call_tracked(name, args, None).await
    }

    /// Like [`Self::call`], with a progress token so the server reports progress.
    pub(crate) async fn call_tracked(
        &self,
        name: &'static str,
        args: Value,
        token: Option<&str>,
    ) -> Result<CallToolResult, Box<dyn std::error::Error>> {
        let mut params = CallToolRequestParams::new(name);
        params.meta =
            token.map(|t| RequestMetaObject::with_progress_token(ProgressToken(NumberOrString::String(t.into()))));
        if let Value::Object(arguments) = args {
            params = params.with_arguments(arguments);
        }
        Ok(self.client.call_tool(params).await?)
    }

    pub(crate) async fn tool_names(&self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let tools = self.client.list_all_tools().await?;
        Ok(tools.into_iter().map(|tool| tool.name.to_string()).collect())
    }

    pub(crate) async fn finish(self) -> TestResult {
        self.client.cancel().await?;
        Ok(())
    }
}
