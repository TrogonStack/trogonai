use serde::{Deserialize, Serialize};

use crate::{ContainerOverrides, FilesystemOptions, LinuxOptions, LiteralPath, MacosOptions};

/// Opt-ins that only some platforms can enforce.
///
/// A worker that cannot enforce a set option refuses the lease. Every group is left out of the wire when it holds
/// its defaults, so an empty object reads as no opt-ins.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlatformOptions {
    #[serde(default, skip_serializing_if = "MacosOptions::is_default")]
    pub macos: MacosOptions,
    #[serde(default, skip_serializing_if = "LinuxOptions::is_default")]
    pub linux: LinuxOptions,
    #[serde(default, skip_serializing_if = "FilesystemOptions::is_default")]
    pub filesystem: FilesystemOptions,
    /// Directories git treats as safe even though another user owns them inside the sandbox (`safe.directory`).
    /// This does not grant write.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub git_safe_directories: Vec<LiteralPath>,
    /// Per-lease container hardening overrides, for a worker that runs payloads in containers.
    #[serde(default, skip_serializing_if = "ContainerOverrides::is_default")]
    pub container: ContainerOverrides,
}

impl PlatformOptions {
    /// Reports whether nothing is opted into, so the wire can leave the field out.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
