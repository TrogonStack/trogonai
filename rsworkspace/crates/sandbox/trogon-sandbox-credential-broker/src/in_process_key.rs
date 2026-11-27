use std::fmt;
use std::str::FromStr;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::aead_cipher::{aead_decrypt, aead_encrypt};
use crate::constants::{KEY_LEN, NONCE_LEN};
use crate::{KmsError, KmsProvider};

/// A root key held in process memory.
///
/// It parses from the standard base64 encoding of exactly 32 bytes. `Debug` never shows the key.
#[derive(Clone)]
pub struct InProcessKey {
    key: [u8; KEY_LEN],
}

impl InProcessKey {
    pub fn from_bytes(key: [u8; KEY_LEN]) -> Self {
        Self { key }
    }
}

impl FromStr for InProcessKey {
    type Err = KmsError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let decoded = BASE64.decode(raw.trim()).map_err(KmsError::InvalidEncoding)?;
        let actual = decoded.len();
        let key = <[u8; KEY_LEN]>::try_from(decoded).map_err(|_| KmsError::InvalidKeyLength {
            expected: KEY_LEN,
            actual,
        })?;
        Ok(Self { key })
    }
}

impl fmt::Debug for InProcessKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InProcessKey([REDACTED])")
    }
}

#[async_trait]
impl KmsProvider for InProcessKey {
    async fn encrypt(&self, aad: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, [u8; NONCE_LEN]), KmsError> {
        aead_encrypt(&self.key, aad, plaintext)
    }

    async fn decrypt(&self, aad: &[u8], ciphertext: &[u8], nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, KmsError> {
        aead_decrypt(&self.key, aad, ciphertext, nonce)
    }
}
