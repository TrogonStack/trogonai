mod config;

use std::error::Error;
use std::io;
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

fn config_err_box(msg: String) -> BoxError {
    Box::new(io::Error::other(msg))
}

async fn build_permission_checker<E: trogon_std::env::ReadEnv>(
    env: &E,
) -> Result<Arc<dyn trogon_mcp_gateway::authz::PermissionChecker>, BoxError> {
    let Some(sb) =
        config::spicedb_connect_config(env).map_err(config_err_box)?
    else {
        info!("MCP gateway authorization: allow-all (SpiceDB not configured)");
        return Ok(Arc::new(trogon_mcp_gateway::authz::AllowAllPermissionChecker));
    };

    info!(
        endpoint = %sb.endpoint,
        insecure = sb.insecure,
        "MCP gateway SpiceDB authorization (tools/call, resources/read)"
    );

    let mut builder = spicedb_rs_client::ClientBuilder::new(sb.endpoint);
    if let Some(token) = sb.token {
        builder = builder.with_token(token);
    }

    let client = builder.insecure(sb.insecure).connect().await.map_err(|e| {
        config_err_box(format!("failed to connect to SpiceDB ({e}); check MCP_GATEWAY_SPICEDB_*"))
    })?;

    Ok(Arc::new(trogon_mcp_gateway::spicedb::SpicedbPermissionChecker::new(
        trogon_mcp_gateway::spicedb::SpicedbCheckerRuntime {
            client,
            tool_resource_object_type: sb.tool_resource_object_type,
            resource_object_type: sb.resource_object_type,
            subject_object_type: sb.subject_object_type,
            tool_call_permission: sb.tool_call_permission,
            resource_read_permission: sb.resource_read_permission,
            anonymous_subject_object_id: sb.anonymous_subject_object_id,
        },
    )))
}

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

    let jwt_ingress = trogon_mcp_gateway::jwt::JwtIngressConfig::from_env(&SystemEnv).map_err(config_err_box)?;
    let jwt = trogon_mcp_gateway::jwt::JwtValidator::try_new(jwt_ingress).map_err(config_err_box)?;
    info!(
        jwt_mode = ?jwt.mode(),
        jwt_strip_legacy_tenant_header_to_backend = jwt.jwt_controls_transport(),
        "MCP gateway verified identity"
    );

    let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> =
        build_permission_checker(&SystemEnv).await?;


    let nats_connect_timeout = mcp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = Arc::new(mcp_nats::nats::connect(mcp.nats(), nats_connect_timeout).await?);
    let traces = trogon_mcp_gateway::trace::TraceStore::default();

    let settings = trogon_mcp_gateway::gateway::GatewaySettings {
        queue_group,
        audit_stream_name,
        init_audit_stream,
        mcp,
        jwt,
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
