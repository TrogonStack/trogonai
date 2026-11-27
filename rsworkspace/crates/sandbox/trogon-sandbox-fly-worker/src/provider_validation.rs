use trogon_sandbox_contract::{
    DriveMountMode, LeasePersistence, LeasePrewarm, LeaseRequest, NetworkPolicy, PrivilegedAccess, ProviderKind,
    SandboxImage, WorkspaceSource,
};

use crate::check::ensure;
use crate::{FlyProvider, FlyProviderError, FlySnapshotRef, FlyVolumeBinding};

impl FlyProvider {
    /// Refuses everything this worker cannot honor, so a lease never starts with options it would ignore.
    pub(crate) fn validate_lease(&self, request: &LeaseRequest) -> Result<Option<&FlyVolumeBinding>, FlyProviderError> {
        let spec = &request.spec;
        self.restore_source(spec.image())?;
        ensure(
            spec.privileged() == PrivilegedAccess::Denied,
            FlyProviderError::Rejected("privileged access is unavailable on Fly"),
        )?;
        ensure(
            matches!(spec.workspace(), WorkspaceSource::Empty(_)) && spec.mounts().is_empty(),
            FlyProviderError::Rejected("workspace sources and remote mounts are unavailable on Fly"),
        )?;
        ensure(
            matches!(spec.network_policy(), None | Some(NetworkPolicy::AllowAll)),
            FlyProviderError::Rejected("network policy is unavailable on Fly"),
        )?;
        ensure(
            spec.fs_policy().is_none_or(|policy| policy.is_empty())
                && spec.secret_bindings().is_empty()
                && spec.platform().is_default()
                && spec.user().is_none(),
            FlyProviderError::Rejected("filesystem policy, secret bindings, platform and user options are unavailable"),
        )?;
        ensure(
            request.ttl_seconds > 0
                && request.persistent == LeasePersistence::Ephemeral
                && request.prewarm == LeasePrewarm::Regular,
            FlyProviderError::Rejected("lease must be ephemeral, not prewarmed and have a ttl"),
        )?;
        if self.config.workspace_volume.is_some()
            && let Some(cwd) = spec.working_dir()
        {
            ensure(
                cwd.starts_with('/') && cwd != "/" && !cwd.contains('\0'),
                FlyProviderError::Rejected("working directory must be an absolute path below the root"),
            )?;
        }
        ensure(
            spec.drives().len() <= 1,
            FlyProviderError::Rejected("only one native volume can be mounted"),
        )?;
        let Some(mount) = spec.drives().first() else {
            return Ok(None);
        };
        let binding = self
            .config
            .volume
            .as_ref()
            .ok_or(FlyProviderError::Rejected("native volume binding is missing"))?;
        ensure(
            request.tenant == mount.drive_id.tenant
                && mount.drive_id == *binding.drive_id()
                && mount.mount_path == *binding.mount_path()
                && mount.mode == DriveMountMode::ReadWrite,
            FlyProviderError::Rejected("requested drive does not match the worker's volume"),
        )?;
        Ok(Some(binding))
    }

    /// The volume snapshot a lease restores from, or `None` when it boots the configured image.
    pub(crate) fn restore_source(&self, image: &SandboxImage) -> Result<Option<String>, FlyProviderError> {
        match image {
            SandboxImage::Registry { reference, .. } if reference.allowlist_string() == self.config.image => Ok(None),
            SandboxImage::NativeSnapshot { reference } if reference.provider == ProviderKind::Fly => {
                ensure(
                    self.config.workspace_volume.is_some(),
                    FlyProviderError::Rejected("native snapshot restore needs a workspace volume"),
                )?;
                let decoded = FlySnapshotRef::decode(reference.id.as_str())?;
                decoded.ensure_restorable(&self.config.region, &self.config.image)?;
                Ok(Some(decoded.snapshot_id().to_owned()))
            }
            _ => Err(FlyProviderError::Rejected(
                "the image is not the one this worker serves",
            )),
        }
    }
}
