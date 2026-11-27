use std::error::Error;

use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CapabilityNeed, CapabilityRequirements, CapabilitySupport, CredentialProvider, CredentialRequest, CredentialScope,
    IsolationTier, LeaseRequest, ProviderCapabilityProfile, PullSecretId, ResourceRange, SandboxSpec,
};

use super::{Egress, admit, admit_with};
use crate::PROFILE;

fn appcontainer_profile() -> ProviderCapabilityProfile {
    let off = CapabilitySupport::Unsupported;
    let mut profile = PROFILE.clone();
    profile.isolation.default_tier = IsolationTier::Container;
    profile.isolation.supported_tiers = vec![IsolationTier::Container];
    profile.isolation.supports_domain_allowlist = off;
    profile.isolation.supports_tls_intercept = off;
    profile.isolation.supports_secret_substitution = off;
    profile.isolation.supports_secret_bindings = off;
    profile.isolation.supports_masked_files = off;
    profile.isolation.supports_fs_policy = off;
    profile.volume.supports_fuse_mounts = off;
    profile
}

fn base_json() -> Value {
    json!({
        "tenant": "t",
        "provider_pref": ["subprocess"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "registry.example", "repository": "img", "tag": "latest" },
            },
            "workspace": { "empty": {} },
            "cpu_seconds": 2,
            "memory_mib": 512,
        },
        "ttl_seconds": 300,
        "idempotency_key": "k",
    })
}

/// A lease request whose JSON `edit` changes before it is parsed.
fn request(edit: impl FnOnce(&mut Value)) -> Result<LeaseRequest, Box<dyn Error>> {
    let mut wire = base_json();
    edit(&mut wire);
    Ok(serde_json::from_value(wire)?)
}

fn refused(edit: impl FnOnce(&mut Value)) -> Result<String, Box<dyn Error>> {
    let error = admit(&request(edit)?, &appcontainer_profile())
        .err()
        .ok_or("the lease should be refused")?;
    Ok(error.to_string())
}

fn user_defined() -> Value {
    json!({ "user_defined": { "rules": [], "allow_ranges": [], "deny_ranges": [] } })
}

#[test]
fn a_disk_quota_is_refused_because_no_host_directory_quota_exists() -> Result<(), Box<dyn Error>> {
    assert!(
        refused(|wire| wire["spec"]["disk_mib"] = json!(1024))?.contains("capabilities this worker does not advertise")
    );
    Ok(())
}

#[test]
fn a_vcpu_share_is_refused_unless_the_range_is_advertised() -> Result<(), Box<dyn Error>> {
    let mut profile = appcontainer_profile();
    profile.resources.vcpu_millis = None;
    let req = request(|wire| wire["spec"]["vcpu_millis"] = json!(500))?;
    assert!(admit(&req, &profile).is_err());
    profile.resources.vcpu_millis = Some(ResourceRange::new(100, 2000)?);
    admit(&req, &profile)?;
    let too_big = request(|wire| wire["spec"]["vcpu_millis"] = json!(4000))?;
    assert!(admit(&too_big, &profile).is_err());
    Ok(())
}

#[test]
fn a_plain_lease_is_admitted() -> Result<(), Box<dyn Error>> {
    admit(&request(|_| {})?, &appcontainer_profile())?;
    Ok(())
}

#[test]
fn only_the_options_of_the_platform_the_worker_runs_on_are_admitted() -> Result<(), Box<dyn Error>> {
    let macos = request(|wire| wire["spec"]["platform"] = json!({ "macos": { "allow_pty": true } }))?;
    let linux = request(|wire| wire["spec"]["platform"] = json!({ "linux": { "weaker_nested_sandbox": true } }))?;
    assert_eq!(
        admit(&macos, &appcontainer_profile()).is_ok(),
        cfg!(target_os = "macos")
    );
    assert_eq!(
        admit(&linux, &appcontainer_profile()).is_ok(),
        cfg!(target_os = "linux")
    );
    Ok(())
}

#[test]
fn a_lease_with_the_network_denied_is_admitted() -> Result<(), Box<dyn Error>> {
    admit(
        &request(|wire| wire["spec"]["network_policy"] = json!("deny_all"))?,
        &appcontainer_profile(),
    )?;
    Ok(())
}

#[test]
fn a_lease_for_the_supported_tier_is_admitted() -> Result<(), Box<dyn Error>> {
    admit(
        &request(|wire| wire["isolation_tier"] = serde_json::to_value(IsolationTier::Container).unwrap_or_default())?,
        &appcontainer_profile(),
    )?;
    Ok(())
}

#[test]
fn network_access_is_refused() -> Result<(), Box<dyn Error>> {
    assert!(refused(|wire| wire["spec"]["network_policy"] = json!("allow_all"))?.contains("network"));
    assert!(refused(|wire| wire["spec"]["network_policy"] = user_defined())?.contains("network"));
    Ok(())
}

