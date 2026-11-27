use std::sync::Arc;
use std::time::SystemTime;

use trogon_sandbox_contract::{CredentialId, CredentialProvider, TenantId};

use crate::constants::FORMAT_ENVELOPE;
use crate::envelope::Envelope;
use crate::record_binding::RecordBinding;
use crate::stored_credential::StoredCredential;
use crate::{CredentialMaterial, CredentialStoreError, KmsProvider};

/// Seals and opens records for every store implementation.
pub(crate) struct RecordCodec {
    pub(crate) kms: Arc<dyn KmsProvider>,
    pub(crate) allow_legacy: bool,
}

impl RecordCodec {
    pub(crate) fn new(kms: Arc<dyn KmsProvider>) -> Self {
        Self {
            kms,
            allow_legacy: true,
        }
    }

    pub(crate) async fn seal(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        material: &CredentialMaterial,
        version: u32,
    ) -> Result<StoredCredential, CredentialStoreError> {
        let plaintext = serde_json::to_vec(material)?;
        let binding = RecordBinding {
            provider,
            tenant,
            credential_id,
            version,
        };
        let (envelope, ciphertext, nonce) = Envelope::seal(self.kms.as_ref(), &binding, &plaintext).await?;
        Ok(StoredCredential {
            version,
            last_rotated: SystemTime::now(),
            nonce,
            ciphertext,
            format_version: FORMAT_ENVELOPE,
            envelope: Some(envelope),
        })
    }

    pub(crate) async fn open(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        credential_id: &CredentialId,
        stored: &StoredCredential,
    ) -> Result<CredentialMaterial, CredentialStoreError> {
        let plaintext = if stored.is_legacy() {
            if !self.allow_legacy {
                return Err(CredentialStoreError::LegacyDisabled);
            }
            self.kms.decrypt(&[], &stored.ciphertext, &stored.nonce).await?
        } else {
            let envelope = stored.envelope.as_ref().ok_or(CredentialStoreError::MissingEnvelope)?;
            let binding = RecordBinding {
                provider,
                tenant,
                credential_id,
                version: stored.version,
            };
            envelope
                .open(self.kms.as_ref(), &binding, &stored.ciphertext, &stored.nonce)
                .await?
        };
        Ok(serde_json::from_slice(&plaintext)?)
    }

    /// Re-seals a record under the envelope format with the next version.
    ///
    /// An envelope record is always opened under its own recorded tenant, and `tenant`, when given, must match
    /// it. A legacy record needs `tenant` because it records none.
    pub(crate) async fn reseal(
        &self,
        provider: CredentialProvider,
        tenant: Option<&TenantId>,
        credential_id: &CredentialId,
        stored: &StoredCredential,
    ) -> Result<StoredCredential, CredentialStoreError> {
        let owner = match (&stored.envelope, tenant) {
            (Some(envelope), Some(tenant)) if &envelope.tenant != tenant => {
                return Err(CredentialStoreError::TenantMismatch);
            }
            (Some(envelope), _) => envelope.tenant.clone(),
            (None, Some(tenant)) => tenant.clone(),
            (None, None) => return Err(CredentialStoreError::LegacyTenantRequired),
        };
        let material = self.open(provider, &owner, credential_id, stored).await?;
        self.seal(
            provider,
            &owner,
            credential_id,
            &material,
            stored.version.saturating_add(1),
        )
        .await
    }
}
