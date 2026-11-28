use std::collections::BTreeMap;

use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    BootMode, DeniedDomain, DomainPattern, DriveError, EnvVarName, HostImport, LeaseTtl, MonitoringRetention,
    MountAccess, MountPath, NetworkPolicy, Port, PrivilegedAccess, RemoteMount, RemoteMountSource, RetentionToggles,
    S3BucketName, S3BucketNameError, SandboxImage, SandboxSpec, SandboxSpecError, SandboxSpecParts, SecretBinding,
    SecretName, SnapshotCaptureAccess, WasiEgressRule, WasiEgressRuleError, WasiPreopenSpec, WasiPreopenSpecError,
    WasmCapabilityGrant, WorkspaceSource,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn error_of<T, E>(result: Result<T, E>) -> Result<E, Box<dyn std::error::Error>> {
    result.err().ok_or_else(|| "expected an error".into())
}

fn rejects<T: serde::de::DeserializeOwned>(value: Value) -> TestResult {
    assert_eq!(error_of(serde_json::from_value::<T>(value.clone()))?.classify(), Data);
    assert_eq!(
        error_of(serde_json::from_str::<T>(&value.to_string()))?.classify(),
        Data
    );
    Ok(())
}

fn base() -> Value {
    json!({
        "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
        "workspace": { "empty": {} },
        "cpu_seconds": 10,
        "memory_mib": 512,
    })
}

fn base_wire() -> Value {
    let mut wire = base();
    wire["env"] = json!({});
    wire["boot_mode"] = json!("entrypoint_default");
    wire["privileged"] = json!(false);
    wire
}

fn merged(overrides: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let mut wire = base();
    let target = wire.as_object_mut().ok_or("base is an object")?;
    for (key, value) in overrides.as_object().ok_or("overrides are an object")? {
        target.insert(key.clone(), value.clone());
    }
    Ok(wire)
}

fn base_parts() -> Result<SandboxSpecParts, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value::<SandboxSpec>(base())?.into_parts())
}

fn apply(parts: &mut SandboxSpecParts, key: &str, value: Value) -> TestResult {
    match key {
        "image" => parts.image = serde_json::from_value(value)?,
        "env" => parts.env = serde_json::from_value(value)?,
        "snapshot_out" => parts.snapshot_out = serde_json::from_value(value)?,
        "network_policy" => parts.network_policy = serde_json::from_value(value)?,
        "mounts" => parts.mounts = serde_json::from_value(value)?,
        "drives" => parts.drives = serde_json::from_value(value)?,
        "exposed_ports" => parts.exposed_ports = serde_json::from_value(value)?,
        "secret_bindings" => parts.secret_bindings = serde_json::from_value(value)?,
        "idle_timeout_seconds" => parts.idle_timeout_seconds = serde_json::from_value(value)?,
        "idle_policy" => parts.idle_policy = serde_json::from_value(value)?,
        "no_ttl" => parts.no_ttl = serde_json::from_value(value)?,
        "retention" => parts.retention = serde_json::from_value(value)?,
        other => return Err(format!("unsupported override {other}").into()),
    }
    Ok(())
}

fn parts_with(overrides: &Value) -> Result<SandboxSpecParts, Box<dyn std::error::Error>> {
    let mut parts = base_parts()?;
    for (key, value) in overrides.as_object().ok_or("overrides are an object")? {
        apply(&mut parts, key, value.clone())?;
    }
    Ok(parts)
}

fn rejected(overrides: Value, expected: &SandboxSpecError) -> TestResult {
    let wire = merged(&overrides)?;
    let from_value = error_of(serde_json::from_value::<SandboxSpec>(wire.clone()))?;
    assert_eq!(from_value.classify(), Data);
    assert_eq!(from_value.to_string(), expected.to_string());
    assert_eq!(
        error_of(serde_json::from_str::<SandboxSpec>(&wire.to_string()))?.classify(),
        Data
    );
    assert_eq!(&error_of(SandboxSpec::new(parts_with(&overrides)?))?, expected);
    Ok(())
}

fn accepted(overrides: Value) -> Result<SandboxSpec, Box<dyn std::error::Error>> {
    let from_value = serde_json::from_value::<SandboxSpec>(merged(&overrides)?)?;
    let built = SandboxSpec::new(parts_with(&overrides)?)?;
    assert_eq!(from_value, built);
    Ok(built)
}

fn source() -> Value {
    json!({
        "provider": "registry",
        "pull_secret_id": "registry-pull",
        "scope": { "kind": "registry_pull", "repository_glob": "*" },
        "ttl_seconds": 60,
    })
}

fn binding(name: &str, hosts: &[&str], scope_version: Option<u8>) -> Value {
    let mut wire = json!({
        "name": name,
        "target": { "env": name },
        "mode": "mask",
        "inject_hosts": hosts,
        "source": source(),
    });
    if let Some(version) = scope_version {
        wire["scope_version"] = json!(version);
    }
    wire
}

fn network(rules: &[&str], denied: &[&str]) -> Value {
    json!({ "user_defined": {
        "rules": rules.iter().map(|domain| json!({ "domain": domain })).collect::<Vec<_>>(),
        "allow_ranges": [],
        "deny_ranges": [],
        "denied_domains": denied,
    } })
}

fn secret_name(value: &str) -> Result<SecretName, Box<dyn std::error::Error>> {
    Ok(SecretName::new(value)?)
}

fn pattern(value: &str) -> Result<DomainPattern, Box<dyn std::error::Error>> {
    Ok(DomainPattern::new(value)?)
}

fn digest() -> String {
    format!("sha256:{}", "a".repeat(64))
}

