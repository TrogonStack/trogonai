/// Why a dispatch public key was refused.
#[derive(Debug, thiserror::Error)]
pub enum DispatchPublicKeyError {
    /// The value is not base64 of exactly 32 bytes.
    #[error("dispatch public key must be base64 of 32 bytes")]
    Shape,
    /// The 32 bytes are not a valid Ed25519 public key.
    #[error("invalid dispatch public key: {0}")]
    Invalid(ed25519_dalek::SignatureError),
}
