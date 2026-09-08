//! Feature-specific smoke tests for the gateway's public startup entrypoints.

#[cfg(feature = "spicedb")]
use a2a_auth_callout::AuthCalloutError;
use a2a_gateway::config::ConfigError;
#[cfg(feature = "spicedb")]
use a2a_gateway::runtime::run_with_args;
use a2a_gateway::{Args, RuntimeError, run};
#[cfg(feature = "spicedb")]
use trogon_nats::test_support::CoreTestServer;
#[cfg(feature = "spicedb")]
use trogon_std::env::InMemoryEnv;

#[cfg(not(feature = "spicedb"))]
#[tokio::test(flavor = "current_thread")]
async fn run_completes_without_spicedb() -> Result<(), RuntimeError> {
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };
    run(args).await
}

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

#[cfg(feature = "spicedb")]
#[tokio::test(flavor = "current_thread")]
async fn run_rejects_missing_signing_credentials() {
    // Startup connects to NATS before loading the signing credentials.
    let server = CoreTestServer::start().await;
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: server.address().to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };

    assert!(matches!(
        run_with_args(args, &env).await,
        Err(RuntimeError::SigningKeySource(AuthCalloutError::MissingEnvVar(
            "AUTH_CALLOUT_SIGNING_SECRET"
        )))
    ));
}
