mod common;

use common::TestResult;
use serde_json::{Value, json};
use trogon_sandbox_contract::{PullSecretId, SandboxSpec};
use trogon_sandbox_protocol::{COMMITTED_POLICY_BUCKET, credential_references};

fn source(id: &str) -> Value {
    json!({
        "provider": "registry",
        "pull_secret_id": id,
        "scope": { "kind": "registry_pull", "repository_glob": "*" },
        "ttl_seconds": 60,
    })
}

fn spec(extra: Value) -> Result<SandboxSpec, Box<dyn std::error::Error>> {
    let mut wire = json!({
        "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
        "workspace": { "empty": {} },
        "cpu_seconds": 10,
        "memory_mib": 512,
        "env": {},
        "privileged": false,
        "boot_mode": "entrypoint_default",
    });
    if let (Some(base), Some(extra)) = (wire.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    Ok(serde_json::from_value(wire)?)
}

#[test]
fn the_bucket_name_is_fixed() {
    assert_eq!(COMMITTED_POLICY_BUCKET, "sandbox-committed-credential-policies");
}

#[test]
fn a_spec_without_credentials_references_nothing() -> TestResult {
    assert!(credential_references(&spec(json!({}))?).is_empty());
    Ok(())
}

#[test]
fn secret_binding_sources_are_referenced() -> TestResult {
    let spec = spec(json!({
        "secret_bindings": [
            { "name": "A", "target": { "env": "A" }, "mode": "mask", "inject_hosts": ["a.example.com"], "source": source("pull-a") },
            { "name": "B", "target": { "env": "B" }, "mode": "mask", "inject_hosts": ["a.example.com"], "source": source("pull-a") },
        ],
        "network_policy": { "user_defined": {
            "rules": [{ "domain": "a.example.com", "action": "pass_through" }],
            "allow_ranges": [],
            "deny_ranges": [],
        } },
    }))?;
    let references = credential_references(&spec);
    assert_eq!(references.len(), 1);
    assert!(references.contains(&PullSecretId::new("pull-a")));
    Ok(())
}

#[test]
fn transform_rules_are_referenced_and_other_actions_are_not() -> TestResult {
    let spec = spec(json!({
        "secret_bindings": [
            { "name": "A", "target": { "env": "A" }, "mode": "mask", "inject_hosts": ["a.example.com"], "source": source("pull-a") },
        ],
        "network_policy": { "user_defined": {
            "rules": [
                { "domain": "a.example.com", "action": "pass_through" },
                { "domain": "b.example.com", "action": { "transform": {
                    "credential": source("pull-b"),
                    "injection": { "header": "authorization" },
                } } },
            ],
            "allow_ranges": [],
            "deny_ranges": [],
        } },
    }))?;
    let references = credential_references(&spec);
    assert_eq!(references.len(), 2);
    assert!(references.contains(&PullSecretId::new("pull-a")));
    assert!(references.contains(&PullSecretId::new("pull-b")));
    Ok(())
}
