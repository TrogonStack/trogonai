use async_trait::async_trait;

use crate::KmsError;

/// Length of the nonce that accompanies every ciphertext, in bytes.
pub const NONCE_LEN: usize = 12;

/// Root-key operations.
///
/// `aad` is authenticated but not stored: decryption fails unless the caller presents the same bytes. An empty
/// `aad` is the behavior of records written before the envelope format, and is only used to read them.
#[async_trait]
pub trait KmsProvider: Send + Sync {
    async fn encrypt(&self, aad: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, [u8; NONCE_LEN]), KmsError>;

    async fn decrypt(&self, aad: &[u8], ciphertext: &[u8], nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, KmsError>;
}
