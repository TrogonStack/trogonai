use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    adapters::SettingsHost,
    constants::CONNECTOR_CACHE_LIMIT,
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

    /// The machine policy reading this call goes on with, re-reading the host when upstream would.
    pub(crate) fn policy_reading<H: SettingsHost>(&self, host: &H) -> Reading {
        match self.policy.lock() {
            Ok(mut policy) => policy.current(host, self.rereads),
            Err(_) => PolicyState::default().current(host, self.rereads),
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
