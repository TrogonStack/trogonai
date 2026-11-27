use std::error::Error;

use clap::Parser;

use super::*;
use crate::test_support::LocalNats;

type TestResult = Result<(), Box<dyn Error>>;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("trogon-sandbox-broker").chain(args.iter().copied()))
}

#[tokio::test]
async fn unenforced_authz_uses_the_always_allow_context() -> TestResult {
    let nats = LocalNats::start().await;
    let mut cli = parse(&["--insecure-dev"])?;
    cli.apply_insecure_dev();
    let context = connect(&cli, nats.client.clone()).await?;
    assert!(!context.enforce);
    Ok(())
}

#[tokio::test]
async fn enforced_authz_requires_an_endpoint() -> TestResult {
    let nats = LocalNats::start().await;
    let cli = parse(&[])?;
    let result = connect(&cli, nats.client.clone()).await;
    assert!(matches!(result, Err(AuthSetupError::MissingEndpoint)));
    Ok(())
}

#[tokio::test]
async fn enforced_authz_requires_a_token() -> TestResult {
    let nats = LocalNats::start().await;
    let cli = parse(&["--spicedb-endpoint", "127.0.0.1:50051"])?;
    let result = connect(&cli, nats.client.clone()).await;
    assert!(matches!(result, Err(AuthSetupError::MissingToken)));
    Ok(())
}

#[tokio::test]
async fn enforced_authz_rejects_a_blank_endpoint_and_token() -> TestResult {
    let nats = LocalNats::start().await;
    let blank_endpoint = parse(&["--spicedb-endpoint", " ", "--spicedb-token", "x"])?;
    assert!(matches!(
        connect(&blank_endpoint, nats.client.clone()).await,
        Err(AuthSetupError::Endpoint(_))
    ));
    let blank_token = parse(&["--spicedb-endpoint", "127.0.0.1:50051", "--spicedb-token", " "])?;
    assert!(matches!(
        connect(&blank_token, nats.client.clone()).await,
        Err(AuthSetupError::Token(_))
    ));
    Ok(())
}
