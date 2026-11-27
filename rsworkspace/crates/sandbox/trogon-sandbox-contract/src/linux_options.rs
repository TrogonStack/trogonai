use serde::{Deserialize, Serialize};

use crate::{AllUnixSocketsAccess, NestedSandboxMode, NestedUserNamespaceAccess};

/// What a lease opts into on Linux, each off by default.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinuxOptions {
    /// Run inside an unprivileged container, where a fresh `/proc` cannot be mounted.
    #[serde(default)]
    pub weaker_nested_sandbox: NestedSandboxMode,
    /// Creating `AF_UNIX` and `AF_VSOCK` sockets, which reaches every socket file visible in the sandbox.
    #[serde(default)]
    pub allow_all_unix_sockets: AllUnixSocketsAccess,
    /// Creating user and mount namespaces and mounting inside them.
    #[serde(default)]
    pub allow_nested_user_namespaces: NestedUserNamespaceAccess,
}

impl LinuxOptions {
    /// Reports whether nothing is opted into, so the wire can leave the field out.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
