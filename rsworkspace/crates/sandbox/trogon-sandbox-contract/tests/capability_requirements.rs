use serde_json::{Map, Value, json};
use trogon_sandbox_contract::{
    CapabilityNeed, CapabilityRequirements, EgressEnforcementLevel, EnvDigest, IsolationTier, SuspendLevel,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ENV_DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const NEED_KEYS: [&str; 38] = [
    "needs_snapshot",
    "needs_checkpoint_running",
    "needs_restore",
    "needs_warm_pool",
    "needs_named_tags",
    "needs_fuse_mounts",
    "needs_privileged",
    "needs_sandbox_users",
    "needs_interactive_stdin",
    "needs_named_processes",
    "needs_output_streaming",
    "needs_process_readiness",
    "needs_graceful_stop",
    "needs_command_restart",
    "needs_port_preview",
    "needs_domain_allowlist",
    "needs_tls_intercept",
    "needs_secret_substitution",
    "needs_secret_bindings",
    "needs_masked_files",
    "needs_fs_policy",
    "needs_pause_resume",
    "needs_file_ops",
    "needs_file_search",
    "needs_file_archive",
    "needs_output_redaction",
    "needs_usage_gauges",
    "needs_terminal",
    "needs_network_policy_update",
    "needs_sandbox_groups",
    "needs_cidr_allowlist",
    "needs_hardening_overrides",
    "needs_native_passthrough",
    "needs_change_tracking",
    "needs_file_watch",
    "needs_persistent_runtime",
    "needs_dir_transfer",
    "needs_browser",
];

fn all_needs_false() -> Map<String, Value> {
    NEED_KEYS
        .iter()
        .map(|key| ((*key).to_owned(), Value::Bool(false)))
        .collect()
}

fn object(value: Value) -> Result<Map<String, Value>, Box<dyn std::error::Error>> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err("expected a JSON object".into()),
    }
}

#[test]
fn default_requirements_carry_every_need_as_false_and_no_optional_keys() -> TestResult {
    let value = serde_json::to_value(CapabilityRequirements::default())?;

    assert_eq!(value, Value::Object(all_needs_false()));
    Ok(())
}

#[test]
fn populated_requirements_round_trip_with_exact_json() -> TestResult {
    let requirements = CapabilityRequirements {
        needs_snapshot: CapabilityNeed::Required,
        needs_browser: CapabilityNeed::Required,
        min_isolation_tier: Some(IsolationTier::MicroVm),
        max_cold_restore_p50_ms: Some(250),
        exposed_port_count: Some(3),
        min_egress_enforcement: Some(EgressEnforcementLevel::Mandatory),
        needs_suspend: Some(SuspendLevel::Disk),
        vcpu_millis: Some(2000),
        disk_mib: Some(512),
        env_digest: Some(EnvDigest::new(ENV_DIGEST)?),
        region: Some("eu-west".to_owned()),
        lease_lifetime_seconds: Some(CapabilityRequirements::NO_TTL_LIFETIME),
        ..CapabilityRequirements::default()
    };
    let mut expected = all_needs_false();
    for (key, value) in object(json!({
        "needs_snapshot": true,
        "needs_browser": true,
        "min_isolation_tier": "micro_vm",
        "max_cold_restore_p50_ms": 250,
        "exposed_port_count": 3,
        "min_egress_enforcement": "mandatory",
        "needs_suspend": "disk",
        "vcpu_millis": 2000,
        "disk_mib": 512,
        "env_digest": ENV_DIGEST,
        "region": "eu-west",
        "lease_lifetime_seconds": 4_294_967_295_u32,
    }))? {
        expected.insert(key, value);
    }

    let value = serde_json::to_value(&requirements)?;

    assert_eq!(value, Value::Object(expected));
    assert_eq!(serde_json::from_value::<CapabilityRequirements>(value)?, requirements);
    Ok(())
}

#[test]
fn an_empty_object_reads_as_the_default_requirements() -> TestResult {
    let requirements: CapabilityRequirements = serde_json::from_value(json!({}))?;

    assert_eq!(requirements, CapabilityRequirements::default());
    Ok(())
}

#[test]
fn capability_need_serializes_as_a_json_boolean() -> TestResult {
    assert_eq!(serde_json::to_value(CapabilityNeed::Required)?, json!(true));
    assert_eq!(serde_json::to_value(CapabilityNeed::NotRequired)?, json!(false));
    assert_eq!(
        serde_json::from_value::<CapabilityNeed>(json!(true))?,
        CapabilityNeed::Required
    );
    assert_eq!(
        serde_json::from_value::<CapabilityNeed>(json!(false))?,
        CapabilityNeed::NotRequired
    );
    assert!(CapabilityNeed::Required.is_required());
    assert!(!CapabilityNeed::NotRequired.is_required());
    assert_eq!(CapabilityNeed::default(), CapabilityNeed::NotRequired);
    assert_eq!(CapabilityNeed::from(true), CapabilityNeed::Required);
    assert!(bool::from(CapabilityNeed::Required));
    Ok(())
}