#[test]
fn a_minimal_spec_writes_only_its_required_and_always_written_fields() -> TestResult {
    let spec: SandboxSpec = serde_json::from_value(base())?;
    assert_eq!(serde_json::to_value(&spec)?, base_wire());
    assert_eq!(serde_json::from_value::<SandboxSpec>(base_wire())?, spec);
    assert!(!spec.image().is_unspecified());
    assert_eq!(spec.workspace(), &WorkspaceSource::empty());
    assert!(spec.env().is_empty());
    assert_eq!(spec.cpu_seconds().get(), 10);
    assert_eq!(spec.memory_mib().get(), 512);
    assert_eq!(spec.snapshot_out(), None);
    assert_eq!(spec.network_policy(), None);
    assert!(spec.mounts().is_empty());
    assert!(spec.drives().is_empty());
    assert!(spec.exposed_ports().is_empty());
    assert_eq!(spec.boot_mode(), BootMode::EntrypointDefault);
    assert_eq!(spec.working_dir(), None);
    assert_eq!(spec.user(), None);
    assert_eq!(spec.privileged(), PrivilegedAccess::Denied);
    assert_eq!(spec.wasi_capabilities(), None);
    assert_eq!(spec.fs_policy(), None);
    assert!(spec.secret_bindings().is_empty());
    assert!(spec.platform().is_default());
    assert_eq!(spec.idle_timeout_seconds(), None);
    assert_eq!(spec.no_ttl(), LeaseTtl::Bounded);
    assert_eq!(spec.idle_policy(), None);
    assert_eq!(spec.vcpu_millis(), None);
    assert_eq!(spec.disk_mib(), None);
    assert_eq!(spec.retention(), RetentionToggles::default());
    Ok(())
}

#[test]
fn a_full_spec_round_trips_against_literal_json() -> TestResult {
    let wire = json!({
        "image": {
            "kind": "snapshot_tag",
            "tag": { "tenant": "acme", "name": "nightly" },
            "artifact_type": "sandbox_rootfs_v1",
            "create_from": {
                "kind": "registry",
                "reference": { "registry": "ghcr.io", "repository": "acme/app", "tag": "1.0" },
                "pull_secret": "registry-pull",
            },
        },
        "workspace": { "host_mount": { "path": "/work", "readonly": true } },
        "env": { "LOG_LEVEL": "debug", "MODE": "fast" },
        "cpu_seconds": 30,
        "memory_mib": 1024,
        "snapshot_out": {
            "artifact_type": "sandbox_rootfs_v1",
            "target_repository": "snapshots/acme",
            "tag_name": "nightly",
        },
        "network_policy": { "user_defined": {
            "rules": [{ "domain": "*.example.com", "action": "pass_through" }],
            "allow_ranges": [],
            "deny_ranges": [],
            "denied_domains": ["admin.example.com"], "opaque_tunnels": "refuse",
        } },
        "mounts": [{
            "source": { "backend": "s3", "bucket": "team-data", "prefix": "inputs/" },
            "mount_path": "/mnt/inputs",
            "readonly": true,
        }],
        "drives": [{ "drive_id": { "tenant": "acme", "name": "d0" }, "mount_path": "/mnt/d0", "mode": "read_only" }],
        "exposed_ports": [8080, 3000],
        "boot_mode": "bare",
        "working_dir": "/srv",
        "user": "1000:1000",
        "privileged": true,
        "wasi_capabilities": {
            "preopens": [{ "host_dir": "/tmp/scratch", "guest_path": "/data", "readonly": false }],
            "allowed_egress": [{ "host": "example.com", "port": 443 }],
        },
        "fs_policy": { "deny_read": [{ "kind": "literal", "path": "/etc/shadow" }] },
        "secret_bindings": [{
            "name": "API_TOKEN",
            "target": { "env": "API_TOKEN" },
            "mode": "mask",
            "inject_hosts": ["api.example.com"],
            "source": source(),
        }],
        "platform": { "git_safe_directories": ["/work"] },
        "no_ttl": true,
        "idle_policy": { "stages": [
            { "after_seconds": 300, "action": "pause" },
            { "after_seconds": 3600, "action": "release" },
        ] },
        "vcpu_millis": 500,
        "disk_mib": 2048,
        "retention": { "monitoring": false, "snapshots": true },
    });
    let spec: SandboxSpec = serde_json::from_value(wire.clone())?;
    assert_eq!(serde_json::to_value(&spec)?, wire);
    assert_eq!(serde_json::from_str::<SandboxSpec>(&wire.to_string())?, spec);
    assert_eq!(SandboxSpec::new(spec.clone().into_parts())?, spec);
    assert_eq!(spec.working_dir(), Some("/srv"));
    assert_eq!(spec.user(), Some("1000:1000"));
    assert_eq!(spec.boot_mode(), BootMode::Bare);
    assert_eq!(spec.privileged(), PrivilegedAccess::Granted);
    assert_eq!(spec.no_ttl(), LeaseTtl::Unbounded);
    assert_eq!(spec.vcpu_millis(), Some(500));
    assert_eq!(spec.disk_mib(), Some(2048));
    assert_eq!(spec.mounts().len(), 1);
    assert_eq!(spec.drives().len(), 1);
    assert_eq!(spec.exposed_ports().len(), 2);
    assert_eq!(spec.secret_bindings().len(), 1);
    assert!(spec.snapshot_out().is_some());
    assert!(spec.network_policy().is_some());
    assert!(spec.wasi_capabilities().is_some());
    assert!(spec.fs_policy().is_some());
    assert!(!spec.platform().is_default());
    assert_eq!(spec.retention().monitoring, MonitoringRetention::Discarded);
    assert_eq!(spec.idle_policy().map(|policy| policy.stages().len()), Some(2));
    Ok(())
}

