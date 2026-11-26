use async_trait::async_trait;

use crate::{InProcessKey, KmsError, KmsProvider, NONCE_LEN};

/// A root-key provider that delegates to an [`InProcessKey`].
///
/// It marks the seam where a managed key service plugs in: callers hold a [`KmsProvider`], so swapping the
/// backing key service changes no call site.
pub struct EnvelopeKms {
    inner: InProcessKey,
}

impl EnvelopeKms {
    pub fn new(inner: InProcessKey) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl KmsProvider for EnvelopeKms {
    async fn encrypt(&self, aad: &[u8], plaintext: &[u8]) -> Result<(Vec<u8>, [u8; NONCE_LEN]), KmsError> {
        self.inner.encrypt(aad, plaintext).await
    }

    async fn decrypt(&self, aad: &[u8], ciphertext: &[u8], nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, KmsError> {
        self.inner.decrypt(aad, ciphertext, nonce).await
    }
}
