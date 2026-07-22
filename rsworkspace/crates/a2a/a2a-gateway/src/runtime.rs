//! Runtime entry point.
//!
//! `run_with_args` resolves config, connects to NATS, builds the policy
//! layers, and pumps `{prefix}.gateway.>` ingress envelopes through
//! [`dispatch::dispatch_gateway_ingress`]. Sibling modules carry the
//! per-tier helpers the orchestrator composes.

use a2a_nats::NatsConfig;
use trogon_std::env::ReadEnv;

use crate::config::{Args, Config, ConfigError, config_from_args};

#[cfg(all(feature = "spicedb", not(coverage)))]
use a2a_auth_callout::signing_key_source_from_env;
#[cfg(all(feature = "spicedb", not(coverage)))]
use futures::StreamExt;
#[cfg(all(feature = "spicedb", not(coverage)))]
use tokio_util::sync::CancellationToken;
#[cfg(all(feature = "spicedb", not(coverage)))]
use tracing::{info, warn};

#[cfg(all(feature = "spicedb", not(coverage)))]
use crate::gw_ingress_stream::{
    GatewayStreamingIngressConfig, StreamingIngressGate, gateway_streaming_ingress_enabled,
};
#[cfg(all(feature = "spicedb", not(coverage)))]
use crate::jwt_caller_identity::{JwtHeaderCallerIdentitySource, gateway_caller_identity_policy, gateway_jwt_audience};
#[cfg(all(feature = "spicedb", not(coverage)))]
use crate::policy::spicedb_tier1::Tier1SpiceDbConfig;
#[cfg(all(feature = "spicedb", not(coverage)))]
use crate::policy::tier1_declarative::Tier1DeclarativeConfig;

pub mod aauth_env;
pub mod audit_publish;
#[cfg(all(feature = "spicedb", not(coverage)))]
pub mod dispatch;
pub mod env;
pub mod policy_stack;
pub mod reply;
pub mod streaming;
#[cfg(feature = "spicedb")]
pub mod tier1;
#[cfg(feature = "spicedb")]
pub mod tier1_denial;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("gateway config: {0}")]
    Config(#[from] ConfigError),

    #[cfg(all(feature = "spicedb", not(coverage)))]
    #[error("NATS connect: {0}")]
    NatsConnect(#[from] trogon_nats::ConnectError),

    #[cfg(all(feature = "spicedb", not(coverage)))]
    #[error("gateway subscribe: {0}")]
    Subscribe(String),

    #[cfg(all(feature = "spicedb", not(coverage)))]
    #[error("tier-1 SpiceDB config: {0}")]
    Tier1Config(#[from] crate::policy::spicedb_tier1::Tier1SpiceDbBuildError),

    #[cfg(all(feature = "spicedb", not(coverage)))]
    #[error("tier-1 declarative config: {0}")]
    Tier1DeclarativeConfig(#[from] crate::policy::tier1_declarative::Tier1DeclarativeBuildError),

    #[cfg(all(feature = "spicedb", not(coverage)))]
    #[error("gateway signing key source: {0}")]
    SigningKeySource(#[from] a2a_auth_callout::AuthCalloutError),

    #[cfg(all(feature = "spicedb", not(coverage)))]
    #[error("AAuth config: {0}")]
    AAuthConfig(#[from] crate::runtime::aauth_env::AAuthEnvError),
}

/// Resolve config from `args` + `env` and run the gateway. Coverage and
/// non-spicedb builds short-circuit after walking the config seam so the
/// crate still compiles and lints without the live policy stack.
pub async fn run_with_args<E: ReadEnv>(args: Args, env: &E) -> Result<(), RuntimeError> {
    let (config, nats_config) = config_from_args(args, env)?;
    run_with_config(config, nats_config, env).await
}

#[cfg(any(not(feature = "spicedb"), coverage))]
pub async fn run_with_config<E: ReadEnv>(
    _config: Config,
    _nats_config: NatsConfig,
    _env: &E,
) -> Result<(), RuntimeError> {
    // The live dispatch path is only built into the `spicedb` non-coverage
    // profile; coverage / no-spicedb builds keep the crate buildable.
    Ok(())
}

#[cfg(all(feature = "spicedb", not(coverage)))]
pub async fn run_with_config<E: ReadEnv>(config: Config, nats_config: NatsConfig, env: &E) -> Result<(), RuntimeError> {
    let connect_timeout = a2a_nats::nats_connect_timeout(env);
    let client = trogon_nats::connect(&nats_config, connect_timeout).await?;

    let policy_stack = crate::runtime::policy_stack::gateway_policy_stack_from_env(env);
    let caller_identity_policy = gateway_caller_identity_policy(env);
    let signing_key_source = signing_key_source_from_env(env)?;
    let jwt_audience = gateway_jwt_audience(env, config.a2a_prefix.as_str());
    let message_caller_identity = JwtHeaderCallerIdentitySource::new(signing_key_source, jwt_audience);
    let tier1_layer = Tier1SpiceDbConfig::from_env(env).await?;
    let tier1_declarative_layer = Tier1DeclarativeConfig::from_env(env)?;
    let aauth = aauth_env::gateway_aauth_from_env(env)?;

    let gateway_subject_string = config.gateway_subscribe_subject();
    let gateway_subject = async_nats::Subject::from(gateway_subject_string.as_str());

    let mut ingress = match &config.queue_group {
        Some(q) => client
            .queue_subscribe(gateway_subject, q.as_str().to_owned())
            .await
            .map_err(|e| RuntimeError::Subscribe(e.to_string()))?,
        None => client
            .subscribe(gateway_subject)
            .await
            .map_err(|e| RuntimeError::Subscribe(e.to_string()))?,
    };

    info!(
        prefix = %config.a2a_prefix,
        gateway_subject = %gateway_subject_string,
        queue_group = config.queue_group.as_ref().map_or("(none -- ephemeral subscriber)", |q| q.as_str()),
        servers = ?config.nats_servers,
        "gateway subscribed on ingress wildcard; routing to mapped agent RPC subjects",
    );

    let shutdown = CancellationToken::new();
    let shutdown_for_signal = shutdown.clone();
    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        shutdown_for_signal.cancel();
    });

    let streaming_ingress_config = GatewayStreamingIngressConfig::from_env(env);
    let streaming_ingress_enabled = gateway_streaming_ingress_enabled(env);
    let streaming_ingress_gate = StreamingIngressGate::new(streaming_ingress_config);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!(prefix = %config.a2a_prefix, "gateway shutdown signal received");
                break;
            }
            incoming = ingress.next() => {
                match incoming {
                    Some(msg) => {
                        crate::runtime::dispatch::dispatch_gateway_ingress(
                            &client,
                            &config,
                            tier1_layer.gate.as_ref(),
                            tier1_layer.owner_emitter.as_ref(),
                            tier1_declarative_layer.gate.as_ref(),
                            aauth.as_ref(),
                            &policy_stack,
                            &message_caller_identity,
                            caller_identity_policy,
                            streaming_ingress_enabled,
                            streaming_ingress_config,
                            &streaming_ingress_gate,
                            shutdown.clone(),
                            env,
                            msg,
                        )
                        .await;
                    }
                    None => {
                        warn!("gateway ingress NATS subscription closed");
                        break;
                    }
                }
            }
        }
    }

    drop(ingress);
    info!(prefix = %config.a2a_prefix, "A2A gateway shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests;
