//! Refuses a lease that needs something the worker does not advertise. The
//! broker already places leases by capability; this is the same check made
//! again where the lease would run, so a request that reaches the worker by
//! another path is not served with less enforcement than it asked for.

use anyhow::{bail, ensure};
use trogon_sandbox_contract::{LeaseRequest, NetworkPolicy, ProviderCapabilityProfile};

use crate::network_policy_needs::NetworkPolicyNeeds;
use crate::profile_satisfaction::{ProfileSatisfies, with_machine_size};

/// What network a worker can serve besides none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Egress {
    /// Every lease runs with the network denied.
    Denied,
    /// A lease may also name a domain allowlist, served by the lease proxy
    /// the lease is fenced to, and a filesystem policy, served by the fence helper.
    /// Nothing that needs the proxy to read the traffic is served.
    DomainAllowlist,
    /// As `DomainAllowlist`, and the proxy may also terminate TLS with a CA
    /// the lease trusts. Secret substitution is still refused.
    DomainAllowlistTerminatingTls,
}

pub fn admit(req: &LeaseRequest, profile: &ProviderCapabilityProfile) -> anyhow::Result<()> {
    admit_with(req, profile, Egress::Denied)
}

pub fn admit_with(req: &LeaseRequest, profile: &ProviderCapabilityProfile, egress: Egress) -> anyhow::Result<()> {
    let spec = &req.spec;
    ensure!(
        !spec.privileged().is_granted(),
        "privileged workloads need an isolation tier this worker does not provide"
    );
    ensure!(
        spec.user().is_none(),
        "this worker cannot run the payload as another user"
    );
    ensure!(
        spec.mounts().is_empty() && spec.drives().is_empty(),
        "this worker cannot mount remote storage into the sandbox"
    );
    match (spec.network_policy(), egress) {
        (None | Some(NetworkPolicy::DenyAll), _) => {}
        (Some(policy @ NetworkPolicy::UserDefined { .. }), Egress::DomainAllowlist) => {
            ensure!(
                !policy.requires_tls_intercept() && !policy.requires_secret_substitution(),
                "this worker cannot terminate TLS or substitute secrets; the lease asks for a rule that does"
            );
        }
        (Some(policy @ NetworkPolicy::UserDefined { .. }), Egress::DomainAllowlistTerminatingTls) => {
            ensure!(
                !policy.requires_secret_substitution(),
                "this worker cannot substitute secrets; the lease asks for a rule that does"
            );
        }
        (Some(NetworkPolicy::AllowAll), Egress::DomainAllowlist | Egress::DomainAllowlistTerminatingTls) => {
            bail!("this worker only serves a network policy with a domain allowlist, not open network access")
        }
        (Some(_), _) => {
            bail!("this worker denies all network access; the lease asks for a network policy")
        }
    }
    ensure!(
        egress != Egress::Denied || spec.fs_policy().is_none_or(|policy| policy.is_empty()),
        "this worker cannot enforce a filesystem policy"
    );
    ensure!(
        spec.secret_bindings().is_empty(),
        "this worker cannot serve secret bindings"
    );
    ensure!(
        spec.platform().macos.is_default() || cfg!(target_os = "macos"),
        "this worker cannot enforce the macOS options of the lease"
    );
    ensure!(
        spec.platform().linux.is_default() || cfg!(target_os = "linux"),
        "this worker cannot enforce the Linux options of the lease"
    );
    ensure!(
        spec.platform().git_safe_directories.is_empty() || !cfg!(windows),
        "this worker cannot set git safe directories"
    );
    if let Some(tier) = req.isolation_tier {
        ensure!(
            profile.isolation.supported_tiers.contains(&tier),
            "this worker does not provide the {tier:?} isolation tier"
        );
    }
    let required = with_machine_size(req.required_capabilities.clone().unwrap_or_default(), spec);
    ensure!(
        profile.satisfies(&required),
        "the lease requires capabilities this worker does not advertise"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
