//! What the broker decides about worker placement without trusting the request.

use tracing::warn;
use trogon_sandbox_contract::EnvDigest;
use trogon_sandbox_contract::{CapabilityRequirements, Digest, TenantId};

use crate::core::SnapshotKvReader;

/// The environment digest a restore of `snapshot` must run under, read from the catalog record
/// the snapshot builder wrote from the signed manifest. It replaces whatever the request put in
/// `required_capabilities.env_digest`, so a caller cannot steer a restore onto a worker running
/// a different environment. `None` when nothing is restored, the snapshot is unknown to the
/// catalog, or it carries no digest; a digest the request supplied then stays as the caller's own
/// (narrower) constraint, and the worker still checks the snapshot against its own environment.
pub(crate) async fn derive_env_digest(
    catalog: &dyn SnapshotKvReader,
    tenant: &TenantId,
    snapshot: Option<&Digest>,
) -> Option<EnvDigest> {
    let snapshot = snapshot?;
    match catalog.get_owned(tenant, snapshot).await {
        Ok(record) => record?.env_digest,
        Err(error) => {
            warn!(%snapshot, %error, "could not read the snapshot catalog to derive the environment digest");
            None
        }
    }
}

/// The always-on slice of `required`, or `None` when it asks nothing that must
/// be checked even with the worker registry unenforced: egress enforcement,
/// machine size, an uncapped lifetime, and the fuse and privileged axes, which
/// a worker would otherwise accept and silently not deliver (a drives lease
/// would claim its drive lock and never mount).
pub(crate) fn egress_requirements_only(required: &CapabilityRequirements) -> Option<CapabilityRequirements> {
    let gate = CapabilityRequirements {
        needs_domain_allowlist: required.needs_domain_allowlist,
        needs_cidr_allowlist: required.needs_cidr_allowlist,
        needs_hardening_overrides: required.needs_hardening_overrides,
        needs_native_passthrough: required.needs_native_passthrough,
        env_digest: required.env_digest.clone(),
        needs_fuse_mounts: required.needs_fuse_mounts,
        needs_privileged: required.needs_privileged,
        // One port is covered by `supports_port_preview`; only a request for
        // several ports can outgrow a worker's `ingress.max_exposed_ports`.
        exposed_port_count: required.exposed_port_count.filter(|&count| count > 1),
        min_egress_enforcement: required.min_egress_enforcement,
        vcpu_millis: required.vcpu_millis,
        disk_mib: required.disk_mib,
        // A no-TTL lease on a capped vendor would be killed at the cap, so it
        // is gated like egress. A plain TTL is left to the worker's own check.
        lease_lifetime_seconds: required
            .lease_lifetime_seconds
            .filter(|&need| need == CapabilityRequirements::NO_TTL_LIFETIME),
        ..Default::default()
    };
    (gate != CapabilityRequirements::default()).then_some(gate)
}

#[cfg(test)]
mod tests;
