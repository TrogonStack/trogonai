/// Why a preview token was refused or could not be made.
#[derive(Debug, thiserror::Error)]
pub enum PreviewTokenError {
    #[error("malformed preview token")]
    Malformed,
    #[error("unknown preview signing key id")]
    UnknownKey,
    #[error("invalid preview token signature")]
    BadSignature,
    #[error("preview token expired")]
    Expired,
    #[error("preview token was issued for another lease")]
    WrongLease,
    #[error("preview token was issued for another port")]
    WrongPort,
    #[error("preview claims could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("preview signing key is unusable: {0}")]
    Key(#[from] hmac::digest::InvalidLength),
}
