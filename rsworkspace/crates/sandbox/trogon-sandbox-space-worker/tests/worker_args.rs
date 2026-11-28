use std::path::Path;

use clap::Parser;
use trogon_sandbox_space_worker::WorkerArgs;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const REQUIRED: [&str; 7] = [
    "worker",
    "--endpoint",
    "https://space.example",
    "--data-plane-url",
    "https://data.space.example",
    "--token",
    "t",
];

#[test]
fn unset_options_take_their_defaults() -> TestResult {
    let args = WorkerArgs::try_parse_from(REQUIRED)?;
    assert_eq!(args.template, "seed:latest");
    assert_eq!(args.max_leases, 4);
    assert_eq!(args.boot_timeout_seconds, 180);
    assert_eq!(args.operation_timeout_seconds, 120);
    assert_eq!(args.state_dir, Path::new("/var/lib/sandbox-space"));
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    Ok(())
}

#[test]
fn explicit_options_override_the_defaults() -> TestResult {
    let mut line = REQUIRED.to_vec();
    line.extend(["--max-leases", "9", "--template", "custom:1"]);
    let args = WorkerArgs::try_parse_from(line)?;
    assert_eq!(args.max_leases, 9);
    assert_eq!(args.template, "custom:1");
    Ok(())
}

#[test]
fn the_endpoints_are_required() {
    assert!(WorkerArgs::try_parse_from(["worker", "--token", "t"]).is_err());
}
