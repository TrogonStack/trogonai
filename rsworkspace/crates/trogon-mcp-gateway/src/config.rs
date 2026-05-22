use clap::Parser;
use mcp_nats::Config;
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

pub const ENV_MCP_GATEWAY_QUEUE_GROUP: &str = "MCP_GATEWAY_QUEUE_GROUP";
pub const ENV_MCP_GATEWAY_AUDIT_STREAM: &str = "MCP_GATEWAY_AUDIT_STREAM";
pub const ENV_MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT: &str = "MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT";

#[derive(Parser)]
#[command(name = "trogon-mcp-gateway")]
#[command(about = "MCP ingress gateway on NATS (Phase 1)", long_about = None)]
pub struct Args {
    #[arg(long, default_value = "mcp-gateway")]
    queue_group: String,

    #[arg(long, default_value = "MCP_AUDIT")]
    audit_stream: String,

    #[arg(long, default_value_t = false)]
    skip_audit_stream_init: bool,
}

pub struct GatewayCliConfig {
    pub mcp: Config,
    pub queue_group: String,
    pub audit_stream_name: String,
    pub init_audit_stream: bool,
}

pub fn gateway_cli_config<P: ParseArgs<Args = Args>, E: ReadEnv>(
    parser: &P,
    env: &E,
) -> Result<GatewayCliConfig, mcp_nats::McpPrefixError> {
    let args = parser.parse_args();
    let queue_group = env
        .var(ENV_MCP_GATEWAY_QUEUE_GROUP)
        .unwrap_or_else(|_| args.queue_group.clone());
    let audit_stream_name = env
        .var(ENV_MCP_GATEWAY_AUDIT_STREAM)
        .unwrap_or_else(|_| args.audit_stream.clone());
    let skip_init_cli = args.skip_audit_stream_init;
    let skip_init_env = env
        .var(ENV_MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);

    Ok(GatewayCliConfig {
        mcp: Config::from_env(env)?,
        queue_group,
        audit_stream_name,
        init_audit_stream: !(skip_init_cli || skip_init_env),
    })
}
