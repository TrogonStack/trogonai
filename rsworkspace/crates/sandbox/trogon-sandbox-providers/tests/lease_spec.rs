use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use trogon_sandbox_contract::{
    BootMode, CloneAuth, HostMountAccess, LeaseId, LeaseRequest, NetworkPolicy, PrivilegedAccess, SecretRef,
};
use trogon_sandbox_providers::{
    CommitShaError, DEFAULT_LEASE_CPUS, EgressPolicy, GitProvider, LeaseSpecError, lease_cpus, lease_timeout,
    translate_lease_request_to_sandbox_spec,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn registry_image() -> Value {
    json!({"kind": "registry", "reference": {"registry": "my-registry", "repository": "agent", "tag": "v1"}})
}

fn request(image: Value, workspace: Value, spec_extra: Value, ttl: u32) -> Result<LeaseRequest, serde_json::Error> {
    let mut spec = json!({
        "image": image,
        "workspace": workspace,
        "cpu_seconds": 2,
        "memory_mib": 512,
    });
    if let (Some(spec), Some(extra)) = (spec.as_object_mut(), spec_extra.as_object()) {
        spec.extend(extra.clone());
    }
    serde_json::from_value(json!({
        "tenant": "test-tenant",
        "provider_pref": ["local_docker"],
        "spec": spec,
        "ttl_seconds": ttl,
        "idempotency_key": "idem-1",
    }))
}

fn empty_request(spec_extra: Value) -> Result<LeaseRequest, serde_json::Error> {
    request(registry_image(), json!({"empty": {}}), spec_extra, 300)
}

fn lease_id() -> LeaseId {
    LeaseId::new("01JTEST0000000000000000000")
}

#[test]
fn an_empty_workspace_uses_the_placeholder_sha() -> TestResult {
    let spec = translate_lease_request_to_sandbox_spec(&empty_request(json!({}))?, &lease_id())?;
    assert_eq!(spec.resolved_sha.as_str(), "0".repeat(40));
    assert!(spec.clone.is_none());
    assert!(spec.base_sha.is_none());
    assert!(spec.host_mount.is_none());
    assert_eq!(spec.lease_id, Some(lease_id()));
    assert_eq!(spec.repo.provider, GitProvider::GitHub);
    assert_eq!(spec.repo.owner, "sandbox");
    assert_eq!(spec.repo.name, "01jtest0000000000000000000");
    assert!(spec.tools.is_empty());
    assert!(spec.extra_binds.is_empty());
    Ok(())
}

#[test]
fn a_lease_timeout_is_its_ttl_unless_it_asked_for_none() -> TestResult {
    let bounded = empty_request(json!({}))?;
    assert_eq!(lease_timeout(&bounded), Duration::from_secs(300));
    let unbounded = empty_request(json!({"no_ttl": true, "idle_timeout_seconds": 60}))?;
    assert_eq!(lease_timeout(&unbounded), Duration::ZERO);
    let spec = translate_lease_request_to_sandbox_spec(&unbounded, &lease_id())?;
    assert!(spec.timeout.is_zero());
    Ok(())
}

#[test]
fn a_clone_workspace_sets_credentials() -> TestResult {
    let workspace = json!({"clone": {
        "url": "https://github.com/acme/widget.git",
        "sha": "a".repeat(40),
        "base_sha": "b".repeat(40),
        "auth": {"bearer_token": {"token_ref": {"literal": {"value": "tok"}}}},
    }});
    let req = request(registry_image(), workspace, json!({}), 600)?;
    let spec = translate_lease_request_to_sandbox_spec(&req, &lease_id())?;
    assert_eq!(spec.resolved_sha.as_str(), "a".repeat(40));
    assert_eq!(
        spec.base_sha.as_ref().map(|sha| sha.as_str().to_string()),
        Some("b".repeat(40))
    );
    let credentials = spec.clone.as_ref().ok_or("clone credentials")?;
    assert_eq!(credentials.clone_url, "https://github.com/acme/widget.git");
    assert_eq!(
        credentials.auth,
        CloneAuth::BearerToken {
            token_ref: SecretRef::Literal {
                value: "tok".to_string()
            }
        }
    );
    assert_eq!(spec.timeout, Duration::from_secs(600));
    Ok(())
}

#[test]
fn a_clone_workspace_without_a_base_sha_has_none() -> TestResult {
    let workspace = json!({"clone": {
        "url": "https://github.com/acme/widget.git",
        "sha": "a".repeat(40),
        "base_sha": null,
        "auth": {"none": {}},
    }});
    let spec =
        translate_lease_request_to_sandbox_spec(&request(registry_image(), workspace, json!({}), 60)?, &lease_id())?;
    assert!(spec.base_sha.is_none());
    Ok(())
}

#[test]
fn a_malformed_clone_sha_is_rejected() -> TestResult {
    let bad = |sha: &str, base: Value| -> Result<Result<_, LeaseSpecError>, serde_json::Error> {
        let workspace =
            json!({"clone": {"url": "https://x/y.git", "sha": sha, "base_sha": base, "auth": {"none": {}}}});
        Ok(translate_lease_request_to_sandbox_spec(
            &request(registry_image(), workspace, json!({}), 60)?,
            &lease_id(),
        ))
    };
    assert!(matches!(
        bad("xyz", Value::Null)?,
        Err(LeaseSpecError::InvalidResolvedSha(CommitShaError::InvalidLength(3)))
    ));
    assert!(matches!(
        bad(&"a".repeat(40), json!("zzzz"))?,
        Err(LeaseSpecError::InvalidBaseSha(CommitShaError::NonHex))
    ));
    Ok(())
}

#[test]
fn the_lease_image_is_labelled_for_every_image_kind() -> TestResult {
    let images = [
        (registry_image(), "my-registry/agent:v1"),
        (
            json!({"kind": "snapshot", "reference": {"registry": "r.io", "repository": "snap", "digest": DIGEST, "artifact_type": "sandbox_rootfs_v1"}}),
            &format!("r.io/snap@{DIGEST}"),
        ),
        (
            json!({"kind": "hyperlight_module", "reference": {"registry": "r.io", "repository": "mod", "digest": DIGEST, "artifact_type": "hyperlight_snapshot_v1"}}),
            &format!("r.io/mod@{DIGEST}"),
        ),
        (
            json!({"kind": "snapshot_tag", "tag": {"tenant": "acme", "name": "base"}, "artifact_type": "sandbox_rootfs_v1"}),
            "tag:acme:base",
        ),
        (
            json!({"kind": "native_snapshot", "reference": {"provider": "modal", "id": "snap-1"}}),
            "native:Modal:snap-1",
        ),
    ];
    for (image, label) in images {
        let req = request(image, json!({"empty": {}}), json!({}), 60)?;
        let spec = translate_lease_request_to_sandbox_spec(&req, &lease_id())?;
        assert_eq!(spec.image.as_str(), label);
    }
    Ok(())
}

#[test]
fn the_cpu_seconds_budget_does_not_size_the_container() -> TestResult {
    let req = empty_request(json!({"cpu_seconds": 3600}))?;
    assert_eq!(lease_cpus(&req.spec), DEFAULT_LEASE_CPUS);
    let spec = translate_lease_request_to_sandbox_spec(&req, &lease_id())?;
    assert_eq!(spec.cpu, DEFAULT_LEASE_CPUS);
    assert_eq!(spec.memory_mib, 512);
    Ok(())
}

#[test]
fn explicit_vcpus_and_disk_override_the_defaults() -> TestResult {
    let req = empty_request(json!({"vcpu_millis": 500, "disk_mib": 2048}))?;
    let spec = translate_lease_request_to_sandbox_spec(&req, &lease_id())?;
    assert_eq!(spec.cpu, 0.5_f32);
    assert_eq!(spec.disk_mib, Some(2048));
    Ok(())
}

#[test]
fn the_egress_policy_is_deny_all() -> TestResult {
    let spec = translate_lease_request_to_sandbox_spec(&empty_request(json!({}))?, &lease_id())?;
    assert_eq!(spec.egress_policy, EgressPolicy::DenyAll);
    Ok(())
}

#[test]
fn a_host_mount_workspace_sets_the_mount_and_no_clone() -> TestResult {
    for (readonly, access) in [(true, HostMountAccess::ReadOnly), (false, HostMountAccess::ReadWrite)] {
        let workspace = json!({"host_mount": {"path": "/srv/workspaces/acme", "readonly": readonly}});
        let req = request(registry_image(), workspace, json!({}), 60)?;
        let spec = translate_lease_request_to_sandbox_spec(&req, &lease_id())?;
        assert!(spec.clone.is_none());
        assert!(spec.base_sha.is_none());
        let mount = spec.host_mount.as_ref().ok_or("host mount")?;
        assert_eq!(mount.path, PathBuf::from("/srv/workspaces/acme"));
        assert_eq!(mount.access, access);
        assert_eq!(spec.resolved_sha.as_str(), "0".repeat(40));
    }
    Ok(())
}

#[test]
fn a_host_mount_path_must_be_present_and_absolute() -> TestResult {
    let empty = request(
        registry_image(),
        json!({"host_mount": {"path": "", "readonly": false}}),
        json!({}),
        60,
    )?;
    let error = translate_lease_request_to_sandbox_spec(&empty, &lease_id()).err();
    assert!(matches!(error, Some(LeaseSpecError::HostMountPathEmpty)));
    assert_eq!(
        error.map(|error| error.to_string()).as_deref(),
        Some("host_mount path must not be empty")
    );

    let relative = request(
        registry_image(),
        json!({"host_mount": {"path": "relative/dir", "readonly": false}}),
        json!({}),
        60,
    )?;
    let error = translate_lease_request_to_sandbox_spec(&relative, &lease_id()).err();
    assert!(matches!(error, Some(LeaseSpecError::HostMountPathNotAbsolute { .. })));
    assert_eq!(
        error.map(|error| error.to_string()).as_deref(),
        Some("host_mount path must be absolute: relative/dir")
    );
    Ok(())
}

#[test]
fn an_s3_volume_workspace_is_not_supported() -> TestResult {
    let workspace = json!({"s3_volume": {"bucket": "b", "prefix": "p", "mode": "read_only"}});
    let req = request(registry_image(), workspace, json!({}), 60)?;
    let error = translate_lease_request_to_sandbox_spec(&req, &lease_id()).err();
    assert!(matches!(error, Some(LeaseSpecError::S3VolumeUnsupported)));
    assert_eq!(
        error.map(|error| error.to_string()).as_deref(),
        Some("s3_volume workspaces are not supported by the providers yet")
    );
    Ok(())
}

#[test]
fn the_network_policy_is_threaded_through() -> TestResult {
    let none = translate_lease_request_to_sandbox_spec(&empty_request(json!({}))?, &lease_id())?;
    assert_eq!(none.network_policy, None);
    let some =
        translate_lease_request_to_sandbox_spec(&empty_request(json!({"network_policy": "deny_all"}))?, &lease_id())?;
    assert_eq!(some.network_policy, Some(NetworkPolicy::DenyAll));
    Ok(())
}

#[test]
fn user_privileged_ports_working_dir_boot_mode_and_env_are_threaded_through() -> TestResult {
    let defaults = translate_lease_request_to_sandbox_spec(&empty_request(json!({}))?, &lease_id())?;
    assert_eq!(defaults.user, None);
    assert_eq!(defaults.working_dir, None);
    assert_eq!(defaults.privileged, PrivilegedAccess::Denied);
    assert_eq!(defaults.boot_mode, BootMode::EntrypointDefault);
    assert!(defaults.exposed_ports.is_empty());
    assert!(defaults.env.is_empty());

    let req = empty_request(json!({
        "user": "1000:1000",
        "privileged": true,
        "working_dir": "/work",
        "boot_mode": "bare",
        "exposed_ports": [8080, 9090],
        "env": {"MY_VAR": "one"},
    }))?;
    let spec = translate_lease_request_to_sandbox_spec(&req, &lease_id())?;
    assert_eq!(spec.user.as_deref(), Some("1000:1000"));
    assert_eq!(spec.privileged, PrivilegedAccess::Granted);
    assert_eq!(spec.working_dir.as_deref(), Some("/work"));
    assert_eq!(spec.boot_mode, BootMode::Bare);
    assert_eq!(spec.exposed_ports, vec![8080, 9090]);
    assert_eq!(spec.env.get("MY_VAR").map(String::as_str), Some("one"));
    Ok(())
}

#[test]
fn an_oversized_env_is_rejected_by_the_contract_before_translation() {
    let env: serde_json::Map<String, Value> = (0..=128).map(|i| (format!("K{i}"), json!("v"))).collect();
    assert!(empty_request(json!({"env": env})).is_err());
}