#[test]
fn a_domain_allowlist_is_admitted_only_by_a_worker_that_serves_one() -> Result<(), Box<dyn Error>> {
    let lease = request(|wire| wire["spec"]["network_policy"] = user_defined())?;
    admit_with(&lease, &appcontainer_profile(), Egress::DomainAllowlist)?;
    assert!(admit_with(&lease, &appcontainer_profile(), Egress::Denied).is_err());

    let open = request(|wire| wire["spec"]["network_policy"] = json!("allow_all"))?;
    let error = admit_with(&open, &appcontainer_profile(), Egress::DomainAllowlist)
        .err()
        .ok_or("open network access is refused")?;
    assert!(error.to_string().contains("domain allowlist"));
    Ok(())
}

#[test]
fn terminating_tls_is_admitted_only_by_a_worker_that_does() -> Result<(), Box<dyn Error>> {
    let intercepting = json!({ "user_defined": {
        "rules": [{
            "domain": "internal.example.com",
            "action": { "forward": { "forward_url": "https://proxy.example.com/relay" } },
        }],
        "allow_ranges": [],
        "deny_ranges": [],
    } });
    let lease = request(|wire| wire["spec"]["network_policy"] = intercepting)?;
    let profile = appcontainer_profile();
    assert!(admit_with(&lease, &profile, Egress::DomainAllowlist).is_err());
    admit_with(&lease, &profile, Egress::DomainAllowlistTerminatingTls)?;
    Ok(())
}

/// An env past the entry limit never parses into a lease request, so it cannot reach admission.
#[test]
fn an_oversized_env_is_refused() -> Result<(), Box<dyn Error>> {
    let error = request(|wire| {
        let env: serde_json::Map<String, Value> = (0..=SandboxSpec::MAX_ENV_ENTRIES)
            .map(|i| (format!("K{i}"), json!("v")))
            .collect();
        wire["spec"]["env"] = Value::Object(env);
    })
    .err()
    .ok_or("the oversized env should be refused")?;
    assert!(error.to_string().contains("env"), "{error}");
    Ok(())
}

#[test]
fn privilege_and_another_user_are_refused() -> Result<(), Box<dyn Error>> {
    assert!(refused(|wire| wire["spec"]["privileged"] = json!(true))?.contains("privileged"));
    assert!(refused(|wire| wire["spec"]["user"] = json!("someone"))?.contains("user"));
    Ok(())
}

#[test]
fn a_filesystem_policy_is_refused_but_an_empty_one_is_not() -> Result<(), Box<dyn Error>> {
    let policy = json!({ "deny_read": [{ "kind": "literal", "path": "/home" }] });
    assert!(refused(|wire| wire["spec"]["fs_policy"] = policy.clone())?.contains("filesystem"));
    let fenced = request(|wire| wire["spec"]["fs_policy"] = policy)?;
    admit_with(&fenced, &PROFILE, Egress::DomainAllowlist)?;
    admit(
        &request(|wire| wire["spec"]["fs_policy"] = json!({}))?,
        &appcontainer_profile(),
    )?;
    Ok(())
}

#[test]
fn secret_bindings_are_refused() -> Result<(), Box<dyn Error>> {
    let source = CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new("s"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".to_string(),
        },
        ttl_seconds: 60,
        requester: None,
    };
    let binding = json!({
        "name": "api",
        "target": { "env": "API_TOKEN" },
        "mode": "mask",
        "inject_hosts": ["api.example.com"],
        "source": serde_json::to_value(source)?,
    });
    let policy = json!({ "user_defined": {
        "rules": [{ "domain": "api.example.com" }],
        "allow_ranges": [],
        "deny_ranges": [],
    } });
    let lease = request(|wire| {
        wire["spec"]["network_policy"] = policy.clone();
        wire["spec"]["secret_bindings"] = json!([binding]);
    })?;
    let error = admit_with(&lease, &PROFILE, Egress::DomainAllowlist)
        .err()
        .ok_or("the lease should be refused")?;
    assert!(error.to_string().contains("secret"));
    Ok(())
}

#[test]
fn a_tier_the_worker_does_not_provide_is_refused() -> Result<(), Box<dyn Error>> {
    let tier = serde_json::to_value(IsolationTier::Bwrap)?;
    assert!(refused(|wire| wire["isolation_tier"] = tier)?.contains("isolation"));
    Ok(())
}

#[test]
fn required_capabilities_the_profile_lacks_are_refused() -> Result<(), Box<dyn Error>> {
    let needs = CapabilityRequirements {
        needs_domain_allowlist: CapabilityNeed::Required,
        ..Default::default()
    };
    let needs = serde_json::to_value(needs)?;
    assert!(refused(|wire| wire["required_capabilities"] = needs)?.contains("advertise"));
    Ok(())
}
