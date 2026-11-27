use serde::{Deserialize, Serialize};

use crate::{MountAccess, MountPath, RemoteMountSource};

/// A remote backend a provider mounts into the sandbox at `mount_path`.
///
/// The mount path is absolute, never the root, and free of `..` components. A mount is writable unless `readonly` is
/// set, and an omitted `readonly` reads as writable.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RemoteMount {
    pub source: RemoteMountSource,
    pub mount_path: MountPath,
    #[serde(default, rename = "readonly")]
    pub access: MountAccess,
}
