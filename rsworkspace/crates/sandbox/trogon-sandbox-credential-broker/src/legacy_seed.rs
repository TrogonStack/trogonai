use std::error::Error;
use std::time::SystemTime;

use trogon_sandbox_contract::{CredentialId, CredentialProvider};

use crate::constants::FORMAT_LEGACY;
use crate::stored_credential::StoredCredential;
use crate::{CredentialMaterial, InMemoryCredentialStore};

/// Writes a record exactly as the format before envelopes did: no associated data and no envelope.
pub(crate) async fn seed_legacy(
    store: &InMemoryCredentialStore,
    provider: CredentialProvider,
    id: &str,
    material: &CredentialMaterial,
) -> Result<(), Box<dyn Error>> {
    let (ciphertext, nonce) = store.codec.kms.encrypt(&[], &serde_json::to_vec(material)?).await?;
    store.credentials.lock().await.insert(
        (provider, CredentialId::new(id)),
        StoredCredential {
            version: 1,
            last_rotated: SystemTime::now(),
            nonce,
            ciphertext,
            format_version: FORMAT_LEGACY,
            envelope: None,
        },
    );
    Ok(())
}
