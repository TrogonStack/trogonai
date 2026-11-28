use crate::constants::{ENV_FENCE_FILE, ENV_PUBLIC_KEYS_FILE, ENV_VERIFY_MODE};
use crate::{DispatchPublicKeyError, EnforcementModeError, EpochModeError};

/// Why the dispatch verifier could not be built from its configuration.
#[derive(Debug, thiserror::Error)]
pub enum DispatchVerifierConfigError {
    /// The verification mode is not off, warn or require.
    #[error(transparent)]
    Mode(#[from] EnforcementModeError),
    /// The epoch mode is not off, warn or require.
    #[error(transparent)]
    Epoch(#[from] EpochModeError),
    /// The public key file could not be read.
    #[error("{ENV_PUBLIC_KEYS_FILE}: cannot read {path}: {source}")]
    KeysFile {
        /// The file that could not be read.
        path: String,
        /// The read failure.
        source: std::io::Error,
    },
    /// A public key entry is not `key_id:base64`.
    #[error("dispatch public key entries look like key_id:base64")]
    KeyEntryMalformed,
    /// A public key was refused.
    #[error(transparent)]
    Key(#[from] DispatchPublicKeyError),
    /// Verification is required but no public key is configured.
    #[error("{ENV_VERIFY_MODE}=require needs at least one public key")]
    RequireWithoutKeys,
    /// The fence journal the operator named could not be opened.
    #[error("{ENV_FENCE_FILE}: cannot open {path}: {source}")]
    FenceFile {
        /// The journal that could not be opened.
        path: String,
        /// The open failure.
        source: std::io::Error,
    },
}
