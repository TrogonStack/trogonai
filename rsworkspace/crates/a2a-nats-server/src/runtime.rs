use std::fmt;
use std::net::SocketAddr;

use a2a_nats::client::Client;
use a2a_nats::{A2aAgentId, A2aPrefix, A2aPrefixError, AgentIdError, Config, NatsConfig};
use tracing::info;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_std::env::SystemEnv;
use trogon_std::signal::shutdown_signal;

use crate::router;

const DEFAULT_BIND: &str = "0.0.0.0:8080";
const ENV_HTTP_BIND: &str = "A2A_HTTP_BIND";
const ENV_AGENT_ID: &str = "A2A_AGENT_ID";

#[derive(Debug)]
pub enum RuntimeError {
    MissingAgentId,
    InvalidAgentId(AgentIdError),
    InvalidPrefix(A2aPrefixError),
    InvalidBind(std::net::AddrParseError),
    NatsConnect(trogon_nats::ConnectError),
    Io(std::io::Error),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAgentId => write!(f, "A2A_AGENT_ID environment variable is required"),
            Self::InvalidAgentId(e) => write!(f, "invalid agent id: {e}"),
            Self::InvalidPrefix(e) => write!(f, "invalid A2A prefix: {e}"),
            Self::InvalidBind(e) => write!(f, "invalid bind address: {e}"),
            Self::NatsConnect(e) => write!(f, "NATS connection failed: {e}"),
            Self::Io(e) => write!(f, "IO error: {e}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidAgentId(e) => Some(e),
            Self::InvalidPrefix(e) => Some(e),
            Self::InvalidBind(e) => Some(e),
            Self::NatsConnect(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::MissingAgentId => None,
        }
    }
}

pub async fn run() -> Result<(), RuntimeError> {
    let env = SystemEnv;

    let raw_prefix = trogon_std::env::ReadEnv::var(&env, a2a_nats::ENV_A2A_PREFIX)
        .unwrap_or_else(|_| a2a_nats::DEFAULT_A2A_PREFIX.to_string());
    let prefix = A2aPrefix::new(raw_prefix).map_err(RuntimeError::InvalidPrefix)?;

    let raw_agent_id = trogon_std::env::ReadEnv::var(&env, ENV_AGENT_ID)
        .map_err(|_| RuntimeError::MissingAgentId)?;
    let agent_id = A2aAgentId::new(raw_agent_id).map_err(RuntimeError::InvalidAgentId)?;

    let bind_addr: SocketAddr = trogon_std::env::ReadEnv::var(&env, ENV_HTTP_BIND)
        .unwrap_or_else(|_| DEFAULT_BIND.to_string())
        .parse()
        .map_err(RuntimeError::InvalidBind)?;

    let nats_config = NatsConfig::from_env(&env);
    let connect_timeout = a2a_nats::nats_connect_timeout(&env);
    let nats_client = trogon_nats::connect(&nats_config, connect_timeout)
        .await
        .map_err(RuntimeError::NatsConnect)?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = NatsJetStreamClient::new(js_context);

    let a2a_config = Config::new(prefix, nats_config);
    let a2a_config = a2a_nats::apply_timeout_overrides(a2a_config, &env);

    let client = Client::new(a2a_config, agent_id, nats_client, js_client);

    let app = router::build(client);

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(RuntimeError::Io)?;

    info!(address = %bind_addr, "A2A NATS server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            shutdown_signal().await;
            info!("Shutdown signal received");
        })
        .await
        .map_err(RuntimeError::Io)
}