#[test]
fn debug_output_shows_environment_keys_and_hides_their_values() -> TestResult {
    let spec = accepted(json!({ "env": { "API_TOKEN": "hunter2", "MODE": "fast" } }))?;
    let output = format!("{spec:?}");
    assert!(!output.contains("hunter2"));
    assert!(!output.contains("fast"));
    let expected = format!(
        "SandboxSpec {{ image: {:?}, workspace: {:?}, env: {{\"API_TOKEN\": \"[REDACTED]\", \"MODE\": \"[REDACTED]\"}}, \
         cpu_seconds: {:?}, memory_mib: {:?}, snapshot_out: None, network_policy: None, mounts: [], drives: [], \
         exposed_ports: [], boot_mode: EntrypointDefault, working_dir: None, user: None, privileged: Denied, \
         wasi_capabilities: None, fs_policy: None, secret_bindings: {:?}, platform: {:?}, idle_timeout_seconds: None, \
         no_ttl: Bounded, idle_policy: None, vcpu_millis: None, disk_mib: None, retention: {:?} }}",
        spec.image(),
        spec.workspace(),
        spec.cpu_seconds(),
        spec.memory_mib(),
        spec.secret_bindings(),
        spec.platform(),
        spec.retention(),
    );
    assert_eq!(output, expected);
    Ok(())
}

#[test]
fn required_fields_and_nonzero_resources_are_enforced_on_the_wire() -> TestResult {
    for key in ["image", "workspace", "cpu_seconds", "memory_mib"] {
        let mut wire = base();
        wire.as_object_mut().ok_or("object")?.remove(key);
        rejects::<SandboxSpec>(wire)?;
    }
    rejects::<SandboxSpec>(merged(&json!({ "cpu_seconds": 0 }))?)?;
    rejects::<SandboxSpec>(merged(&json!({ "memory_mib": 0 }))?)?;
    rejects::<SandboxSpec>(merged(&json!({ "idle_timeout_seconds": 0 }))?)?;
    rejects::<SandboxSpec>(merged(&json!({ "exposed_ports": [0] }))?)?;
    Ok(())
}

#[test]
fn an_empty_image_is_rejected() -> TestResult {
    rejected(
        json!({ "image": { "kind": "registry", "reference": { "registry": "", "repository": "" } } }),
        &SandboxSpecError::EmptyImage,
    )
}

#[test]
fn env_rules_are_enforced() -> TestResult {
    let many: BTreeMap<String, String> = (0..=SandboxSpec::MAX_ENV_ENTRIES)
        .map(|index| (format!("K{index}"), String::new()))
        .collect();
    rejected(
        json!({ "env": many }),
        &SandboxSpecError::TooManyEnvEntries {
            max: SandboxSpec::MAX_ENV_ENTRIES,
            got: SandboxSpec::MAX_ENV_ENTRIES + 1,
        },
    )?;
    for key in ["", "A=B", "A\u{0}B"] {
        rejected(
            json!({ "env": { key: "v" } }),
            &SandboxSpecError::InvalidEnvKey { key: key.to_string() },
        )?;
    }
    let long_key = "K".repeat(SandboxSpec::MAX_ENV_KEY_BYTES + 1);
    rejected(
        json!({ "env": { long_key.clone(): "v" } }),
        &SandboxSpecError::EnvKeyTooLong {
            key: long_key,
            max: SandboxSpec::MAX_ENV_KEY_BYTES,
        },
    )?;
    for value in ["V".repeat(SandboxSpec::MAX_ENV_VALUE_BYTES + 1), "a\u{0}b".to_string()] {
        rejected(
            json!({ "env": { "K": value } }),
            &SandboxSpecError::InvalidEnvValue {
                key: "K".to_string(),
                max: SandboxSpec::MAX_ENV_VALUE_BYTES,
            },
        )?;
    }
    let big: BTreeMap<String, String> = (0..8)
        .map(|index| {
            (
                format!("{index}{}", "K".repeat(127)),
                "V".repeat(SandboxSpec::MAX_ENV_VALUE_BYTES),
            )
        })
        .collect();
    rejected(
        json!({ "env": big }),
        &SandboxSpecError::EnvTooLarge {
            total: 8 * (128 + SandboxSpec::MAX_ENV_VALUE_BYTES),
            max: SandboxSpec::MAX_ENV_TOTAL_BYTES,
        },
    )?;
    let longest = "V".repeat(SandboxSpec::MAX_ENV_VALUE_BYTES);
    let spec = accepted(json!({ "env": { "K".repeat(SandboxSpec::MAX_ENV_KEY_BYTES): longest } }))?;
    assert_eq!(spec.env().len(), 1);
    Ok(())
}

#[test]
fn idle_rules_are_enforced() -> TestResult {
    let max = SandboxSpec::MAX_IDLE_SECONDS;
    rejected(
        json!({ "idle_timeout_seconds": max + 1 }),
        &SandboxSpecError::IdleTimeoutTooLong { max },
    )?;
    let policy = json!({ "stages": [{ "after_seconds": 60, "action": "release" }] });
    rejected(
        json!({ "idle_timeout_seconds": 60, "idle_policy": policy }),
        &SandboxSpecError::IdleTimeoutWithPolicy,
    )?;
    rejected(
        json!({ "idle_policy": { "stages": [{ "after_seconds": max + 1, "action": "release" }] } }),
        &SandboxSpecError::IdlePolicyTooLong { max },
    )?;
    rejected(json!({ "no_ttl": true }), &SandboxSpecError::NoTtlWithoutIdleRelease)?;
    rejected(
        json!({ "no_ttl": true, "idle_policy": { "stages": [{ "after_seconds": 60, "action": "pause" }] } }),
        &SandboxSpecError::NoTtlWithoutIdleRelease,
    )?;
    accepted(json!({ "no_ttl": true, "idle_timeout_seconds": max }))?;
    accepted(json!({ "no_ttl": true, "idle_policy": { "stages": [
        { "after_seconds": 60, "action": "pause" },
        { "after_seconds": 120, "action": "release" },
    ] } }))?;
    let bounded = accepted(json!({ "idle_policy": policy }))?;
    assert_eq!(bounded.no_ttl(), LeaseTtl::Bounded);
    Ok(())
}

