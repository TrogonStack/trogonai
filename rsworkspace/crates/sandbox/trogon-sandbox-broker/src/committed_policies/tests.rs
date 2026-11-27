use std::error::Error;

use clap::Parser;

use super::*;
use crate::test_support::LocalNats;

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Parser, Debug)]
struct Harness {
    #[command(flatten)]
    args: Args,
}

fn parse(args: &[&str]) -> Result<Args, clap::Error> {
    Ok(Harness::try_parse_from(std::iter::once("harness").chain(args.iter().copied()))?.args)
}

#[test]
fn commits_by_default_and_can_be_switched_off() -> TestResult {
    assert!(parse(&[])?.enabled);
    assert!(!parse(&["--commit-credential-policies", "false"])?.enabled);
    Ok(())
}

#[tokio::test]
async fn no_writer_without_a_shared_store_or_when_disabled() -> TestResult {
    let nats = LocalNats::start().await;
    let enabled = parse(&[])?;
    assert!(enabled.writer(&nats.client, false).await?.is_none());
    let disabled = parse(&["--commit-credential-policies", "false"])?;
    assert!(disabled.writer(&nats.client, true).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn writer_opens_the_bucket_when_enabled_with_a_shared_store() -> TestResult {
    let nats = LocalNats::start().await;
    assert!(parse(&[])?.writer(&nats.client, true).await?.is_some());
    Ok(())
}
