use acp_nats::{AcpPrefix, AcpPrefixError, Config, NatsConfig};
use clap::Parser;
use trogon_std::ParseArgs;
use trogon_std::env::ReadEnv;

#[derive(Parser, Debug, Clone)]
#[command(name = "acp-nats-stdio")]
#[command(about = "ACP stdio to NATS bridge for agent-client protocol", long_about = None)]
pub struct Args {
    #[arg(long = "acp-prefix")]
    pub acp_prefix: Option<String>,
}

pub fn base_config<P: ParseArgs<Args = Args>, E: ReadEnv>(
    parser: &P,
    env_provider: &E,
) -> Result<Config, AcpPrefixError> {
    let args = parser.parse_args();
    base_config_from_args(args, env_provider)
}

fn base_config_from_args<E: ReadEnv>(args: Args, env_provider: &E) -> Result<Config, AcpPrefixError> {
    let raw_prefix = args
        .acp_prefix
        .or_else(|| env_provider.var(acp_nats::ENV_ACP_PREFIX).ok())
        .unwrap_or_else(|| acp_nats::DEFAULT_ACP_PREFIX.to_string());
    let prefix = AcpPrefix::new(raw_prefix)?;
    Ok(Config::with_prefix(prefix, NatsConfig::from_env(env_provider)))
}

#[cfg(test)]
mod tests;