#[test]
fn snapshot_output_needs_snapshot_capture() -> TestResult {
    let snapshot_out = json!({ "artifact_type": "sandbox_rootfs_v1", "target_repository": "snapshots/acme" });
    rejected(
        json!({ "snapshot_out": snapshot_out, "retention": { "snapshots": false } }),
        &SandboxSpecError::SnapshotOutWithoutSnapshots,
    )?;
    accepted(json!({ "snapshot_out": snapshot_out }))?;
    accepted(json!({ "retention": { "snapshots": false } }))?;
    Ok(())
}

#[test]
fn remote_mount_paths_must_be_distinct() -> TestResult {
    let mount =
        |path: &str| json!({ "source": { "backend": "s3", "bucket": "team-data", "prefix": "" }, "mount_path": path });
    rejected(
        json!({ "mounts": [mount("/mnt/a"), mount("/mnt/a")] }),
        &SandboxSpecError::DuplicateMountPath(MountPath::new("/mnt/a")?),
    )?;
    let spec = accepted(json!({ "mounts": [mount("/mnt/a"), mount("/mnt/b")] }))?;
    assert_eq!(spec.mounts().len(), 2);
    Ok(())
}

#[test]
fn drive_rules_are_enforced() -> TestResult {
    let drive = |name: &str, path: &str| json!({ "drive_id": { "tenant": "acme", "name": name }, "mount_path": path });
    let five: Vec<Value> = (0..=SandboxSpec::MAX_DRIVES)
        .map(|index| drive(&format!("d{index}"), &format!("/mnt/{index}")))
        .collect();
    rejected(
        json!({ "drives": five }),
        &SandboxSpecError::Drives(DriveError::TooManyDrives {
            count: SandboxSpec::MAX_DRIVES + 1,
            max: SandboxSpec::MAX_DRIVES,
        }),
    )?;
    rejected(
        json!({ "drives": [drive("a", "/mnt/x"), drive("b", "/mnt/x")] }),
        &SandboxSpecError::Drives(DriveError::DriveMountPathOverlap {
            mount_path: "/mnt/x".to_string(),
        }),
    )?;
    let four: Vec<Value> = (0..SandboxSpec::MAX_DRIVES)
        .map(|index| drive(&format!("d{index}"), &format!("/mnt/{index}")))
        .collect();
    assert_eq!(
        accepted(json!({ "drives": four }))?.drives().len(),
        SandboxSpec::MAX_DRIVES
    );
    Ok(())
}

#[test]
fn exposed_port_rules_are_enforced() -> TestResult {
    let sixteen: Vec<u16> = (1..=16).collect();
    rejected(
        json!({ "exposed_ports": sixteen }),
        &SandboxSpecError::TooManyExposedPorts {
            max: SandboxSpec::MAX_EXPOSED_PORTS,
            got: 16,
        },
    )?;
    rejected(
        json!({ "exposed_ports": [8080, 3000, 8080] }),
        &SandboxSpecError::DuplicateExposedPort(Port::new(8080)?),
    )?;
    let fifteen: Vec<u16> = (1..=15).collect();
    assert_eq!(
        accepted(json!({ "exposed_ports": fifteen }))?.exposed_ports().len(),
        SandboxSpec::MAX_EXPOSED_PORTS
    );
    Ok(())
}

#[test]
fn a_binding_that_targets_a_spec_env_key_is_rejected() -> TestResult {
    rejected(
        json!({
            "env": { "GH": "x" },
            "network_policy": network(&["api.example.com"], &[]),
            "secret_bindings": [binding("GH", &["api.example.com"], None)],
        }),
        &SandboxSpecError::EnvCollision {
            name: secret_name("GH")?,
            var: EnvVarName::new("GH")?,
        },
    )?;
    accepted(json!({
        "env": { "OTHER": "x" },
        "network_policy": network(&["api.example.com"], &[]),
        "secret_bindings": [binding("GH", &["api.example.com"], None)],
    }))?;
    Ok(())
}

#[test]
fn a_binding_needs_a_user_defined_network_policy() -> TestResult {
    let expected = SandboxSpecError::NoAllowedDomains(secret_name("GH")?);
    let bindings = json!([binding("GH", &["api.example.com"], None)]);
    rejected(json!({ "secret_bindings": bindings }), &expected)?;
    for policy in [json!("allow_all"), json!("deny_all")] {
        rejected(
            json!({ "network_policy": policy, "secret_bindings": bindings }),
            &expected,
        )?;
    }
    Ok(())
}

#[test]
fn every_inject_host_must_be_covered_by_an_allowed_domain() -> TestResult {
    let not_allowed = |host: &str| -> Result<SandboxSpecError, Box<dyn std::error::Error>> {
        Ok(SandboxSpecError::InjectHostNotAllowed {
            name: secret_name("GH")?,
            host: pattern(host)?,
        })
    };
    rejected(
        json!({
            "network_policy": network(&["api.example.com"], &[]),
            "secret_bindings": [binding("GH", &["evil.example.com"], None)],
        }),
        &not_allowed("evil.example.com")?,
    )?;
    rejected(
        json!({
            "network_policy": network(&["api.example.com"], &[]),
            "secret_bindings": [binding("GH", &["*.example.com"], None)],
        }),
        &not_allowed("*.example.com")?,
    )?;
    rejected(
        json!({
            "network_policy": network(&[], &[]),
            "secret_bindings": [binding("GH", &["api.example.com"], None)],
        }),
        &not_allowed("api.example.com")?,
    )?;
    rejected(
        json!({
            "network_policy": network(&["api.example.com"], &[]),
            "secret_bindings": [binding("GH", &["api.example.com", "other.example.com"], None)],
        }),
        &not_allowed("other.example.com")?,
    )?;
    Ok(())
}

