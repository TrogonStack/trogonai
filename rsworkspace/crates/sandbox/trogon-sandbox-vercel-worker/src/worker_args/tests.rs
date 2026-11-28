use clap::Parser;
use clap::error::ErrorKind;

use super::*;

fn parse(extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let mut argv = vec![
        "trogon-sandbox-vercel-worker",
        "--token",
        "test",
        "--team-id",
        "team_1",
        "--project-id",
        "prj_1",
    ];
    argv.extend_from_slice(extra);
    WorkerArgs::try_parse_from(argv)
}

#[test]
fn defaults_apply_when_only_credentials_are_given() -> Result<(), clap::Error> {
    let args = parse(&[])?;
    assert_eq!(args.team_id.as_str(), "team_1");
    assert_eq!(args.project_id.as_str(), "prj_1");
    assert_eq!(args.api_url.as_str(), DEFAULT_API_URL);
    assert_eq!(args.runtime, "node24");
    assert_eq!(args.plan, VercelPlan::Pro);
    assert_eq!(args.max_vcpus, DEFAULT_MAX_VCPUS);
    assert_eq!(args.max_concurrent_leases, 4);
    assert_eq!(args.http_timeout_secs, 300);
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert!(args.regions.is_empty());
    assert!(args.dispatch_fence_file.is_none());
    assert!(args.dispatch_ledger_file.is_none());
    Ok(())
}

#[test]
fn flags_override_the_defaults() -> Result<(), clap::Error> {
    let args = parse(&[
        "--plan",
        "enterprise",
        "--max-vcpus",
        "16",
        "--runtime",
        "python3.13",
        "--regions",
        "iad1,sfo1",
        "--http-timeout-secs",
        "10",
        "--api-url",
        "http://127.0.0.1:9",
        "--dispatch-fence-file",
        "/tmp/fence",
        "--dispatch-ledger-file",
        "/tmp/ledger.sqlite3",
    ])?;
    assert_eq!(args.plan, VercelPlan::Enterprise);
    assert_eq!(args.max_vcpus, 16);
    assert_eq!(args.runtime, "python3.13");
    assert_eq!(args.regions, vec!["iad1", "sfo1"]);
    assert_eq!(args.http_timeout_secs, 10);
    assert_eq!(args.api_url.as_str(), "http://127.0.0.1:9/");
    assert_eq!(args.dispatch_fence_file, Some(PathBuf::from("/tmp/fence")));
    assert_eq!(args.dispatch_ledger_file, Some(PathBuf::from("/tmp/ledger.sqlite3")));
    Ok(())
}

#[test]
fn bad_values_are_refused() {
    for extra in [
        ["--plan", "team"],
        ["--max-vcpus", "0"],
        ["--http-timeout-secs", "0"],
        ["--api-url", "not a url"],
        ["--max-concurrent-leases", "many"],
    ] {
        assert!(parse(&extra).is_err(), "{extra:?}");
    }
}

#[test]
fn credentials_are_required_and_validated() {
    let missing = WorkerArgs::try_parse_from(["trogon-sandbox-vercel-worker", "--team-id", "t", "--project-id", "p"]);
    assert!(matches!(missing, Err(ref error) if error.kind() == ErrorKind::MissingRequiredArgument));
    let empty = WorkerArgs::try_parse_from([
        "trogon-sandbox-vercel-worker",
        "--token",
        "",
        "--team-id",
        "t",
        "--project-id",
        "p",
    ]);
    assert!(matches!(empty, Err(ref error) if error.kind() == ErrorKind::ValueValidation));
}
