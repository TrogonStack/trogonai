use clap::Parser;

use super::*;

fn parse(extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let mut argv = vec!["trogon-sandbox-e2b-worker", "--api-key", "test"];
    argv.extend_from_slice(extra);
    WorkerArgs::try_parse_from(argv)
}

#[test]
fn defaults_apply_when_only_the_key_is_given() -> Result<(), clap::Error> {
    let args = parse(&[])?;
    assert_eq!(args.nats_url, DEFAULT_NATS_URL);
    assert_eq!(args.api_url.as_str(), "https://api.e2b.app/");
    assert_eq!(args.domain, DEFAULT_DOMAIN);
    assert_eq!(args.template, DEFAULT_TEMPLATE);
    assert_eq!(args.max_concurrent_leases, DEFAULT_MAX_CONCURRENT_LEASES);
    assert_eq!(args.http_timeout_secs, DEFAULT_HTTP_TIMEOUT_SECS);
    assert!(args.regions.is_empty());
    assert!(args.envd_url.is_none());
    assert!(args.dispatch_ledger_file.is_none());
    assert!(args.dispatch_fence_file.is_none());
    Ok(())
}

#[test]
fn regions_split_on_commas() -> Result<(), clap::Error> {
    assert_eq!(parse(&["--regions", "eu,us"])?.regions, ["eu", "us"]);
    Ok(())
}

#[test]
fn the_timeout_must_be_positive() {
    assert!(parse(&["--http-timeout-secs", "0"]).is_err());
}

#[test]
fn the_key_is_required() {
    assert!(WorkerArgs::try_parse_from(["trogon-sandbox-e2b-worker"]).is_err());
}
