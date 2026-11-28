use clap::Parser;

use super::SnapshotBuilderArgs;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn parses_the_defaults() -> TestResult {
    let args = SnapshotBuilderArgs::try_parse_from(["builder", "--nats-url", "nats://127.0.0.1:4222"])?;
    assert_eq!(args.nats_url, "nats://127.0.0.1:4222");
    assert_eq!(args.max_concurrent.get(), 4);
    assert!(args.require_scan_pass);
    Ok(())
}

#[test]
fn refuses_zero_concurrency_instead_of_raising_it() {
    assert!(SnapshotBuilderArgs::try_parse_from(["builder", "--max-concurrent", "0"]).is_err());
}

#[test]
fn turns_the_insecure_registries_into_hosts() -> TestResult {
    let args = SnapshotBuilderArgs::try_parse_from([
        "builder",
        "--oci-insecure-registries",
        "localhost:5000,,registry.internal:5001",
    ])?;
    let config = args.builder_config(None)?;
    let hosts: Vec<&str> = config
        .oci_insecure_registries
        .iter()
        .map(|host| host.as_str())
        .collect();
    assert_eq!(hosts, ["localhost:5000", "registry.internal:5001"]);
    Ok(())
}

#[test]
fn rejects_a_malformed_insecure_registry() -> TestResult {
    let args = SnapshotBuilderArgs::try_parse_from(["builder", "--oci-insecure-registries", "not a host"])?;
    assert!(args.builder_config(None).is_err());
    Ok(())
}
