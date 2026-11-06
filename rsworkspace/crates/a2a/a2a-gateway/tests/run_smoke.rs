//! Smoke tests for the gateway's public startup entrypoints.

use a2a_gateway::config::ConfigError;
use a2a_gateway::ingress_identity::CallerJwtIssuerKeysError;
use a2a_gateway::runtime::run_with_args;
use a2a_gateway::{Args, RuntimeError, run};
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::InMemoryEnv;

#[tokio::test(flavor = "current_thread")]
async fn run_rejects_invalid_prefix() {
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "bad prefix!".to_string(),
        queue_group: None,
    };

    assert!(matches!(
        run(args).await,
        Err(RuntimeError::Config(ConfigError::InvalidPrefix(_)))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn run_rejects_missing_issuer_keys() {
    // Startup connects to NATS before loading the caller JWT issuer keys.
    let server = CoreTestServer::start().await;
    let env = InMemoryEnv::new();
    env.set("NATS_TLS_DISABLED", "true");
    let args = Args {
        nats_url: server.address().to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };

    assert!(matches!(
        run_with_args(args, &env).await,
        Err(RuntimeError::CallerJwtIssuerKeys(CallerJwtIssuerKeysError::Missing))
    ));
}
