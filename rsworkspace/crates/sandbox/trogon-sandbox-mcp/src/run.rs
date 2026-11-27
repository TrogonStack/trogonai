use std::sync::Arc;

use rmcp::transport::IntoTransport;
use rmcp::{RoleServer, ServiceExt};
use trogon_sandbox_client::SandboxClient;
use trogon_std::env::ReadEnv;

use crate::auth::resolve_api_key;
use crate::config::Config;
use crate::policy::{CommandPolicy, Policy, RootPolicy, ToolFilter};
use crate::run_error::RunError;
use crate::server::SandboxMcpServer;
use crate::server_state::ServerState;

/// Builds the role policy the command line asks for.
pub(crate) fn build_policy(config: &Config) -> Result<Policy, RunError> {
    Ok(Policy {
        tools: ToolFilter::new(config.allow_tool.clone(), config.deny_tool.clone()),
        commands: CommandPolicy::new(&config.deny_command, &config.allow_command).map_err(RunError::Commands)?,
        roots: RootPolicy::new(&config.root).map_err(RunError::Roots)?,
    })
}

/// Connects to the broker. A failure leaves the server running without a client, so its tools answer with an error
/// instead of the process exiting before the MCP handshake.
pub(crate) async fn connect(nats_url: &str, api_key: Option<&str>) -> Option<Arc<SandboxClient>> {
    match SandboxClient::connect(nats_url).await {
        Ok(client) => {
            tracing::info!(nats_url, "connected to NATS");
            let client = match api_key {
                Some(key) => client.with_api_key(key.to_string()),
                None => client,
            };
            Some(Arc::new(client))
        }
        Err(error) => {
            tracing::warn!(%error, nats_url, "NATS connection failed; authenticated tool calls will fail");
            None
        }
    }
}

/// Serves the tools over `transport` until the client disconnects.
pub async fn run<T, E, A>(config: Config, env: &impl ReadEnv, transport: T) -> Result<(), RunError>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let api_key = resolve_api_key(config.api_key.clone(), env);
    if api_key.is_none() {
        tracing::warn!("no API key configured; tool calls will return Unauthenticated until one is set");
    }
    let client = connect(&config.nats_url, api_key.as_deref()).await;
    let policy = build_policy(&config)?;
    let tool_filter = !policy.tools.is_empty();
    let command_policy = !policy.commands.is_empty();
    let root_confinement = !policy.roots.is_empty();
    tracing::info!(tool_filter, command_policy, root_confinement, "server policy");
    let state = Arc::new(ServerState::new(policy, config.tenant.clone(), api_key, client));
    let handler = SandboxMcpServer::new(state);
    for pattern in handler.unmatched_tool_patterns() {
        tracing::warn!(pattern, "tool filter pattern matches no registered tool");
    }
    tracing::info!(tenant = config.tenant.as_str(), "serving MCP");
    let service = handler
        .serve(transport)
        .await
        .map_err(|error| RunError::Serve(Box::new(error)))?;
    service.waiting().await.map_err(RunError::Join)?;
    Ok(())
}

#[cfg(test)]
mod tests;
