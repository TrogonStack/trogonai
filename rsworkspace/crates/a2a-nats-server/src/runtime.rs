use std::fmt;
use std::net::SocketAddr;

use a2a_auth_callout::MintedUserJwt;
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
const ENV_USE_GATEWAY: &str = "A2A_USE_GATEWAY";
const ENV_GATEWAY_CALLER_JWT: &str = "A2A_GATEWAY_CALLER_JWT";

#[derive(Debug)]
pub enum RuntimeError {
    MissingAgentId,
    MissingGatewayCallerJwt,
    InvalidGatewayCallerJwt(String),
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
            Self::MissingGatewayCallerJwt => write!(
                f,
                "{ENV_GATEWAY_CALLER_JWT} is required when {ENV_USE_GATEWAY} is enabled"
            ),
            Self::InvalidGatewayCallerJwt(msg) => write!(f, "invalid gateway caller JWT: {msg}"),
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
            Self::MissingAgentId | Self::MissingGatewayCallerJwt | Self::InvalidGatewayCallerJwt(_) => None,
        }
    }
}

fn env_flag<E: trogon_std::env::ReadEnv>(env: &E, key: &str) -> bool {
    matches!(
        trogon_std::env::ReadEnv::var(env, key).as_deref().map(str::trim),
        Ok("1" | "true" | "TRUE" | "True" | "yes" | "YES" | "Yes" | "on" | "ON" | "On")
    )
}

pub async fn run() -> Result<(), RuntimeError> {
    let env = SystemEnv;

    let raw_prefix = trogon_std::env::ReadEnv::var(&env, a2a_nats::ENV_A2A_PREFIX)
        .unwrap_or_else(|_| a2a_nats::DEFAULT_A2A_PREFIX.to_string());
    let prefix = A2aPrefix::new(raw_prefix).map_err(RuntimeError::InvalidPrefix)?;

    let raw_agent_id = trogon_std::env::ReadEnv::var(&env, ENV_AGENT_ID).map_err(|_| RuntimeError::MissingAgentId)?;
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
    let client = if env_flag(&env, ENV_USE_GATEWAY) {
        let raw_jwt = trogon_std::env::ReadEnv::var(&env, ENV_GATEWAY_CALLER_JWT)
            .map_err(|_| RuntimeError::MissingGatewayCallerJwt)?;
        let caller_jwt = MintedUserJwt::new(raw_jwt);
        caller_jwt
            .ensure_fresh()
            .map_err(|e| RuntimeError::InvalidGatewayCallerJwt(e.to_string()))?;
        info!("Routing HTTP requests through a2a-gateway ingress");
        client.routing_via_gateway_ingress(caller_jwt)
    } else {
        client
    };

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
