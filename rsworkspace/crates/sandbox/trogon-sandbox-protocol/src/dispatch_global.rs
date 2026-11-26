use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use trogon_sandbox_contract::WorkerId;

use crate::dispatch_auth_constants::{ENV_VERIFY_MODE, FENCE_FILE_NAME, OP_LEASE_DISPATCH};
use crate::{DispatchAuthError, DispatchDecodeError, DispatchVerifier, EnforcementMode};

static GLOBAL: OnceLock<DispatchVerifier> = OnceLock::new();
static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();
static FENCE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Where the registered state directory or fence file puts the journal, if either was registered.
pub(crate) fn default_fence_path() -> Option<PathBuf> {
    FENCE_PATH
        .get()
        .cloned()
        .or_else(|| STATE_DIR.get().map(|dir| dir.join(FENCE_FILE_NAME)))
}

/// Checks a reassembled chunked body against the signature on its message, under the process-wide verifier.
pub fn check_reassembled(headers: &async_nats::HeaderMap, body: &[u8]) -> Result<(), DispatchAuthError> {
    global_verifier().check_reassembled(headers, body)
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

/// The process-wide verifier, built from the environment on first use. A configuration error fails closed when
/// `require` was asked for.
pub fn global_verifier() -> &'static DispatchVerifier {
    GLOBAL.get_or_init(|| match DispatchVerifier::from_env() {
        Ok(verifier) => verifier,
        Err(error) => {
            let required =
                std::env::var(ENV_VERIFY_MODE).is_ok_and(|value| value.trim().eq_ignore_ascii_case("require"));
            tracing::error!(%error, required, "dispatch verification is misconfigured");
            DispatchVerifier::new(if required {
                EnforcementMode::Require
            } else {
                EnforcementMode::Warn
            })
        }
    })
}

/// Worker hook: decodes a lease dispatch like `serde_json::from_slice`, after checking the broker's signature
/// under the process-wide verifier.
pub fn decode_verified<T: serde::de::DeserializeOwned>(
    payload: &[u8],
    own_worker: Option<&WorkerId>,
) -> Result<T, DispatchDecodeError> {
    global_verifier().decode(payload, OP_LEASE_DISPATCH, own_worker)
}
