// Same coverage split as `a2a-nats-server` / `a2a-nats-stdio`: env validation
// and error types are pure; the connect-and-serve half lives behind
// `cfg(not(coverage))` because `trogon-nats::NatsJetStreamClient` is excluded
// during coverage builds.

use a2a_identity_types::JwtError;
use a2a_nats::{A2aPrefixError, AgentIdError};

#[cfg(not(coverage))]
use std::net::SocketAddr;

#[cfg(not(coverage))]
use a2a_identity_types::MintedUserJwt;
#[cfg(not(coverage))]
use a2a_nats::client::A2aClient;
#[cfg(not(coverage))]
use a2a_nats::{A2aAgentId, A2aPrefix, Config, NatsConfig};
#[cfg(not(coverage))]
use tracing::info;
#[cfg(not(coverage))]
use trogon_nats::jetstream::NatsJetStreamClient;
#[cfg(not(coverage))]
use trogon_std::env::SystemEnv;
#[cfg(not(coverage))]
use trogon_std::signal::shutdown_signal;

#[cfg(not(coverage))]
use crate::router;

#[cfg_attr(coverage, allow(dead_code))]
const DEFAULT_BIND: &str = "0.0.0.0:8080";
#[cfg_attr(coverage, allow(dead_code))]
const ENV_HTTP_BIND: &str = "A2A_HTTP_BIND";
#[cfg_attr(coverage, allow(dead_code))]
const ENV_AGENT_ID: &str = "A2A_AGENT_ID";
const ENV_USE_GATEWAY: &str = "A2A_USE_GATEWAY";
const ENV_GATEWAY_CALLER_JWT: &str = "A2A_GATEWAY_CALLER_JWT";

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("{} environment variable is required", ENV_AGENT_ID)]
    MissingAgentId,
    #[error("{} is required when {} is enabled", ENV_GATEWAY_CALLER_JWT, ENV_USE_GATEWAY)]
    MissingGatewayCallerJwt,
    #[error("invalid gateway caller JWT")]
    InvalidGatewayCallerJwt(#[source] JwtError),
    #[error("invalid agent id")]
    InvalidAgentId(#[source] AgentIdError),
    #[error("invalid A2A prefix")]
    InvalidPrefix(#[source] A2aPrefixError),
    #[error("invalid bind address")]
    InvalidBind(#[source] std::net::AddrParseError),
    #[error("NATS connection failed")]
    NatsConnect(#[source] trogon_nats::ConnectError),
    #[error("IO error")]
    Io(#[source] std::io::Error),
}

#[cfg_attr(coverage, allow(dead_code))]
fn env_flag<E: trogon_std::env::ReadEnv>(env: &E, key: &str) -> bool {
    matches!(
        trogon_std::env::ReadEnv::var(env, key).as_deref().map(str::trim),
        Ok("1" | "true" | "TRUE" | "True" | "yes" | "YES" | "Yes" | "on" | "ON" | "On")
    )
}

#[cfg(not(coverage))]
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

    let a2a_config = Config::new(prefix.clone(), nats_config);
    let a2a_config = a2a_nats::apply_timeout_overrides(a2a_config, &env);

    let client =
        A2aClient::new(prefix, agent_id, nats_client, js_client).with_operation_timeout(a2a_config.operation_timeout());
    let client = if env_flag(&env, ENV_USE_GATEWAY) {
        let raw_jwt = trogon_std::env::ReadEnv::var(&env, ENV_GATEWAY_CALLER_JWT)
            .map_err(|_| RuntimeError::MissingGatewayCallerJwt)?;
        let caller_jwt = MintedUserJwt::new(raw_jwt).map_err(RuntimeError::InvalidGatewayCallerJwt)?;
        caller_jwt
            .ensure_fresh()
            .map_err(RuntimeError::InvalidGatewayCallerJwt)?;
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

#[cfg(coverage)]
pub async fn run() -> Result<(), RuntimeError> {
    Ok(())
}
