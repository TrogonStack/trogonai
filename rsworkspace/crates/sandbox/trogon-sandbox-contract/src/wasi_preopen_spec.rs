use serde::{Deserialize, Serialize};

use crate::wasi_preopen_spec_wire::WasiPreopenSpecWire;
use crate::{MountAccess, WasiPreopenSpecError};

/// One directory grant for a WASI guest's filesystem view.
///
/// `host_dir` is a path the worker process can open directly, such as a scratch directory already on disk, and it is
/// never empty. `guest_path` is where the guest sees it, absolute and free of `..` components. `ReadOnly` maps to
/// WASI's read-only permissions, and `ReadWrite` lets the guest create, modify, and delete within the directory.
/// Construction and deserialization both enforce these rules.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "WasiPreopenSpecWire", into = "WasiPreopenSpecWire")]
pub struct WasiPreopenSpec {
    host_dir: String,
    guest_path: String,
    access: MountAccess,
}

impl WasiPreopenSpec {
    pub fn new(
        host_dir: impl Into<String>,
        guest_path: impl Into<String>,
        access: MountAccess,
    ) -> Result<Self, WasiPreopenSpecError> {
        let host_dir = host_dir.into();
        let guest_path = guest_path.into();
        if host_dir.is_empty() {
            return Err(WasiPreopenSpecError::EmptyHostDir);
        }
        if !guest_path.starts_with('/') {
            return Err(WasiPreopenSpecError::RelativeGuestPath);
        }
        if guest_path.split('/').any(|component| component == "..") {
            return Err(WasiPreopenSpecError::ParentComponent);
        }
        Ok(Self {
            host_dir,
            guest_path,
            access,
        })
    }

    pub fn host_dir(&self) -> &str {
        &self.host_dir
    }

    pub fn guest_path(&self) -> &str {
        &self.guest_path
    }

    pub fn access(&self) -> MountAccess {
        self.access
    }
}

impl TryFrom<WasiPreopenSpecWire> for WasiPreopenSpec {
    type Error = WasiPreopenSpecError;

    fn try_from(wire: WasiPreopenSpecWire) -> Result<Self, Self::Error> {
        Self::new(wire.host_dir, wire.guest_path, wire.access)
    }
}

impl From<WasiPreopenSpec> for WasiPreopenSpecWire {
    fn from(spec: WasiPreopenSpec) -> Self {
        Self {
            host_dir: spec.host_dir,
            guest_path: spec.guest_path,
            access: spec.access,
        }
    }
}
