//! A pinned command supervisor that the worker installs in a sandbox so a detached command can be signaled, fed
//! input and observed from outside its own process tree.

use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::sync::Arc;

use crate::constants::{BRIDGE, MAX_ARTIFACT_BYTES, MAX_BRIDGE_BYTES};
use crate::guest_architecture::GuestArchitecture;
use crate::guest_platform::GuestPlatform;
use crate::installed_runtime::InstalledRuntime;
use crate::ordinary_command_scripts::{CREATE_ROOT, PLATFORM_PROBE, VERIFY_RUNTIME};
use crate::sha256_pin::Sha256Pin;
use crate::{DaytonaApi, OrdinaryCommandRuntimeError};

/// The supervisor artifact an operator configured, checked once at load.
///
/// It is read without following links, must match its pin byte for byte, and must be a statically linked Linux
/// executable. Each command installs a copy in its sandbox, verifies the copy and probes it before use.
pub struct OrdinaryCommandRuntime {
    artifact: Arc<[u8]>,
    digest: Sha256Pin,
    architecture: GuestArchitecture,
    bridge_digest: Sha256Pin,
}

impl OrdinaryCommandRuntime {
    /// Loads the artifact at `path`, which must hash to `sha256_pin`, a lowercase hex digest.
    pub fn load(path: &Path, sha256_pin: &str) -> Result<Self, OrdinaryCommandRuntimeError> {
        let pin = Sha256Pin::parse(sha256_pin)?;
        let metadata = std::fs::symlink_metadata(path).map_err(OrdinaryCommandRuntimeError::Open)?;
        if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES as u64 {
            return Err(OrdinaryCommandRuntimeError::NotBoundedRegularFile);
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        // A link is refused at open, and a pipe swapped in after the check does not block the open.
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let mut file = options.open(path).map_err(OrdinaryCommandRuntimeError::Open)?;
        let metadata = file.metadata().map_err(OrdinaryCommandRuntimeError::Read)?;
        if !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES as u64 {
            return Err(OrdinaryCommandRuntimeError::NotBoundedRegularFile);
        }
        let mut artifact = Vec::new();
        file.by_ref()
            .take(MAX_ARTIFACT_BYTES as u64 + 1)
            .read_to_end(&mut artifact)
            .map_err(OrdinaryCommandRuntimeError::Read)?;
        if artifact.len() > MAX_ARTIFACT_BYTES || Sha256Pin::of(&artifact) != pin {
            return Err(OrdinaryCommandRuntimeError::PinMismatch);
        }
        if BRIDGE.is_empty() || BRIDGE.len() > MAX_BRIDGE_BYTES {
            return Err(OrdinaryCommandRuntimeError::BridgeInvalid);
        }
        Ok(Self {
            architecture: GuestArchitecture::from_elf(&artifact)?,
            artifact: artifact.into(),
            digest: pin,
            bridge_digest: Sha256Pin::of(BRIDGE.as_bytes()),
        })
    }

    /// Installs the supervisor and bridge in `sandbox_id` under a directory of their own, then verifies both and
    /// probes the supervisor. A failed install removes what it created.
    pub(crate) async fn install(
        &self,
        api: &DaytonaApi,
        sandbox_id: &str,
        execution: &str,
        user: Option<&str>,
    ) -> Result<InstalledRuntime, OrdinaryCommandRuntimeError> {
        let platform = GuestPlatform::parse(&api.run_node(sandbox_id, PLATFORM_PROBE, &[], None).await?)?;
        if platform.architecture != self.architecture {
            return Err(OrdinaryCommandRuntimeError::GuestArchitectureMismatch);
        }
        let nonce = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        let root = format!("/tmp/.trogon-ordinary-{execution}-{nonce}");
        let installed = InstalledRuntime {
            helper: format!("{root}/supervisor"),
            bridge: format!("{root}/bridge.cjs"),
            root,
            digest: self.digest.clone(),
            nonce,
        };
        let created = api
            .run_node(sandbox_id, CREATE_ROOT, &[&installed.root, &installed.nonce], None)
            .await?;
        if created != "created" {
            return Err(OrdinaryCommandRuntimeError::DirectoryUnconfirmed);
        }
        let result = self.place(api, sandbox_id, &installed, user).await;
        if result.is_err() {
            let _ = installed.cleanup(api, sandbox_id).await;
        }
        result.map(|()| installed)
    }

    async fn place(
        &self,
        api: &DaytonaApi,
        sandbox_id: &str,
        installed: &InstalledRuntime,
        user: Option<&str>,
    ) -> Result<(), OrdinaryCommandRuntimeError> {
        api.upload(sandbox_id, &installed.helper, &self.artifact)
            .await
            .map_err(|source| OrdinaryCommandRuntimeError::Upload {
                what: "artifact",
                source,
            })?;
        api.upload(sandbox_id, &installed.bridge, BRIDGE.as_bytes())
            .await
            .map_err(|source| OrdinaryCommandRuntimeError::Upload { what: "bridge", source })?;
        api.set_permissions(sandbox_id, &installed.helper, 0o555)
            .await
            .map_err(|source| OrdinaryCommandRuntimeError::Permissions {
                what: "artifact",
                source,
            })?;
        api.set_permissions(sandbox_id, &installed.bridge, 0o444)
            .await
            .map_err(|source| OrdinaryCommandRuntimeError::Permissions { what: "bridge", source })?;
        let verified = api
            .run_node(
                sandbox_id,
                VERIFY_RUNTIME,
                &[
                    &installed.helper,
                    installed.digest.as_str(),
                    &installed.bridge,
                    self.bridge_digest.as_str(),
                ],
                user,
            )
            .await?;
        if verified == "verified" {
            Ok(())
        } else {
            Err(OrdinaryCommandRuntimeError::ProbeNotExact)
        }
    }
}

#[cfg(test)]
mod tests;