#[test]
fn allowed_domains_cover_hosts_by_name_wildcard_and_port() -> TestResult {
    for (rule, host, scope_version) in [
        ("api.example.com", "api.example.com", None),
        ("*.example.com", "api.example.com", None),
        ("*.example.com", "*.api.example.com", None),
        ("*.example.com", "*.example.com", None),
        ("api.example.com:443", "api.example.com:443", None),
        ("api.example.com:443", "api.example.com", Some(2)),
        ("api.example.com", "api.example.com", Some(2)),
        ("api.example.com", "api.example.com:8443", None),
        ("10.0.0.1", "10.0.0.1", None),
        ("api.example.com:8443", "api.example.com", Some(2)),
    ] {
        accepted(json!({
            "network_policy": network(&[rule], &[]),
            "secret_bindings": [binding("GH", &[host], scope_version)],
        }))?;
    }
    Ok(())
}

#[test]
fn an_inject_host_entirely_covered_by_a_denied_domain_is_rejected() -> TestResult {
    let denied = |host: &str, denied: &str| -> Result<SandboxSpecError, Box<dyn std::error::Error>> {
        Ok(SandboxSpecError::InjectHostDenied {
            name: secret_name("GH")?,
            host: pattern(host)?,
            denied: DeniedDomain::new(denied)?,
        })
    };
    for (rules, host, entry, scope_version, host_shown, denied_shown) in [
        (
            &["*.example.com"][..],
            "api.example.com",
            "api.example.com",
            None,
            "api.example.com",
            "api.example.com",
        ),
        (
            &["*.example.com"][..],
            "api.example.com",
            "*.example.com",
            None,
            "api.example.com",
            "*.example.com",
        ),
        (
            &["*.example.com"][..],
            "api.example.com:22",
            "api.example.com:22",
            None,
            "api.example.com:22",
            "api.example.com:22",
        ),
        (
            &["*.example.com"][..],
            "api.example.com",
            "api.example.com:443",
            Some(2),
            "api.example.com:443",
            "api.example.com:443",
        ),
        (
            &["*.example.com"][..],
            "api.example.com",
            "*",
            None,
            "api.example.com",
            "*",
        ),
        (&["10.0.0.1"][..], "10.0.0.1", "*.example.com", None, "", ""),
    ] {
        if host_shown.is_empty() {
            accepted(json!({
                "network_policy": network(rules, &[entry]),
                "secret_bindings": [binding("GH", &[host], scope_version)],
            }))?;
            continue;
        }
        rejected(
            json!({
                "network_policy": network(rules, &[entry]),
                "secret_bindings": [binding("GH", &[host], scope_version)],
            }),
            &denied(host_shown, denied_shown)?,
        )?;
    }
    Ok(())
}

#[test]
fn a_partial_overlap_with_a_denied_domain_is_accepted() -> TestResult {
    for (host, entry) in [
        ("*.example.com", "admin.example.com"),
        ("api.example.com", "api.example.com:22"),
        ("api.example.com", "other.example.com"),
        ("api.example.com", "10.0.0.1"),
    ] {
        accepted(json!({
            "network_policy": network(&["*.example.com", "api.example.com"], &[entry]),
            "secret_bindings": [binding("GH", &[host], None)],
        }))?;
    }
    Ok(())
}

#[test]
fn error_messages_are_exact() -> TestResult {
    let name = secret_name("GH")?;
    for (error, expected) in [
        (SandboxSpecError::EmptyImage, "image must not be empty"),
        (
            SandboxSpecError::TooManyEnvEntries { max: 128, got: 129 },
            "env supports at most 128 entries, got 129",
        ),
        (
            SandboxSpecError::InvalidEnvKey { key: "A=B".to_string() },
            "env key \"A=B\" must be non-empty and contain no '=' or NUL",
        ),
        (
            SandboxSpecError::EnvKeyTooLong {
                key: "K".to_string(),
                max: 128,
            },
            "env key \"K\" exceeds 128 bytes",
        ),
        (
            SandboxSpecError::InvalidEnvValue {
                key: "K".to_string(),
                max: 8192,
            },
            "env value for \"K\" must be at most 8192 bytes with no NUL",
        ),
        (
            SandboxSpecError::EnvTooLarge {
                total: 70000,
                max: 65536,
            },
            "env totals 70000 bytes, over the 65536 byte limit",
        ),
        (
            SandboxSpecError::IdleTimeoutTooLong { max: 86400 },
            "idle_timeout_seconds must be between 1 and 86400",
        ),
        (
            SandboxSpecError::IdleTimeoutWithPolicy,
            "set idle_timeout_seconds or idle_policy, not both",
        ),
        (
            SandboxSpecError::IdlePolicyTooLong { max: 86400 },
            "idle_policy stage after_seconds must be between 1 and 86400",
        ),
        (
            SandboxSpecError::NoTtlWithoutIdleRelease,
            "no_ttl needs idle_timeout_seconds or an idle_policy release stage",
        ),
        (
            SandboxSpecError::SnapshotOutWithoutSnapshots,
            "snapshot_out needs retention.snapshots on",
        ),
        (
            SandboxSpecError::DuplicateMountPath(MountPath::new("/mnt/a")?),
            "duplicate mount_path '/mnt/a'",
        ),
        (
            SandboxSpecError::Drives(DriveError::TooManyDrives { count: 5, max: 4 }),
            "too many drives: 5 exceeds max 4",
        ),
        (
            SandboxSpecError::TooManyExposedPorts { max: 15, got: 16 },
            "exposed_ports supports at most 15 ports, got 16",
        ),
        (
            SandboxSpecError::DuplicateExposedPort(Port::new(8080)?),
            "duplicate exposed_ports value 8080",
        ),
        (
            SandboxSpecError::EnvCollision {
                name: name.clone(),
                var: EnvVarName::new("GH")?,
            },
            "secret binding 'GH' targets environment variable 'GH' already set in spec.env",
        ),
        (
            SandboxSpecError::NoAllowedDomains(name.clone()),
            "secret binding 'GH' needs a user_defined network policy: inject_hosts must be covered by its allowed domains",
        ),
        (
            SandboxSpecError::InjectHostNotAllowed {
                name: name.clone(),
                host: pattern("evil.example.com")?,
            },
            "secret binding 'GH' inject host 'evil.example.com' is not covered by the allowed domains",
        ),
        (
            SandboxSpecError::InjectHostDenied {
                name,
                host: pattern("api.example.com")?,
                denied: DeniedDomain::new("*.example.com")?,
            },
            "secret binding 'GH' inject host 'api.example.com' is entirely covered by denied domain '*.example.com'",
        ),
    ] {
        assert_eq!(error.to_string(), expected);
    }
    Ok(())
}

