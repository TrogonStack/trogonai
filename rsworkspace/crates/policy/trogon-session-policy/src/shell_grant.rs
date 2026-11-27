use std::num::NonZeroU32;

use trogon_sandbox_contract::{
    CapabilityRequirements, EgressEnforcementLevel, IsolationTier, LeaseRequest, LeaseTtl, MonitoringRetention,
    NetworkPolicy, SandboxImage, SandboxSpec, SnapshotCaptureAccess, WorkspaceSource,
};

use crate::{Limits, PolicyError, ReviewPolicy};

/// The worker must enforce this grant before starting a raw shell.
#[derive(Debug, Clone)]
pub struct ShellGrant {
    pub(crate) isolation: IsolationTier,
    pub(crate) network: NetworkPolicy,
    pub(crate) limits: Limits,
    pub(crate) review: ReviewPolicy,
}

impl ShellGrant {
    pub fn isolation(&self) -> IsolationTier {
        self.isolation
    }
    pub fn network(&self) -> &NetworkPolicy {
        &self.network
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Use these requirements on the real broker request, never advisory placement.
    pub fn required_capabilities(&self) -> CapabilityRequirements {
        CapabilityRequirements {
            min_isolation_tier: Some(self.isolation),
            min_egress_enforcement: Some(EgressEnforcementLevel::Mandatory),
            ..CapabilityRequirements::default()
        }
    }

    /// Constrain an administrative image template before sending it to the real broker.
    /// A worker adapter must additionally enforce the process ceiling and command deadline.
    pub fn constrain_lease(&self, mut request: LeaseRequest) -> Result<LeaseRequest, PolicyError> {
        let mut parts = request.spec.into_parts();
        let pinned_image = matches!(&parts.image, SandboxImage::Registry { reference, pull_secret: None } if reference.digest.is_some());
        if !pinned_image
            || !matches!(parts.workspace, WorkspaceSource::Empty(_))
            || !parts.mounts.is_empty()
            || !parts.drives.is_empty()
            || !parts.exposed_ports.is_empty()
            || parts.privileged.is_granted()
            || parts.wasi_capabilities.is_some()
            || !parts.secret_bindings.is_empty()
            || !parts.platform.is_default()
            || !parts.env.is_empty()
            || request.native.is_some()
            || !request.host_imports.is_empty()
            || request.name.is_some()
            || request.prewarm.is_prewarm()
        {
            return Err(PolicyError::ShellLease);
        }
        parts.cpu_seconds =
            NonZeroU32::new(parts.cpu_seconds.get().min(self.limits.command_seconds())).ok_or(PolicyError::Limits)?;
        parts.memory_mib =
            NonZeroU32::new(parts.memory_mib.get().min(self.limits.memory_mib())).ok_or(PolicyError::Limits)?;
        parts.network_policy = Some(self.network.clone());
        parts.no_ttl = LeaseTtl::Bounded;
        parts.snapshot_out = None;
        parts.retention.monitoring = MonitoringRetention::Discarded;
        parts.retention.snapshots = SnapshotCaptureAccess::Refused;
        request.ttl_seconds = request.ttl_seconds.max(1).min(self.limits.command_seconds());
        let mut capabilities = request.required_capabilities.unwrap_or_default();
        if request.isolation_tier == Some(IsolationTier::WasmMicroVm)
            || capabilities.min_isolation_tier == Some(IsolationTier::WasmMicroVm)
        {
            return Err(PolicyError::ShellLease);
        }
        let isolation = if self.isolation == IsolationTier::FullVm
            || request.isolation_tier == Some(IsolationTier::FullVm)
            || capabilities.min_isolation_tier == Some(IsolationTier::FullVm)
        {
            IsolationTier::FullVm
        } else {
            IsolationTier::MicroVm
        };
        request.isolation_tier = Some(isolation);
        capabilities.min_isolation_tier = Some(isolation);
        capabilities.min_egress_enforcement = Some(EgressEnforcementLevel::Mandatory);
        request.required_capabilities = Some(capabilities);
        request.spec = SandboxSpec::new(parts)?;
        Ok(request)
    }
}
