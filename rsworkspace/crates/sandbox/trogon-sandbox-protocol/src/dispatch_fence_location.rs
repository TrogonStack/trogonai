use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::constants::FENCE_FILE_NAME;

static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();
static FENCE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Where the registered state directory or fence file puts the journal, if either was registered.
pub(crate) fn default_fence_path() -> Option<PathBuf> {
    FENCE_PATH
        .get()
        .cloned()
        .or_else(|| STATE_DIR.get().map(|dir| dir.join(FENCE_FILE_NAME)))
}

/// For a worker with no state directory: keeps the lease fence in the journal at `path` when one is configured,
/// and in memory when it is `None`. Call it before the first dispatch is decoded; `SANDBOX_DISPATCH_FENCE_FILE`
/// still takes precedence.
pub fn register_fence_file(path: Option<&Path>) {
    if let Some(path) = path.filter(|path| !path.as_os_str().is_empty()) {
        let _ = FENCE_PATH.set(path.to_path_buf());
    }
}

/// Tells the process-wide verifier where the worker keeps its state, so the lease fence survives a restart. Call it
/// before the first dispatch is decoded; `SANDBOX_DISPATCH_FENCE_FILE` overrides the location.
pub fn register_state_dir(dir: &Path) {
    let _ = STATE_DIR.set(dir.to_path_buf());
}
