//! The worker's command line: defaults, environment names, bounds and what the flags turn into.

use clap::{CommandFactory, Parser};
use trogon_sandbox_kubevirt_worker::{WarmPoolConfig, WorkerArgs};
use trogon_sandbox_volumes::VolumeBackend;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const REQUIRED: [&str; 5] = [
    "trogon-sandbox-kubevirt-worker",
    "--k8s-namespace",
    "ns",
    "--image",
    "img",
];

fn parse(extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let argv: Vec<&str> = REQUIRED.iter().chain(extra).copied().collect();
    WorkerArgs::try_parse_from(argv)
}

fn env_of(id: &str) -> Option<String> {
    let command = WorkerArgs::command();
    command
        .get_arguments()
        .find(|arg| arg.get_id() == id)
        .and_then(|arg| arg.get_env())
        .and_then(|env| env.to_str())
        .map(str::to_string)
}

#[test]
fn max_concurrent_leases_defaults_and_reads_the_shared_env_name() -> TestResult {
    assert_eq!(parse(&[])?.max_concurrent_leases, 4);
    assert_eq!(
        env_of("max_concurrent_leases").as_deref(),
        Some("SANDBOX_MAX_CONCURRENT_LEASES")
    );
    Ok(())
}

#[test]
fn warm_memory_defaults_to_the_pool_default() -> TestResult {
    assert_eq!(parse(&[])?.warm_memory_mib, WarmPoolConfig::default().warm_memory_mib);
    Ok(())
}

#[test]
fn warm_memory_is_settable_and_must_be_positive() -> TestResult {
    assert_eq!(parse(&["--warm-memory-mib", "2048"])?.warm_memory_mib, 2048);
    assert!(parse(&["--warm-memory-mib", "0"]).is_err());
    Ok(())
}

#[test]
fn the_warm_pool_defaults_to_the_pool_defaults() -> TestResult {
    assert_eq!(parse(&[])?.warm_pool_config(), WarmPoolConfig::default());
    Ok(())
}

#[test]
fn the_warm_pool_is_settable_from_flags() -> TestResult {
    let args = parse(&[
        "--warm-pool-enabled",
        "false",
        "--warm-pool-max-per-digest",
        "2",
        "--warm-pool-max-total",
        "8",
        "--warm-pool-max-idle-secs",
        "60",
        "--warm-memory-mib",
        "1024",
    ])?;
    assert_eq!(
        args.warm_pool_config(),
        WarmPoolConfig {
            enabled: false,
            max_pool_size_per_digest: 2,
            max_total_warm: 8,
            max_idle_seconds: 60,
            warm_memory_mib: 1024,
        }
    );
    Ok(())
}

#[test]
fn the_warm_pool_sizes_and_idle_time_must_be_positive() {
    for flag in [
        "--warm-pool-max-per-digest",
        "--warm-pool-max-total",
        "--warm-pool-max-idle-secs",
    ] {
        assert!(parse(&[flag, "0"]).is_err(), "{flag} accepted 0");
    }
}

#[test]
fn the_warm_pool_flags_have_their_environment_variables() {
    for (id, env) in [
        ("warm_pool_enabled", "SANDBOX_KUBEVIRT_WARM_POOL_ENABLED"),
        ("warm_pool_max_per_digest", "SANDBOX_KUBEVIRT_WARM_POOL_MAX_PER_DIGEST"),
        ("warm_pool_max_total", "SANDBOX_KUBEVIRT_WARM_POOL_MAX_TOTAL"),
        ("warm_pool_max_idle_secs", "SANDBOX_KUBEVIRT_WARM_POOL_MAX_IDLE_SECS"),
        ("warm_memory_mib", "SANDBOX_KUBEVIRT_WARM_MEMORY_MIB"),
    ] {
        assert_eq!(env_of(id).as_deref(), Some(env), "{id}");
    }
}

#[test]
fn the_snapshot_registry_is_always_an_insecure_registry_once() -> TestResult {
    let args = parse(&[
        "--snapshot-registry",
        "registry.local:5000",
        "--oci-insecure-registries",
        "other:5000,registry.local:5000",
    ])?;
    let hosts: Vec<String> = args.insecure_registries().iter().map(ToString::to_string).collect();
    assert_eq!(hosts, ["other:5000", "registry.local:5000"]);
    let defaulted: Vec<String> = parse(&[])?
        .insecure_registries()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(defaulted, ["localhost:5000"]);
    Ok(())
}

#[test]
fn a_registry_with_a_path_is_refused() {
    assert!(parse(&["--snapshot-registry", "registry.local/path"]).is_err());
}

#[test]
fn volumes_are_configured_only_with_a_bucket() -> TestResult {
    assert!(parse(&[])?.volume_config().is_none());
    let config = parse(&["--volume-s3-bucket", "bucket", "--volume-s3-region", "eu-west-1"])?
        .volume_config()
        .ok_or("no volume config")?;
    assert!(matches!(config.backend, VolumeBackend::S3 { endpoint: None, .. }));
    Ok(())
}

#[test]
fn the_runtime_config_carries_the_flags() -> TestResult {
    let config = parse(&[
        "--storage-class",
        "fast",
        "--disk-size-gi",
        "20",
        "--worker-id",
        "kubevirt-7",
    ])?
    .kube_virt_config()?;
    assert_eq!(config.namespace, "ns");
    assert_eq!(config.default_image, "img");
    assert_eq!(config.storage_class.as_deref(), Some("fast"));
    assert_eq!(config.disk_size_gi, 20);
    assert_eq!(config.worker_id.as_str(), "kubevirt-7");
    assert!(config.cosign_verifier.is_none());
    Ok(())
}

#[test]
fn a_missing_cosign_key_fails_the_config() -> TestResult {
    let args = parse(&["--cosign-public-key", "/nonexistent/cosign.pub"])?;
    assert!(args.kube_virt_config().is_err());
    Ok(())
}
