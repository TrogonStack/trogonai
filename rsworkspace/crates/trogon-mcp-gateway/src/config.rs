use std::env;

use clap::Parser;
use mcp_nats::Config;
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

pub const ENV_MCP_GATEWAY_QUEUE_GROUP: &str = "MCP_GATEWAY_QUEUE_GROUP";
pub const ENV_MCP_GATEWAY_AUDIT_STREAM: &str = "MCP_GATEWAY_AUDIT_STREAM";
pub const ENV_MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT: &str = "MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT";

pub const ENV_MCP_GATEWAY_SPICEDB_ENDPOINT: &str = "MCP_GATEWAY_SPICEDB_ENDPOINT";
pub const ENV_MCP_GATEWAY_SPICEDB_TOKEN: &str = "MCP_GATEWAY_SPICEDB_TOKEN";
pub const ENV_MCP_GATEWAY_SPICEDB_INSECURE: &str = "MCP_GATEWAY_SPICEDB_INSECURE";
pub const ENV_MCP_GATEWAY_SPICEDB_TOOL_OBJECT_TYPE: &str = "MCP_GATEWAY_SPICEDB_TOOL_OBJECT_TYPE";
pub const ENV_MCP_GATEWAY_SPICEDB_SUBJECT_OBJECT_TYPE: &str = "MCP_GATEWAY_SPICEDB_SUBJECT_OBJECT_TYPE";
pub const ENV_MCP_GATEWAY_SPICEDB_PERMISSION: &str = "MCP_GATEWAY_SPICEDB_PERMISSION";
pub const ENV_MCP_GATEWAY_SPICEDB_RESOURCE_OBJECT_TYPE: &str = "MCP_GATEWAY_SPICEDB_RESOURCE_OBJECT_TYPE";
pub const ENV_MCP_GATEWAY_SPICEDB_READ_PERMISSION: &str = "MCP_GATEWAY_SPICEDB_READ_PERMISSION";
pub const ENV_MCP_GATEWAY_SPICEDB_ANONYMOUS_SUBJECT_ID: &str = "MCP_GATEWAY_SPICEDB_ANONYMOUS_SUBJECT_ID";

pub const DEFAULT_SPICEDB_TOOL_RESOURCE_TYPE: &str = "trogon/mcp_tool";
pub const DEFAULT_SPICEDB_RESOURCE_TYPE: &str = "trogon/mcp_resource";
pub const DEFAULT_SPICEDB_SUBJECT_TYPE: &str = "trogon/principal";
pub const DEFAULT_SPICEDB_TOOL_PERMISSION: &str = "call";
pub const DEFAULT_SPICEDB_READ_PERMISSION: &str = "read";
pub const DEFAULT_SPICEDB_ANONYMOUS_SUBJECT_ID: &str = "anonymous";
pub struct SpicedbConnectConfig {
    pub endpoint: String,
    pub token: Option<String>,
    pub insecure: bool,
    pub tool_resource_object_type: String,
    pub resource_object_type: String,
    pub subject_object_type: String,
    pub tool_call_permission: String,
    pub resource_read_permission: String,
    pub anonymous_subject_object_id: String,
}

pub fn spicedb_connect_config<E: ReadEnv>(env: &E) -> Result<Option<SpicedbConnectConfig>, String> {
    let endpoint = match env.var(ENV_MCP_GATEWAY_SPICEDB_ENDPOINT) {
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(e) => return Err(format!("reading {ENV_MCP_GATEWAY_SPICEDB_ENDPOINT}: {e}")),
        Ok(s) if s.trim().is_empty() => return Ok(None),
        Ok(s) => s,
    };

    let token = env
        .var(ENV_MCP_GATEWAY_SPICEDB_TOKEN)
        .ok()
        .filter(|t| !t.trim().is_empty());

    let insecure = env
        .var(ENV_MCP_GATEWAY_SPICEDB_INSECURE)
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);

    let tool_resource_object_type = env
        .var(ENV_MCP_GATEWAY_SPICEDB_TOOL_OBJECT_TYPE)
        .unwrap_or_else(|_| DEFAULT_SPICEDB_TOOL_RESOURCE_TYPE.to_string());

    let subject_object_type = env
        .var(ENV_MCP_GATEWAY_SPICEDB_SUBJECT_OBJECT_TYPE)
        .unwrap_or_else(|_| DEFAULT_SPICEDB_SUBJECT_TYPE.to_string());

    let tool_call_permission = env
        .var(ENV_MCP_GATEWAY_SPICEDB_PERMISSION)
        .unwrap_or_else(|_| DEFAULT_SPICEDB_TOOL_PERMISSION.to_string());

    let resource_object_type = env
        .var(ENV_MCP_GATEWAY_SPICEDB_RESOURCE_OBJECT_TYPE)
        .unwrap_or_else(|_| DEFAULT_SPICEDB_RESOURCE_TYPE.to_string());

    let resource_read_permission = env
        .var(ENV_MCP_GATEWAY_SPICEDB_READ_PERMISSION)
        .unwrap_or_else(|_| DEFAULT_SPICEDB_READ_PERMISSION.to_string());

    let anonymous_subject_object_id = env
        .var(ENV_MCP_GATEWAY_SPICEDB_ANONYMOUS_SUBJECT_ID)
        .unwrap_or_else(|_| DEFAULT_SPICEDB_ANONYMOUS_SUBJECT_ID.to_string());

    Ok(Some(SpicedbConnectConfig {
        endpoint,
        token,
        insecure,
        tool_resource_object_type,
        resource_object_type,
        subject_object_type,
        tool_call_permission,
        resource_read_permission,
        anonymous_subject_object_id,
    }))
}

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
