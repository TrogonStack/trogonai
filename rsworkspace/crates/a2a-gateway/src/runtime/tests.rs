use trogon_std::env::InMemoryEnv;

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn run_with_args_resolves_config_and_returns_ok() {
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "a2a".to_string(),
        queue_group: None,
    };
    run_with_args(args, &env).await.expect("bootstrap config seam");
}

#[tokio::test(flavor = "current_thread")]
async fn run_with_args_surfaces_config_error() {
    let env = InMemoryEnv::new();
    let args = Args {
        nats_url: "localhost:4222".to_string(),
        prefix: "bad prefix!".to_string(),
        queue_group: None,
    };
    let err = run_with_args(args, &env).await.unwrap_err();
    assert!(matches!(err, RuntimeError::Config(ConfigError::InvalidPrefix(_))));
}
