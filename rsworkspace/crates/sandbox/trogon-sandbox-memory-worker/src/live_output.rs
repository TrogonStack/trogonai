use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use crate::mutex_ext::MutexExt;

/// What a running command has made visible so far: the output it has emitted
/// and the simulated TCP ports it listens on. Readiness probes of a named
/// process read this while the command still runs.
#[derive(Clone, Debug, Default)]
pub struct LiveOutput {
    text: Arc<Mutex<Vec<u8>>>,
    ports: Arc<Mutex<BTreeSet<u16>>>,
}

impl LiveOutput {
    /// Makes `text` visible to output probes before the command finishes.
    pub fn emit(&self, text: &str) {
        self.text.lock_clean().extend_from_slice(text.as_bytes());
    }

    /// Marks `port` as accepting connections.
    pub fn listen(&self, port: u16) {
        self.ports.lock_clean().insert(port);
    }

    /// Whether the emitted output contains `needle`. An empty needle never matches.
    pub fn contains(&self, needle: &[u8]) -> bool {
        contains_bytes(&self.text.lock_clean(), needle)
    }

    /// Whether `port` was marked as accepting connections.
    pub fn is_listening(&self, port: u16) -> bool {
        self.ports.lock_clean().contains(&port)
    }
}

/// Whether `haystack` contains `needle`. An empty needle never matches.
pub(crate) fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}
