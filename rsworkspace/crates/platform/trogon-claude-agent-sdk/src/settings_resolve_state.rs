use std::{
    io,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use crate::{
    adapters::{CurrentDirectory, SettingsHost},
    connector_folder::ConnectorFolders,
    connector_vetting::VettingState,
    node_path::NodePathDialect,
    settings_resolve_platform::platform_kind,
    settings_resolve_platform_kind::PlatformKind,
    settings_resolve_reread::{PolicyRereads, PolicyState, Reading},
};

/// What upstream keeps in module state between `resolveSettings` calls of one process.
///
/// [`crate::resolve_settings`] shares one value for the whole process, the way the SDK module does;
/// [`crate::resolve_settings_with`] starts from a fresh one on every call.
#[derive(Debug, Default)]
pub struct ResolverState {
    connector_folders: ConnectorFolders,
    vetting: VettingState,
    launch_directory: OnceLock<Option<PathBuf>>,
    node_path_dialect: NodePathDialect,
    policy: Mutex<PolicyState>,
    rereads: PolicyRereads,
    platform: OnceLock<PlatformKind>,
}

impl ResolverState {
    pub fn new() -> Self {
        Self::default()
    }

    /// A state whose machine policy re-reads run where `rereads` says.
    pub fn with_rereads(rereads: PolicyRereads) -> Self {
        Self {
            rereads,
            ..Self::default()
        }
    }

    /// `getPlatform()` upstream, which keeps its first answer for the life of the process.
    pub(crate) fn platform<H: SettingsHost>(&self, host: &H) -> PlatformKind {
        *self.platform.get_or_init(|| platform_kind(host))
    }

    /// Pins the working directory the SDK module read when it loaded (`XE` upstream), whose ancestry the connector
    /// folder check trusts; `None` stands for a process whose working directory could not be read then. Without it,
    /// the host's working directory at the first resolve that needs it is used.
    #[must_use]
    pub fn with_launch_directory(self, directory: Option<PathBuf>) -> Self {
        Self {
            launch_directory: OnceLock::from(directory),
            ..self
        }
    }

    /// Follows the `path` module of the node release the SDK would run on when the connector folder check reads
    /// paths; [`NodePathDialect::Current`] by default.
    #[must_use]
    pub fn with_node_path_dialect(self, dialect: NodePathDialect) -> Self {
        Self {
            node_path_dialect: dialect,
            ..self
        }
    }

    /// The machine policy reading this call goes on with, re-reading the host when upstream would.
    pub(crate) fn policy_reading<H: SettingsHost>(&self, host: &H) -> Reading {
        let kind = self.platform(host);
        match self.policy.lock() {
            Ok(mut policy) => policy.current(host, kind, self.rereads),
            Err(_) => PolicyState::default().current(host, kind, self.rereads),
        }
    }

    /// The launch directory, read from `host` the first time when none was pinned.
    pub(crate) fn launch_directory<H: CurrentDirectory<Error = io::Error>>(&self, host: &H) -> Option<PathBuf> {
        self.launch_directory.get_or_init(|| host.current_dir().ok()).clone()
    }

    pub(crate) fn node_path_dialect(&self) -> NodePathDialect {
        self.node_path_dialect
    }

    pub(crate) fn connector_folders(&self) -> &ConnectorFolders {
        &self.connector_folders
    }

    pub(crate) fn vetting(&self) -> &VettingState {
        &self.vetting
    }
}
