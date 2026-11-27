/// A key could not be parsed, or the cipher refused to encrypt or decrypt.
#[derive(Debug, thiserror::Error)]
pub enum KmsError {
    #[error("the key is not valid base64")]
    InvalidEncoding(#[source] base64::DecodeError),
    #[error("the key must decode to {expected} bytes, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
}
