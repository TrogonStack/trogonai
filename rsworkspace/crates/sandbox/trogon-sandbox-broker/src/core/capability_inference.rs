use trogon_sandbox_contract::{
    CapabilityNeed, CapabilityRequirements, DomainRuleAction, EgressEnforcementLevel, FsPolicy, LeaseRequest, LeaseTtl,
    NetworkPolicy, SandboxImage, SandboxSpec, SecretBinding, SecretMode, SecretTarget,
};

use crate::constants::PERSISTENT_RUNTIME_ENV;

fn need(required: bool) -> CapabilityNeed {
    CapabilityNeed::from(required)
}

/// Whether the spec asks for a persistent runtime through its environment.
pub(crate) fn wants_persistent_runtime(spec: &SandboxSpec) -> bool {
    matches!(
        spec.env().get(PERSISTENT_RUNTIME_ENV).map(String::as_str),
        Some("true" | "1")
    )
}

/// A lease that restores from, or exports to, a named snapshot tag needs a
/// worker that implements named tags.
pub(crate) fn wants_named_tags(req: &LeaseRequest) -> bool {
    matches!(req.spec.image(), SandboxImage::SnapshotTag { .. })
        || req.spec.snapshot_out().is_some_and(|policy| policy.tag_name.is_some())
}

/// A policy of only IP ranges.
fn is_cidr_only(policy: &NetworkPolicy) -> bool {
    match policy {
        NetworkPolicy::UserDefined {
            rules,
            allow_ranges,
            deny_ranges,
            denied_domains,
            request_rules,
            ..
        } => {
            rules.is_empty()
                && denied_domains.is_empty()
                && request_rules.is_empty()
                && !(allow_ranges.is_empty() && deny_ranges.is_empty())
        }
        NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => false,
    }
}

/// Whether any domain rule of the policy has an action that matches `pick`.
fn any_rule_action(policy: &NetworkPolicy, pick: impl Fn(&DomainRuleAction) -> bool) -> bool {
    match policy {
        NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => false,
        NetworkPolicy::UserDefined { rules, .. } => rules.iter().any(|rule| pick(&rule.action)),
    }
}

/// Adds what the network policy needs from a worker.
fn with_network_policy(mut requirements: CapabilityRequirements, policy: &NetworkPolicy) -> CapabilityRequirements {
    let domain_allowlist = matches!(policy, NetworkPolicy::UserDefined { .. }) && !is_cidr_only(policy);
    requirements.needs_domain_allowlist = need(requirements.needs_domain_allowlist.is_required() || domain_allowlist);
    requirements.needs_cidr_allowlist = need(requirements.needs_cidr_allowlist.is_required() || is_cidr_only(policy));
    let level = match policy {
        NetworkPolicy::AllowAll => None,
        NetworkPolicy::DenyAll | NetworkPolicy::UserDefined { .. } => Some(EgressEnforcementLevel::BestEffort),
    };
    if level.is_some() {
        requirements.min_egress_enforcement = requirements.min_egress_enforcement.max(level);
    }
    let intercepts = any_rule_action(policy, |action| {
        matches!(action, DomainRuleAction::Transform(_) | DomainRuleAction::Forward(_))
    });
    requirements.needs_tls_intercept = need(requirements.needs_tls_intercept.is_required() || intercepts);
    let substitutes = any_rule_action(policy, |action| matches!(action, DomainRuleAction::Transform(_)));
    requirements.needs_secret_substitution = need(requirements.needs_secret_substitution.is_required() || substitutes);
    requirements
}

/// Adds what secret bindings need from a worker. Masking additionally needs TLS
/// termination to swap sentinels back, and masking a file needs a worker that can
/// bind a fake copy over it.
fn with_secret_bindings(
    mut requirements: CapabilityRequirements,
    bindings: &[SecretBinding],
) -> CapabilityRequirements {
    if bindings.is_empty() {
        return requirements;
    }
    requirements.needs_secret_bindings = CapabilityNeed::Required;
    if bindings.iter().any(|binding| binding.mode() == SecretMode::Mask) {
        requirements.needs_tls_intercept = CapabilityNeed::Required;
        requirements.needs_secret_substitution = CapabilityNeed::Required;
    }
    if bindings
        .iter()
        .any(|binding| binding.mode() == SecretMode::Mask && matches!(binding.target(), SecretTarget::File(_)))
    {
        requirements.needs_masked_files = CapabilityNeed::Required;
    }
    requirements
}

