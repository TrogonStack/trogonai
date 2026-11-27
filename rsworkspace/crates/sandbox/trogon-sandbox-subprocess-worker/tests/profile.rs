use trogon_sandbox_contract::{
    CapabilityNeed, CapabilityRequirements, CapabilitySupport, EgressCapability, EgressEnforcementLevel,
};
#[cfg(windows)]
use trogon_sandbox_contract::{IsolationTier, ResourceLimitMechanism};
use trogon_sandbox_subprocess_worker::profile_satisfaction::ProfileSatisfies;
use trogon_sandbox_subprocess_worker::{IsolationBackendKind, PROFILE, profile_for_backend};

fn support(supported: bool) -> CapabilitySupport {
    CapabilitySupport::from(supported)
}

#[test]
fn the_backend_none_does_not_claim_egress_it_cannot_enforce() {
    let profile = profile_for_backend(IsolationBackendKind::None);
    assert_eq!(profile.isolation.egress_enforcement, EgressEnforcementLevel::None);
    assert_eq!(profile.isolation.egress, EgressCapability::NONE);
    assert!(!profile.isolation.supports_domain_allowlist.is_supported());
    assert!(!profile.isolation.supports_secret_bindings.is_supported());

    let deny_all = CapabilityRequirements {
        min_egress_enforcement: Some(EgressEnforcementLevel::BestEffort),
        ..Default::default()
    };
    assert!(!profile.satisfies(&deny_all));
    let nothing_needed = CapabilityRequirements {
        needs_domain_allowlist: CapabilityNeed::NotRequired,
        ..Default::default()
    };
    assert!(profile.satisfies(&nothing_needed));
}

#[cfg(target_os = "linux")]
#[test]
fn a_confining_backend_keeps_the_full_profile() {
    assert_eq!(profile_for_backend(IsolationBackendKind::Bubblewrap), *PROFILE);
}

#[test]
fn profile_mandatory_egress_and_no_snapshot() {
    assert_eq!(PROFILE.isolation.egress_enforcement, EgressEnforcementLevel::Mandatory);
    assert!(!PROFILE.snapshot.can_snapshot.is_supported());
    assert!(!PROFILE.snapshot.can_checkpoint_running.is_supported());
    assert!(!PROFILE.snapshot.can_restore.is_supported());
}

#[cfg(not(windows))]
#[test]
fn profile_advertises_the_proxy_enforced_egress_features() {
    assert!(PROFILE.isolation.supports_domain_allowlist.is_supported());
    assert!(PROFILE.isolation.supports_tls_intercept.is_supported());
    assert!(PROFILE.isolation.supports_secret_bindings.is_supported());
    assert!(PROFILE.isolation.supports_secret_substitution.is_supported());
}

#[test]
fn profile_advertises_masked_files_only_where_a_fake_copy_can_be_bound() {
    assert_eq!(
        PROFILE.isolation.supports_masked_files,
        support(cfg!(target_os = "linux"))
    );
}

#[test]
fn profile_advertises_file_operations_only_where_the_worker_serves_them() {
    let serves = support(cfg!(any(target_os = "linux", target_os = "macos")));
    assert_eq!(PROFILE.isolation.supports_file_ops, serves);
    assert_eq!(PROFILE.isolation.supports_file_search, serves);
    assert_eq!(PROFILE.isolation.supports_file_archive, serves);
}

#[test]
fn profile_does_not_claim_output_redaction_without_serving_run_command() {
    assert!(!PROFILE.isolation.supports_output_redaction.is_supported());
}

#[cfg(windows)]
#[test]
fn profile_advertises_only_what_the_appcontainer_enforces() {
    let isolation = &PROFILE.isolation;
    assert_eq!(isolation.default_tier, IsolationTier::Container);
    assert_eq!(isolation.supported_tiers, vec![IsolationTier::Container]);
    assert_eq!(isolation.resource_limits, ResourceLimitMechanism::JobObject);
    assert!(!isolation.supports_domain_allowlist.is_supported());
    assert!(!isolation.supports_tls_intercept.is_supported());
    assert!(!isolation.supports_secret_bindings.is_supported());
    assert!(!isolation.supports_secret_substitution.is_supported());
    assert!(!isolation.supports_masked_files.is_supported());
    assert!(!isolation.supports_fs_policy.is_supported());
}

#[test]
fn profile_egress_detail_follows_the_proxy_the_platform_can_run() {
    let egress = PROFILE.isolation.egress;
    let proxy = support(!cfg!(windows));
    assert!(egress.deny_all.is_supported());
    assert_eq!(egress.host_allowlist, proxy);
    assert_eq!(egress.cidr_allowlist, proxy);
    assert_eq!(egress.proxy_mediated, proxy);
    assert_eq!(egress.host_allowlist, PROFILE.isolation.supports_domain_allowlist);
}
