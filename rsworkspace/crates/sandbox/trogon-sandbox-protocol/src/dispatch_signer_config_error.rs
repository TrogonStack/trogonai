use crate::constants::{ENV_SIGNING_KEY_FILE, ENV_SIGNING_KEY_ID, ENV_SIGNING_TTL_SECS};

/// Why the dispatch signer could not be built from its configuration.
#[derive(Debug, thiserror::Error)]
pub enum DispatchSignerConfigError {
    /// The file named by the signing key file variable could not be read.
    #[error("{ENV_SIGNING_KEY_FILE}: cannot read {path}: {source}")]
    ReadKeyFile {
        /// The file that could not be read.
        path: String,
        /// The read failure.
        source: std::io::Error,
    },
    /// The signing key is not a base64 32-byte seed.
    #[error("dispatch signing key must be a base64 32-byte seed")]
    InvalidSeed,
    /// The key id contains a character that the public key list uses as a separator.
    #[error("{ENV_SIGNING_KEY_ID} must not contain ':' or ','")]
    InvalidKeyId,
    /// The envelope lifetime is not a positive integer number of seconds.
    #[error("{ENV_SIGNING_TTL_SECS} must be a positive integer")]
    InvalidTtl,
}
