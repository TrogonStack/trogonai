use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_providers::translate_lease_request_to_sandbox_spec;

use super::*;
use crate::test_fixtures::{TestResult, lease_request};

fn classes(
    default: &'static str,
    overrides: &'static [(IsolationTier, &'static str)],
) -> impl Fn(Option<IsolationTier>) -> String {
    move |tier| {
        tier.and_then(|t| overrides.iter().find(|(o, _)| *o == t))
            .map(|(_, rc)| *rc)
            .unwrap_or(default)
            .to_string()
    }
}

#[test]
fn a_default_runtime_class_backs_both_isolated_tiers() {
    let profile = profile_for(classes("kata-fc", &[]));
    assert_eq!(profile.isolation.default_tier, IsolationTier::UserSpaceKernel);
    assert_eq!(
        profile.isolation.supported_tiers,
        vec![IsolationTier::UserSpaceKernel, IsolationTier::MicroVm]
    );
}

#[test]
fn no_runtime_class_advertises_only_a_plain_container() {
    let profile = profile_for(classes("", &[]));
    assert_eq!(profile.isolation.default_tier, IsolationTier::Container);
    assert_eq!(profile.isolation.supported_tiers, vec![IsolationTier::Container]);
}

#[test]
fn only_tiers_with_a_runtime_class_are_advertised() {
    let profile = profile_for(classes("", &[(IsolationTier::MicroVm, "kata-fc")]));
    assert_eq!(profile.isolation.default_tier, IsolationTier::Container);
    assert_eq!(
        profile.isolation.supported_tiers,
        vec![IsolationTier::Container, IsolationTier::MicroVm]
    );
}

#[test]
fn an_empty_tier_override_withdraws_that_tier() {
    let profile = profile_for(classes("kata-fc", &[(IsolationTier::UserSpaceKernel, "")]));
    assert_eq!(profile.isolation.default_tier, IsolationTier::MicroVm);
    assert_eq!(profile.isolation.supported_tiers, vec![IsolationTier::MicroVm]);
}

#[test]
fn egress_is_enforced_by_the_kubernetes_network_policy_alone() {
    let profile = profile();
    assert_eq!(
        profile.isolation.egress,
        EgressCapability {
            deny_all: CapabilitySupport::Supported,
            host_allowlist: CapabilitySupport::Unsupported,
            cidr_allowlist: CapabilitySupport::Supported,
            proxy_mediated: CapabilitySupport::Unsupported,
        }
    );
    assert!(!profile.isolation.supports_domain_allowlist.is_supported());
    assert!(!profile.isolation.supports_tls_intercept.is_supported());
    assert!(!profile.isolation.supports_secret_substitution.is_supported());
}

/// FUSE mounts stay unadvertised until the worker reads live RuntimeClass state.
#[test]
fn fuse_mounts_are_conservatively_unadvertised() {
    assert!(!profile().volume.supports_fuse_mounts.is_supported());
}

/// The wire `privileged` flag survives the translation the worker's lease path uses.
#[test]
fn privileged_flag_survives_lease_request_translation() -> TestResult {
    let lease_id = LeaseId::new("01JPROFILETEST00000000000");
    let spec = translate_lease_request_to_sandbox_spec(&lease_request(true)?, &lease_id)?;
    assert!(spec.privileged.is_granted());
    Ok(())
}
