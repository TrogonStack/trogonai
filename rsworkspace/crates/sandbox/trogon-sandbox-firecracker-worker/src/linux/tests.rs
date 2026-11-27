use super::*;

#[test]
fn starting_without_a_jailer_needs_the_insecure_switch() {
    let jailer = PathBuf::from("/jailer");
    assert!(require_jailer(Some(&jailer), false).is_ok());
    assert!(require_jailer(None, true).is_ok());
    let refused = require_jailer(None, false).err().map(|error| error.to_string());
    assert!(refused.is_some_and(|error| error.contains("--jailer") && error.contains("--insecure-dev")));
}

fn parse(extra: &[&str]) -> Result<Args, clap::Error> {
    let mut argv = vec![
        "worker",
        "--kernel",
        "/kernel",
        "--rootfs",
        "/rootfs",
        "--image",
        "guest:base",
    ];
    argv.extend_from_slice(extra);
    Args::try_parse_from(argv)
}

#[test]
fn existing_helper_and_local_snapshot_options_remain_available() -> Result<(), clap::Error> {
    let args = parse(&[
        "--netns-helper",
        "/helper",
        "--snapshot-dir",
        "/snapshots",
        "--max-snapshots",
        "3",
    ])?;
    assert_eq!(args.netns_helper, Some(PathBuf::from("/helper")));
    assert_eq!(args.snapshot_dir, Some(PathBuf::from("/snapshots")));
    assert_eq!(args.max_snapshots.get(), 3);
    Ok(())
}

#[test]
fn networking_backends_cannot_be_combined() {
    let kind = parse(&["--netns-helper", "/helper", "--host-links", "10.231.0.0/24"])
        .err()
        .map(|error| error.kind());
    assert_eq!(kind, Some(clap::error::ErrorKind::ArgumentConflict));
}

#[test]
fn a_credential_broker_needs_a_networking_backend() {
    assert!(parse(&["--credential-broker-nats-url", "nats://b"]).is_err());
    for backend in [["--netns-helper", "/helper"], ["--host-links", "10.231.0.0/24"]] {
        let mut argv = vec!["--credential-broker-nats-url", "nats://b"];
        argv.extend(backend);
        assert!(parse(&argv).is_ok(), "{backend:?}");
    }
}

#[test]
fn root_helper_cannot_launch_a_jailed_vmm() {
    let kind = parse(&["--netns-helper", "/helper", "--jailer", "/jailer"])
        .err()
        .map(|error| error.kind());
    assert_eq!(kind, Some(clap::error::ErrorKind::ArgumentConflict));
}
