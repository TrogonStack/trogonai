use clap::Parser;
use clap::error::ErrorKind;

use super::*;

fn parse(extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let key = uuid::Uuid::new_v4().simple().to_string();
    let mut argv = vec!["trogon-sandbox-daytona-worker", "--api-key", key.as_str()];
    argv.extend_from_slice(extra);
    WorkerArgs::try_parse_from(argv)
}

#[test]
fn defaults_apply_when_only_the_key_is_given() -> Result<(), clap::Error> {
    let args = parse(&[])?;
    assert_eq!(args.api_url.as_str(), DEFAULT_API_URL);
    assert!(args.organization_id.is_none());
    assert_eq!(args.snapshot, "sandbox-agent:dev");
    assert_eq!(args.max_concurrent_leases, 4);
    assert_eq!(args.http_timeout_secs, 300);
    assert_eq!(args.start_timeout_secs, 300);
    assert_eq!(args.snapshot_poll_interval_secs, 2);
    assert_eq!(args.snapshot_timeout_secs, 300);
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert!(args.dispatch_fence_file.is_none());
    assert!(args.dispatch_ledger_file.is_none());
    assert!(args.regions.is_empty());
    assert!(args.command_supervisor.is_none() && args.command_supervisor_sha256.is_none());
    Ok(())
}

#[test]
fn flags_override_the_defaults() -> Result<(), clap::Error> {
    let args = parse(&[
        "--organization-id",
        "org_1",
        "--snapshot",
        "custom",
        "--http-timeout-secs",
        "10",
        "--start-timeout-secs",
        "60",
        "--snapshot-poll-interval-secs",
        "5",
        "--snapshot-timeout-secs",
        "90",
        "--api-url",
        "http://127.0.0.1:9",
        "--dispatch-fence-file",
        "/tmp/fence",
        "--dispatch-ledger-file",
        "/tmp/ledger.sqlite3",
        "--regions",
        "eu,us",
    ])?;
    assert_eq!(
        args.organization_id.as_ref().map(DaytonaOrganizationId::as_str),
        Some("org_1")
    );
    assert_eq!(args.snapshot, "custom");
    assert_eq!(args.http_timeout_secs, 10);
    assert_eq!(args.start_timeout_secs, 60);
    assert_eq!(args.snapshot_poll_interval_secs, 5);
    assert_eq!(args.snapshot_timeout_secs, 90);
    assert_eq!(args.api_url.as_str(), "http://127.0.0.1:9/");
    assert_eq!(args.dispatch_fence_file, Some(PathBuf::from("/tmp/fence")));
    assert_eq!(args.dispatch_ledger_file, Some(PathBuf::from("/tmp/ledger.sqlite3")));
    assert_eq!(args.regions, vec!["eu", "us"]);
    Ok(())
}

#[test]
fn bad_values_are_refused() {
    for extra in [
        ["--http-timeout-secs", "0"],
        ["--start-timeout-secs", "0"],
        ["--snapshot-poll-interval-secs", "0"],
        ["--snapshot-timeout-secs", "0"],
        ["--start-timeout-secs", "3601"],
        ["--api-url", "not a url"],
        ["--api-url", "http://example.com"],
        ["--max-concurrent-leases", "many"],
    ] {
        assert!(parse(&extra).is_err(), "{extra:?}");
    }
}

#[test]
fn the_key_is_required_and_validated() {
    let missing = WorkerArgs::try_parse_from(["trogon-sandbox-daytona-worker"]);
    assert!(matches!(missing, Err(ref error) if error.kind() == ErrorKind::MissingRequiredArgument));
    let empty = WorkerArgs::try_parse_from(["trogon-sandbox-daytona-worker", "--api-key", ""]);
    assert!(matches!(empty, Err(ref error) if error.kind() == ErrorKind::ValueValidation));
}

#[test]
fn the_supervisor_and_its_digest_come_together() -> Result<(), clap::Error> {
    let pin = "a".repeat(64);
    let both = parse(&[
        "--command-supervisor",
        "/opt/supervisor",
        "--command-supervisor-sha256",
        &pin,
    ])?;
    assert_eq!(both.command_supervisor, Some(PathBuf::from("/opt/supervisor")));
    assert_eq!(both.command_supervisor_sha256.as_deref(), Some(pin.as_str()));
    for extra in [
        vec!["--command-supervisor", "/opt/supervisor"],
        vec!["--command-supervisor-sha256", pin.as_str()],
    ] {
        let alone = parse(&extra);
        assert!(matches!(alone, Err(ref error) if error.kind() == ErrorKind::MissingRequiredArgument));
    }
    Ok(())
}