fn with_fs_policy(mut requirements: CapabilityRequirements, policy: &FsPolicy) -> CapabilityRequirements {
    requirements.needs_fs_policy = need(requirements.needs_fs_policy.is_required() || !policy.is_empty());
    requirements
}

/// Records how long a lease must be able to live.
fn with_lease_lifetime(
    mut requirements: CapabilityRequirements,
    ttl_seconds: u32,
    ttl: LeaseTtl,
) -> CapabilityRequirements {
    requirements.lease_lifetime_seconds = Some(if ttl.is_unbounded() {
        CapabilityRequirements::NO_TTL_LIFETIME
    } else {
        ttl_seconds
    });
    requirements
}

/// Records the machine size a spec asks for.
fn with_machine_size(mut requirements: CapabilityRequirements, spec: &SandboxSpec) -> CapabilityRequirements {
    if spec.vcpu_millis().is_some() {
        requirements.vcpu_millis = spec.vcpu_millis();
    }
    if spec.disk_mib().is_some() {
        requirements.disk_mib = spec.disk_mib();
    }
    requirements
}

/// What the request needs from a worker beyond what the caller listed, or `None`
/// when the request asks for nothing unusual and lists nothing.
pub(crate) fn infer_capability_requirements(req: &LeaseRequest) -> Option<CapabilityRequirements> {
    let spec = &req.spec;
    let network_policy = spec.network_policy();
    let fs_policy = spec.fs_policy().filter(|policy| !policy.is_empty());
    let secret_bindings = spec.secret_bindings().bindings();
    if spec.mounts().is_empty()
        && network_policy.is_none()
        && fs_policy.is_none()
        && secret_bindings.is_empty()
        && !spec.privileged().is_granted()
        && spec.platform().container.is_default()
        && spec.exposed_ports().is_empty()
        && spec.vcpu_millis().is_none()
        && spec.disk_mib().is_none()
        && req.native.is_none()
        && !wants_persistent_runtime(spec)
        && !spec.no_ttl().is_unbounded()
        && !wants_named_tags(req)
    {
        return req
            .required_capabilities
            .clone()
            .map(|caps| with_lease_lifetime(caps, req.ttl_seconds, LeaseTtl::Bounded));
    }
    let mut effective = req.required_capabilities.clone().unwrap_or_default();
    let any = |current: CapabilityNeed, extra: bool| need(current.is_required() || extra);
    effective.needs_native_passthrough = any(effective.needs_native_passthrough, req.native.is_some());
    effective.needs_privileged = any(effective.needs_privileged, spec.privileged().is_granted());
    effective.needs_port_preview = any(effective.needs_port_preview, !spec.exposed_ports().is_empty());
    if !spec.exposed_ports().is_empty() {
        effective.exposed_port_count = Some(u16::try_from(spec.exposed_ports().len()).unwrap_or(u16::MAX));
    }
    effective.needs_hardening_overrides = any(
        effective.needs_hardening_overrides,
        !spec.platform().container.is_default(),
    );
    effective.needs_persistent_runtime = any(effective.needs_persistent_runtime, wants_persistent_runtime(spec));
    effective.needs_named_tags = any(effective.needs_named_tags, wants_named_tags(req));
    if !spec.mounts().is_empty() {
        effective.needs_fuse_mounts = CapabilityNeed::Required;
    }
    if let Some(policy) = network_policy {
        effective = with_network_policy(effective, policy);
    }
    if let Some(policy) = fs_policy {
        effective = with_fs_policy(effective, policy);
    }
    effective = with_secret_bindings(effective, secret_bindings);
    effective = with_machine_size(effective, spec);
    effective = with_lease_lifetime(effective, req.ttl_seconds, spec.no_ttl());
    Some(effective)
}
