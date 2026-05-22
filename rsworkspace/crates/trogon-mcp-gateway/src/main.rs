mod config;

use std::error::Error;
use std::sync::Arc;

use tracing::{error, info};
use trogon_std::{
    env::SystemEnv,
    fs::SystemFs,
    signal::shutdown_signal,
};
use trogon_telemetry::{ResourceAttribute, ServiceName};

use config::GatewayCliConfig;

type BoxError = Box<dyn Error + Send + Sync>;

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let GatewayCliConfig {
        mcp,
        queue_group,
        audit_stream_name,
        init_audit_stream,
    } = config::gateway_cli_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;

    let mcp = mcp_nats::apply_timeout_overrides(mcp, &SystemEnv);
    trogon_telemetry::init_logger(
        ServiceName::TrogonMcpGateway,
        [ResourceAttribute::mcp_prefix(mcp.prefix_str())],
        &SystemEnv,
        &SystemFs,
    );

    let nats_connect_timeout = mcp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = Arc::new(mcp_nats::nats::connect(mcp.nats(), nats_connect_timeout).await?);
    let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> =
        Arc::new(trogon_mcp_gateway::authz::AllowAllPermissionChecker);
    let traces = trogon_mcp_gateway::trace::TraceStore::default();

    let settings = trogon_mcp_gateway::gateway::GatewaySettings {
        queue_group,
        audit_stream_name,
        init_audit_stream,
        mcp,
    };

    info!(
        mcp_prefix = %settings.mcp.prefix_str(),
        audit_stream = %settings.audit_stream_name,
        "MCP NATS gateway starting"
    );

    let result =
        trogon_mcp_gateway::run(nats_client, checker, traces, settings, shutdown_signal()).await;

    match &result {
        Ok(()) => info!("MCP NATS gateway stopped"),
        Err(e) => error!(error = %e.0, "MCP NATS gateway stopped with error"),
    }

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result.map_err(|e| Box::new(e) as BoxError)
}
