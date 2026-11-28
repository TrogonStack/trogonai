use super::*;

use crate::constants::PERSISTENT_RUNTIME_ENV;
use crate::core::capability_inference::{infer_capability_requirements, wants_persistent_runtime};
use crate::profile_satisfaction::ProfileSatisfies;
use trogon_sandbox_contract::{
    AllowRange, CapabilityNeed, CapabilityRequirements, CredentialInjection, CredentialProvider, CredentialRequest,
    CredentialScope, CredentialTransform, DomainPattern, DomainRule, DomainRuleAction, EgressCapability,
    EgressEnforcementLevel, ForwardUrl, HeaderName, NetworkPolicy, PullSecretId, RequestForward,
};

fn user_defined(rules: Vec<DomainRule>, allow_ranges: Vec<AllowRange>) -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules,
        allow_ranges,
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: Default::default(),
        request_rules: vec![],
    }
}

fn rule(action: DomainRuleAction) -> anyhow::Result<DomainRule> {
    Ok(DomainRule {
        domain: DomainPattern::new("example.com")?,
        action,
    })
}

fn transform() -> anyhow::Result<DomainRuleAction> {
    Ok(DomainRuleAction::Transform(CredentialTransform {
        match_rules: vec![],
        credential: CredentialRequest {
            provider: CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new("registry-pull"),
            scope: CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
        injection: CredentialInjection::Header(HeaderName::new("Authorization")?),
    }))
}

fn forward() -> anyhow::Result<DomainRuleAction> {
    Ok(DomainRuleAction::Forward(RequestForward {
        forward_url: ForwardUrl::new("https://forward.example.com")?,
        match_rules: vec![],
        ttl_seconds: RequestForward::DEFAULT_TTL_SECONDS,
    }))
}

fn cidr_only() -> anyhow::Result<NetworkPolicy> {
    Ok(user_defined(vec![], vec![AllowRange::new("10.0.0.0/8")?]))
}

fn inferred_from(policy: NetworkPolicy) -> anyhow::Result<CapabilityRequirements> {
    infer_capability_requirements(&make_network_policy_request(policy)?)
        .ok_or_else(|| anyhow::anyhow!("no requirements"))
}

fn unenforcing_profile() -> ProviderCapabilityProfile {
    let mut profile = default_capability_profile();
    profile.isolation.supports_domain_allowlist = CapabilitySupport::Unsupported;
    profile.isolation.supports_tls_intercept = CapabilitySupport::Unsupported;
    profile.isolation.supports_secret_substitution = CapabilitySupport::Unsupported;
    profile
}

#[test]
fn a_transform_rule_needs_tls_intercept_and_secret_substitution() -> TestResult {
    let req = inferred_from(user_defined(vec![rule(transform()?)?], vec![]))?;
    assert!(req.needs_domain_allowlist.is_required());
    assert!(req.needs_tls_intercept.is_required());
    assert!(req.needs_secret_substitution.is_required());
    Ok(())
}

#[test]
fn a_forward_rule_needs_tls_intercept_without_secret_substitution() -> TestResult {
    let req = inferred_from(user_defined(vec![rule(forward()?)?], vec![]))?;
    assert!(req.needs_tls_intercept.is_required());
    assert!(!req.needs_secret_substitution.is_required());
    Ok(())
}

#[test]
fn a_pass_through_rule_needs_neither_intercept_nor_substitution() -> TestResult {
    let req = inferred_from(user_defined(vec![rule(DomainRuleAction::PassThrough)?], vec![]))?;
    assert!(req.needs_domain_allowlist.is_required());
    assert!(!req.needs_tls_intercept.is_required());
    assert!(!req.needs_secret_substitution.is_required());
    Ok(())
}

#[test]
fn a_range_only_policy_needs_a_cidr_allowlist_not_a_domain_one() -> TestResult {
    let req = inferred_from(cidr_only()?)?;
    assert!(req.needs_cidr_allowlist.is_required());
    assert!(!req.needs_domain_allowlist.is_required());
    assert_eq!(req.min_egress_enforcement, Some(EgressEnforcementLevel::BestEffort));

    let hosts = inferred_from(user_defined(vec![rule(DomainRuleAction::PassThrough)?], vec![]))?;
    assert!(hosts.needs_domain_allowlist.is_required());
    assert!(!hosts.needs_cidr_allowlist.is_required());
    Ok(())
}

#[test]
fn a_cidr_only_provider_serves_range_policies_but_not_host_policies() -> TestResult {
    let mut profile = unenforcing_profile();
    profile.isolation.egress = EgressCapability {
        deny_all: CapabilitySupport::Supported,
        cidr_allowlist: CapabilitySupport::Supported,
        ..EgressCapability::NONE
    };
    let ranges = inferred_from(cidr_only()?)?;
    let hosts = inferred_from(user_defined(vec![rule(DomainRuleAction::PassThrough)?], vec![]))?;
    assert!(profile.satisfies(&ranges));
    assert!(!profile.satisfies(&hosts));

    profile.isolation.egress = EgressCapability::DENY_ONLY;
    assert!(!profile.satisfies(&ranges));
    Ok(())
}

#[test]
fn a_worker_with_a_domain_proxy_serves_range_policies_without_the_cidr_flag() -> TestResult {
    let mut profile = default_capability_profile();
    profile.isolation.egress = EgressCapability::NONE;
    assert!(profile.isolation.supports_domain_allowlist.is_supported());
    assert!(profile.satisfies(&inferred_from(cidr_only()?)?));
    Ok(())
}

#[test]
fn deriving_from_a_policy_never_lowers_an_explicit_egress_minimum() -> TestResult {
    let mut req = make_network_policy_request(NetworkPolicy::DenyAll)?;
    req.required_capabilities = Some(CapabilityRequirements {
        min_egress_enforcement: Some(EgressEnforcementLevel::Mandatory),
        ..Default::default()
    });
    let inferred = infer_capability_requirements(&req).ok_or("requirements")?;
    assert_eq!(inferred.min_egress_enforcement, Some(EgressEnforcementLevel::Mandatory));
    Ok(())
}

#[test]
fn an_unenforcing_profile_accepts_requirements_that_ask_for_no_egress_feature() {
    assert!(unenforcing_profile().satisfies(&CapabilityRequirements::default()));
}

#[test]
fn the_persistent_runtime_requirement_gates_on_profile_support() {
    let req = CapabilityRequirements {
        needs_persistent_runtime: CapabilityNeed::Required,
        ..Default::default()
    };
    let mut profile = default_capability_profile();
    profile.isolation.supports_persistent_runtime = CapabilitySupport::Supported;
    assert!(profile.satisfies(&req));
    profile.isolation.supports_persistent_runtime = CapabilitySupport::Unsupported;
    assert!(!profile.satisfies(&req));
}

#[test]
fn a_spec_wants_a_persistent_runtime_only_for_true_or_one() -> TestResult {
    let base = make_request(vec![ProviderKind::Mock])?;
    assert!(!wants_persistent_runtime(&base.spec));
    for (value, wanted) in [
        ("true", true),
        ("1", true),
        ("false", false),
        ("yes", false),
        ("TRUE", false),
    ] {
        let req = edit_request_spec(base.clone(), |parts| {
            parts.env.insert(PERSISTENT_RUNTIME_ENV.to_string(), value.to_string());
        })?;
        assert_eq!(wants_persistent_runtime(&req.spec), wanted, "{value}");
    }
    Ok(())
}
