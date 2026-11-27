#[derive(Debug, thiserror::Error)]
pub(crate) enum LaunchFingerprintError {
    #[error("command request cannot be fingerprinted: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("command fingerprint must be 64 hexadecimal characters")]
    Malformed,
}
