use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::TenantId;

use crate::aead_cipher::{aead_decrypt, aead_encrypt, random_bytes};
use crate::constants::{KEY_LEN, NONCE_LEN};
use crate::record_binding::{RecordBinding, push_field};
use crate::{KmsError, KmsProvider};

/// AAD for wrapping a data key under the root key: binds the wrap to the tenant.
fn wrap_aad(tenant: &TenantId) -> Vec<u8> {
    let mut out = Vec::new();
    push_field(&mut out, b"sandbox.credential.wrap.v2");
    push_field(&mut out, tenant.as_str().as_bytes());
    out
}

/// The wrapped data key stored next to a record.
///
/// `tenant` is informational (re-encryption reads it). It is authenticated through the wrap AAD, so editing it
/// makes the record unreadable instead of re-attributing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Envelope {
    pub(crate) tenant: TenantId,
    pub(crate) wrapped_dek: Vec<u8>,
    pub(crate) dek_nonce: [u8; NONCE_LEN],
}

impl Envelope {
    /// Encrypts `plaintext` under a fresh random data key. The root key wraps the data key with the tenant as
    /// AAD, and the payload is bound to `binding`.
    pub(crate) async fn seal(
        kms: &dyn KmsProvider,
        binding: &RecordBinding<'_>,
        plaintext: &[u8],
    ) -> Result<(Self, Vec<u8>, [u8; NONCE_LEN]), KmsError> {
        let dek = random_bytes::<KEY_LEN>();
        let (wrapped_dek, dek_nonce) = kms.encrypt(&wrap_aad(binding.tenant), &dek).await?;
        let (ciphertext, nonce) = aead_encrypt(&dek, &binding.data_aad(), plaintext)?;
        let envelope = Self {
            tenant: binding.tenant.clone(),
            wrapped_dek,
            dek_nonce,
        };
        Ok((envelope, ciphertext, nonce))
    }

    pub(crate) async fn open(
        &self,
        kms: &dyn KmsProvider,
        binding: &RecordBinding<'_>,
        ciphertext: &[u8],
        nonce: &[u8; NONCE_LEN],
    ) -> Result<Vec<u8>, KmsError> {
        let unwrapped = kms
            .decrypt(&wrap_aad(binding.tenant), &self.wrapped_dek, &self.dek_nonce)
            .await?;
        let dek = <[u8; KEY_LEN]>::try_from(unwrapped).map_err(|_| KmsError::Decrypt)?;
        aead_decrypt(&dek, &binding.data_aad(), ciphertext, nonce)
    }
}
