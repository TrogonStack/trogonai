mod config;

use std::error::Error;
use std::io;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::{error, info};
use trogon_std::{ReadEnv, env::SystemEnv, fs::SystemFs, signal::shutdown_signal};
use trogon_telemetry::{ResourceAttribute, ServiceName};

use config::GatewayCliConfig;

type BoxError = Box<dyn Error + Send + Sync>;

fn config_err_box(msg: String) -> BoxError {
    Box::new(io::Error::other(msg))
}

async fn build_permission_checker<E: trogon_std::env::ReadEnv>(
    env: &E,
) -> Result<Arc<dyn trogon_mcp_gateway::authz::PermissionChecker>, BoxError> {
    let Some(sb) = config::spicedb_connect_config(env).map_err(config_err_box)? else {
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
        config_err_box(format!(
            "failed to connect to SpiceDB ({e}); check MCP_GATEWAY_SPICEDB_*"
        ))
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
            check_zed_token_cache: Arc::new(Mutex::new(None)),
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

    let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> = build_permission_checker(&SystemEnv).await?;

    let nats_connect_timeout = mcp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = Arc::new(mcp_nats::nats::connect(mcp.nats(), nats_connect_timeout).await?);

    let mesh_gateway_cfg =
        trogon_mcp_gateway::ingress::MeshGatewayConfig::from_env(&SystemEnv);
    let registry_subject = SystemEnv
        .var("MCP_GATEWAY_REGISTRY_SUBJECT")
        .unwrap_or_else(|_| trogon_sts::DEFAULT_REGISTRY_SUBJECT.to_string());
    let registry_client = trogon_sts::registry::ResilientRegistry::new(
        trogon_sts::registry::NatsRegistryClient::new((*nats_client).clone(), registry_subject),
        trogon_sts::circuit_breaker::CircuitBreaker::default(),
    );
    let registry_cache = trogon_sts::cache::RegistryCache::new(registry_client);
    let chain_resolver = if mesh_gateway_cfg.chain_resolution_mode
        == trogon_sts::chain_resolution::ChainResolutionMode::Off
    {
        None
    } else {
        Some(trogon_mcp_gateway::ingress::chain_resolver_boxed(
            registry_cache,
            mesh_gateway_cfg.chain_resolution_mode,
        ))
    };

    let egress = if jwt.agent_identity_mode() == trogon_mcp_gateway::agent_identity::AgentIdentityMode::Off {
        None
    } else {
        let egress_cfg =
            trogon_mcp_gateway::egress::EgressMintConfig::from_env(&SystemEnv).map_err(config_err_box)?;
        let sts_cfg = trogon_sts_client::StsClientConfig::from_env(&SystemEnv);
        let sts = trogon_sts_client::StsClient::from_arc(nats_client.clone(), sts_cfg);
        Some(trogon_mcp_gateway::egress::EgressMinter::from_parts(
            sts,
            egress_cfg,
            Some(nats_client.clone()),
        ))
    };

    info!(
        jwt_mode = ?jwt.mode(),
        jwt_strip_legacy_tenant_header_to_backend = jwt.jwt_controls_transport(),
        mesh_egress = egress.is_some(),
        "MCP gateway verified identity"
    );

    let traces = trogon_mcp_gateway::trace::TraceStore::default();

    let settings = trogon_mcp_gateway::gateway::GatewaySettings {
        queue_group,
        audit_stream_name,
        init_audit_stream,
        mcp,
        jwt,
        egress,
        chain_resolver,
        rate_limit: None,
        approval_gate: None,
        mesh_config: trogon_mcp_gateway::policy::MeshGatewayConfig::default(),
        context_throttle: None,
        anomaly_emitter: None,
    };

    info!(
        mcp_prefix = %settings.mcp.prefix_str(),
        audit_stream = %settings.audit_stream_name,
        "MCP NATS gateway starting"
    );

    let result = trogon_mcp_gateway::run(nats_client, checker, traces, settings, shutdown_signal()).await;

    match &result {
        Ok(()) => info!("MCP NATS gateway stopped"),
        Err(e) => error!(error = %e.0, "MCP NATS gateway stopped with error"),
    }

    if let Err(e) = trogon_telemetry::shutdown_otel() {
        error!(error = %e, "OpenTelemetry shutdown failed");
    }

    result.map_err(|e| Box::new(e) as BoxError)
}
