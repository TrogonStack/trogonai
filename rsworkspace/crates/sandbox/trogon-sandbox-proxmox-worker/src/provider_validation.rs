use trogon_sandbox_contract::{
    LeasePersistence, LeasePrewarm, LeaseRequest, NetworkPolicy, SandboxImage, WorkspaceSource,
};

use crate::ProxmoxProvider;
use crate::ProxmoxProviderError;
use crate::check::ensure;

impl ProxmoxProvider {
    /// Refuses everything this worker cannot honor, so a lease never starts with options it would ignore.
    pub(crate) fn validate_lease(&self, request: &LeaseRequest) -> Result<(), ProxmoxProviderError> {
        let spec = &request.spec;
        ensure(
            matches!(spec.image(), SandboxImage::Registry { reference, .. } if reference.allowlist_string() == self.config.image),
            ProxmoxProviderError::Rejected("image must match the configured Proxmox template"),
        )?;
        ensure(
            matches!(spec.workspace(), WorkspaceSource::Empty(_)),
            ProxmoxProviderError::Rejected("only empty workspaces are supported"),
        )?;
        ensure(
            spec.mounts().is_empty() && spec.drives().is_empty() && spec.exposed_ports().is_empty(),
            ProxmoxProviderError::Rejected("mounts, drives and exposed ports are unavailable"),
        )?;
        ensure(
            matches!(spec.network_policy(), None | Some(NetworkPolicy::DenyAll)) || self.config.network.is_some(),
            ProxmoxProviderError::Rejected("network policy requires a configured proxy network"),
        )?;
        ensure(
            spec.user().is_none(),
            ProxmoxProviderError::Rejected("lease user overrides are unavailable"),
        )?;
        ensure(
            spec.platform().is_default()
                && spec.fs_policy().is_none_or(|policy| policy.is_empty())
                && spec.secret_bindings().is_empty(),
            ProxmoxProviderError::Rejected("platform options, filesystem policies and secret bindings are unavailable"),
        )?;
        ensure(
            request.ttl_seconds > 0
                && request.persistent == LeasePersistence::Ephemeral
                && request.prewarm == LeasePrewarm::Regular,
            ProxmoxProviderError::Rejected("lease must be ephemeral, not prewarmed and have a ttl"),
        )
    }
}
