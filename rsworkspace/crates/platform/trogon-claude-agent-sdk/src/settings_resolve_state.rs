use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use crate::{
    adapters::SettingsHost,
    constants::CONNECTOR_CACHE_LIMIT,
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
    connector_folders: Mutex<HashMap<PathBuf, bool>>,
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

    /// The machine policy reading this call goes on with, re-reading the host when upstream would.
    pub(crate) fn policy_reading<H: SettingsHost>(&self, host: &H) -> Reading {
        let kind = self.platform(host);
        match self.policy.lock() {
            Ok(mut policy) => policy.current(host, kind, self.rereads),
            Err(_) => PolicyState::default().current(host, kind, self.rereads),
        }
    }

    pub(crate) fn connector_folder(&self, directory: &Path) -> Option<bool> {
        self.connector_folders
            .lock()
            .ok()
            .and_then(|folders| folders.get(directory).copied())
    }

    /// Remembers an answer; a full cache forgets every earlier answer first, as the upstream `Map` does.
    pub(crate) fn remember_connector_folder(&self, directory: &Path, ignored: bool) {
        if let Ok(mut folders) = self.connector_folders.lock() {
            if folders.len() >= CONNECTOR_CACHE_LIMIT {
                folders.clear();
            }
            folders.insert(directory.to_path_buf(), ignored);
        }
    }
}
