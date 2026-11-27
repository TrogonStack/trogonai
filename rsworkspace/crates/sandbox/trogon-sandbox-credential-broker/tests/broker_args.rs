use std::error::Error;
use std::time::Duration;

use clap::Parser;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_credential_broker::{BrokerArgs, BrokerRunError, FenceMode};

const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

fn parse(extra: &[&str]) -> Result<BrokerArgs, clap::Error> {
    let mut args = vec!["broker", "--kms-key", KEY];
    args.extend_from_slice(extra);
    BrokerArgs::try_parse_from(args)
}

#[test]
fn defaults_keep_legacy_reads_and_an_optional_fence() -> Result<(), Box<dyn Error>> {
    let args = parse(&[])?;
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert!(args.allow_legacy_credentials);
    assert_eq!(args.fence_mode, FenceMode::Optional);
    assert!(!format!("{args:?}").contains(KEY));
    Ok(())
}

#[test]
fn flags_override_the_defaults() -> Result<(), Box<dyn Error>> {
    let args = parse(&[
        "--nats-url",
        "nats://example:1",
        "--allow-legacy-credentials",
        "false",
        "--fence-mode",
        "required",
    ])?;
    assert_eq!(args.nats_url, "nats://example:1");
    assert!(!args.allow_legacy_credentials);
    assert_eq!(args.fence_mode, FenceMode::Required);
    Ok(())
}

#[test]
fn an_invalid_key_or_mode_is_rejected() {
    assert!(BrokerArgs::try_parse_from(["broker", "--kms-key", "short"]).is_err());
    assert!(parse(&["--fence-mode", "strict"]).is_err());
}

#[tokio::test]
async fn connecting_to_a_closed_port_fails() -> Result<(), Box<dyn Error>> {
    let args = parse(&["--nats-url", "nats://127.0.0.1:1"])?;
    let result = tokio::time::timeout(Duration::from_secs(20), args.connect()).await?;
    assert!(matches!(result, Err(BrokerRunError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn serving_stops_when_shutdown_completes() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let args = parse(&["--nats-url", server.address()])?;
    tokio::time::timeout(Duration::from_secs(20), args.serve(std::future::ready(()))).await??;
    Ok(())
}