#[test]
fn image_variants_round_trip_and_only_an_empty_registry_image_is_unspecified() -> TestResult {
    let artifact = json!({
        "registry": "ghcr.io",
        "repository": "acme/rootfs",
        "digest": digest(),
        "artifact_type": "sandbox_rootfs_v1",
    });
    for wire in [
        json!({ "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } }),
        json!({
            "kind": "registry",
            "reference": { "registry": "ghcr.io", "repository": "acme/app", "tag": "1", "digest": digest() },
            "pull_secret": "pull",
        }),
        json!({ "kind": "snapshot", "reference": artifact }),
        json!({
            "kind": "snapshot",
            "reference": artifact,
            "parent": { "registry": "ghcr.io", "repository": "acme/app" },
        }),
        json!({
            "kind": "snapshot_tag",
            "tag": { "tenant": "acme", "name": "nightly" },
            "artifact_type": "sandbox_rootfs_v1",
        }),
        json!({ "kind": "hyperlight_module", "reference": artifact }),
        json!({
            "kind": "native_snapshot",
            "reference": { "provider": "fly", "id": "snap-1", "expires_at": 1_700_000_000_u64 },
        }),
    ] {
        let image: SandboxImage = serde_json::from_value(wire.clone())?;
        assert_eq!(serde_json::to_value(&image)?, wire);
        assert!(!image.is_unspecified());
    }
    let empty: SandboxImage = serde_json::from_value(
        json!({ "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "" } }),
    )?;
    assert!(empty.is_unspecified());
    rejects::<SandboxImage>(json!({ "kind": "unknown" }))?;
    Ok(())
}

#[test]
fn remote_mounts_default_to_writable_and_validate_their_parts() -> TestResult {
    let wire = json!({
        "source": { "backend": "s3", "bucket": "team-data", "prefix": "inputs/" },
        "mount_path": "/mnt/inputs",
        "readonly": false,
    });
    let mount: RemoteMount = serde_json::from_value(wire.clone())?;
    assert_eq!(mount.access, MountAccess::ReadWrite);
    assert_eq!(mount.mount_path.as_str(), "/mnt/inputs");
    assert_eq!(
        mount.source,
        RemoteMountSource::S3 {
            bucket: S3BucketName::new("team-data")?,
            prefix: "inputs/".to_string(),
        }
    );
    assert_eq!(serde_json::to_value(&mount)?, wire);
    let omitted: RemoteMount = serde_json::from_value(json!({
        "source": { "backend": "s3", "bucket": "team-data", "prefix": "" },
        "mount_path": "/mnt/inputs",
    }))?;
    assert_eq!(omitted.access, MountAccess::ReadWrite);
    for bad in [
        json!({ "source": { "backend": "s3", "bucket": "UP", "prefix": "" }, "mount_path": "/mnt/a" }),
        json!({ "source": { "backend": "s3", "bucket": "team-data", "prefix": "" }, "mount_path": "mnt/a" }),
        json!({ "source": { "backend": "s3", "bucket": "team-data", "prefix": "" }, "mount_path": "/" }),
        json!({ "source": { "backend": "gcs", "bucket": "team-data", "prefix": "" }, "mount_path": "/mnt/a" }),
    ] {
        rejects::<RemoteMount>(bad)?;
    }
    Ok(())
}

#[test]
fn s3_bucket_names_follow_the_bucket_grammar() -> TestResult {
    let longest = "a".repeat(S3BucketName::MAX_LEN);
    for value in ["abc", "a.b-c", "0abc9", longest.as_str()] {
        let name = S3BucketName::new(value)?;
        assert_eq!(name.as_str(), value);
        assert_eq!(name.to_string(), value);
        assert_eq!(value.parse::<S3BucketName>()?, name);
        assert_eq!(S3BucketName::try_from(value.to_string())?, name);
        assert_eq!(serde_json::to_value(&name)?, json!(value));
        assert_eq!(serde_json::from_value::<S3BucketName>(json!(value))?, name);
        assert_eq!(String::from(name), value);
    }
    let too_long = "a".repeat(S3BucketName::MAX_LEN + 1);
    for value in [
        "",
        "ab",
        too_long.as_str(),
        "-abc",
        "abc-",
        ".abc",
        "abc.",
        "Abc",
        "a_c",
        "a c",
    ] {
        let expected = S3BucketNameError(value.to_string());
        assert_eq!(error_of(S3BucketName::new(value))?, expected, "{value:?}");
        assert_eq!(error_of(value.parse::<S3BucketName>())?, expected);
        rejects::<S3BucketName>(json!(value))?;
    }
    assert_eq!(
        S3BucketNameError("Abc".to_string()).to_string(),
        "'Abc' is not a valid S3 bucket name"
    );
    Ok(())
}

#[test]
fn wasi_preopens_and_egress_rules_enforce_their_fields() -> TestResult {
    let preopen = WasiPreopenSpec::new("/tmp/scratch", "/data", MountAccess::ReadOnly)?;
    assert_eq!(preopen.host_dir(), "/tmp/scratch");
    assert_eq!(preopen.guest_path(), "/data");
    assert_eq!(preopen.access(), MountAccess::ReadOnly);
    let wire = json!({ "host_dir": "/tmp/scratch", "guest_path": "/data", "readonly": true });
    assert_eq!(serde_json::to_value(&preopen)?, wire);
    assert_eq!(serde_json::from_value::<WasiPreopenSpec>(wire)?, preopen);
    assert_eq!(
        serde_json::from_value::<WasiPreopenSpec>(json!({ "host_dir": "/tmp", "guest_path": "/" }))?.access(),
        MountAccess::ReadWrite
    );
    for (host_dir, guest_path, expected, message) in [
        (
            "",
            "/data",
            WasiPreopenSpecError::EmptyHostDir,
            "host_dir must not be empty",
        ),
        (
            "/tmp",
            "data",
            WasiPreopenSpecError::RelativeGuestPath,
            "guest_path must be absolute",
        ),
        (
            "/tmp",
            "/a/../b",
            WasiPreopenSpecError::ParentComponent,
            "guest_path must not contain '..' components",
        ),
    ] {
        assert_eq!(
            error_of(WasiPreopenSpec::new(host_dir, guest_path, MountAccess::ReadWrite))?,
            expected
        );
        assert_eq!(expected.to_string(), message);
        rejects::<WasiPreopenSpec>(json!({ "host_dir": host_dir, "guest_path": guest_path }))?;
    }

    let rule = WasiEgressRule::new("example.com", Port::new(443)?)?;
    assert_eq!(rule.host(), "example.com");
    assert_eq!(rule.port(), Port::new(443)?);
    let wire = json!({ "host": "example.com", "port": 443 });
    assert_eq!(serde_json::to_value(&rule)?, wire);
    assert_eq!(serde_json::from_value::<WasiEgressRule>(wire)?, rule);
    assert_eq!(
        error_of(WasiEgressRule::new("", Port::new(443)?))?,
        WasiEgressRuleError::EmptyHost
    );
    assert_eq!(WasiEgressRuleError::EmptyHost.to_string(), "host must not be empty");
    rejects::<WasiEgressRule>(json!({ "host": "", "port": 443 }))?;
    rejects::<WasiEgressRule>(json!({ "host": "example.com", "port": 0 }))?;
    Ok(())
}

#[test]
fn a_capability_grant_omits_empty_lists() -> TestResult {
    let grant = WasmCapabilityGrant::default();
    assert_eq!(serde_json::to_value(&grant)?, json!({}));
    assert_eq!(serde_json::from_value::<WasmCapabilityGrant>(json!({}))?, grant);
    Ok(())
}

#[test]
fn host_imports_are_tagged_by_kind() -> TestResult {
    let import = HostImport::FetchUrl {
        allow_domains: vec!["example.com".to_string()],
        max_body_bytes: 1024,
        timeout_seconds: 5,
    };
    let wire = json!({
        "kind": "fetch_url",
        "allow_domains": ["example.com"],
        "max_body_bytes": 1024,
        "timeout_seconds": 5,
    });
    assert_eq!(serde_json::to_value(&import)?, wire);
    assert_eq!(serde_json::from_value::<HostImport>(wire)?, import);
    rejects::<HostImport>(json!({ "kind": "other" }))?;
    Ok(())
}

#[test]
fn flags_serialize_as_booleans_and_default_to_the_permissive_value() -> TestResult {
    assert_eq!(
        serde_json::to_value(RetentionToggles::default())?,
        json!({ "monitoring": true, "snapshots": true })
    );
    assert_eq!(
        serde_json::from_value::<RetentionToggles>(json!({}))?,
        RetentionToggles::default()
    );
    assert!(RetentionToggles::default().is_default());
    let off = RetentionToggles {
        monitoring: MonitoringRetention::Discarded,
        snapshots: SnapshotCaptureAccess::Refused,
    };
    assert!(!off.is_default());
    assert_eq!(
        serde_json::to_value(off)?,
        json!({ "monitoring": false, "snapshots": false })
    );
    assert_eq!(
        serde_json::from_value::<RetentionToggles>(json!({ "monitoring": false, "snapshots": false }))?,
        off
    );
    assert!(MonitoringRetention::from(true).is_retained());
    assert!(!bool::from(MonitoringRetention::Discarded));
    assert!(SnapshotCaptureAccess::from(true).is_allowed());
    assert!(!bool::from(SnapshotCaptureAccess::Refused));
    assert!(PrivilegedAccess::from(true).is_granted());
    assert!(!bool::from(PrivilegedAccess::Denied));
    assert!(bool::from(PrivilegedAccess::Granted));
    assert!(LeaseTtl::from(true).is_unbounded());
    assert!(!bool::from(LeaseTtl::Bounded));
    assert_eq!(LeaseTtl::from(false), LeaseTtl::Bounded);
    assert!(LeaseTtl::Bounded.is_default());
    assert!(!LeaseTtl::Unbounded.is_default());
    assert!(MountAccess::from(true).is_read_only());
    assert!(!bool::from(MountAccess::ReadWrite));
    assert_eq!(serde_json::to_value(BootMode::Bare)?, json!("bare"));
    assert_eq!(
        serde_json::from_value::<BootMode>(json!("entrypoint_default"))?,
        BootMode::EntrypointDefault
    );
    Ok(())
}

type Case = (
    &'static [&'static str],
    &'static [&'static str],
    &'static str,
    Option<u8>,
    Verdict,
);

enum Verdict {
    Accepted,
    NotAllowed(&'static str),
    Denied(&'static str, &'static str),
}

use Verdict::{Accepted, Denied, NotAllowed};

#[test]
fn coverage_decides_allowed_and_denied_inject_hosts() -> TestResult {
    let cases: Vec<Case> = vec![
        // Allowed side: names, wildcards and ports.
        (&["api.example.com"], &[], "api.example.com", None, Accepted),
        (&["*.example.com"], &[], "api.example.com", None, Accepted),
        (&["*.example.com"], &[], "*.api.example.com", None, Accepted),
        (&["*.example.com"], &[], "*.example.com", None, Accepted),
        (&["10.0.0.1"], &[], "10.0.0.1", None, Accepted),
        (&["api.example.com"], &[], "api.example.com:8443", None, Accepted),
        (&["api.example.com:443"], &[], "api.example.com:443", None, Accepted),
        (
            &["api.example.com"],
            &[],
            "other.example.com",
            None,
            NotAllowed("other.example.com"),
        ),
        (
            &["api.example.com"],
            &[],
            "*.example.com",
            None,
            NotAllowed("*.example.com"),
        ),
        (&["*.example.com"], &[], "example.com", None, NotAllowed("example.com")),
        (
            &["*.example.com"],
            &[],
            "evilexample.com",
            None,
            NotAllowed("evilexample.com"),
        ),
        (&["*.example.com"], &[], "10.0.0.1", None, NotAllowed("10.0.0.1")),
        (&[], &[], "api.example.com", None, NotAllowed("api.example.com")),
        // Omitted ports: V1 spans every port, V2 means 443, and an allowed rule ignores its own port.
        (&["api.example.com:443"], &[], "api.example.com", Some(2), Accepted),
        (&["api.example.com"], &[], "api.example.com", Some(2), Accepted),
        (&["api.example.com:8443"], &[], "api.example.com", Some(2), Accepted),
        (&["api.example.com:8443"], &[], "api.example.com", Some(1), Accepted),
        // Denied side: only a full cover rejects the host.
        (
            &["*.example.com"],
            &["api.example.com"],
            "api.example.com",
            None,
            Denied("api.example.com", "api.example.com"),
        ),
        (
            &["*.example.com"],
            &["*.example.com"],
            "api.example.com",
            None,
            Denied("api.example.com", "*.example.com"),
        ),
        (
            &["*.example.com"],
            &["*.example.com"],
            "*.api.example.com",
            None,
            Denied("*.api.example.com", "*.example.com"),
        ),
        (
            &["*.example.com"],
            &["*.example.com"],
            "*.example.com",
            None,
            Denied("*.example.com", "*.example.com"),
        ),
        (
            &["*.example.com"],
            &["*"],
            "api.example.com",
            None,
            Denied("api.example.com", "*"),
        ),
        (
            &["*.example.com"],
            &["api.example.com:22"],
            "api.example.com:22",
            None,
            Denied("api.example.com:22", "api.example.com:22"),
        ),
        (
            &["*.example.com"],
            &["api.example.com"],
            "api.example.com:443",
            None,
            Denied("api.example.com:443", "api.example.com"),
        ),
        (
            &["*.example.com"],
            &["api.example.com:443"],
            "api.example.com",
            Some(2),
            Denied("api.example.com:443", "api.example.com:443"),
        ),
        (
            &["*.example.com"],
            &["api.example.com:443"],
            "api.example.com",
            Some(1),
            Accepted,
        ),
        (
            &["*.example.com"],
            &["api.example.com:80"],
            "api.example.com:443",
            None,
            Accepted,
        ),
        (
            &["*.example.com"],
            &["admin.example.com"],
            "*.example.com",
            None,
            Accepted,
        ),
        (&["*.example.com"], &["example.com"], "*.example.com", None, Accepted),
        (
            &["*.example.com"],
            &["other.example.com"],
            "api.example.com",
            None,
            Accepted,
        ),
        (&["*.example.com"], &["*.evil.com"], "api.example.com", None, Accepted),
        (&["10.0.0.1"], &["*.example.com"], "10.0.0.1", None, Accepted),
        (&["api.example.com"], &["10.0.0.1"], "api.example.com", None, Accepted),
    ];
    for (rules, denied, host, scope_version, verdict) in cases {
        let overrides = json!({
            "network_policy": network(rules, denied),
            "secret_bindings": [binding("GH", &[host], scope_version)],
        });
        match verdict {
            Accepted => {
                accepted(overrides)?;
            }
            NotAllowed(shown) => rejected(
                overrides,
                &SandboxSpecError::InjectHostNotAllowed {
                    name: secret_name("GH")?,
                    host: pattern(shown)?,
                },
            )?,
            Denied(shown, entry) => rejected(
                overrides,
                &SandboxSpecError::InjectHostDenied {
                    name: secret_name("GH")?,
                    host: pattern(shown)?,
                    denied: DeniedDomain::new(entry)?,
                },
            )?,
        }
    }
    Ok(())
}

#[test]
fn a_user_defined_policy_type_is_what_secret_bindings_need() -> TestResult {
    let policy: NetworkPolicy = serde_json::from_value(network(&["api.example.com"], &[]))?;
    let spec = accepted(json!({
        "network_policy": network(&["api.example.com"], &[]),
        "secret_bindings": [binding("GH", &["api.example.com"], None)],
    }))?;
    assert_eq!(spec.network_policy(), Some(&policy));
    let parsed: SecretBinding = serde_json::from_value(binding("GH", &["api.example.com"], None))?;
    assert_eq!(spec.secret_bindings().bindings(), [parsed]);
    Ok(())
}
