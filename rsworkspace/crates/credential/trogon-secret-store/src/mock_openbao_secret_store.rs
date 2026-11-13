use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_credential_domain::{
    CredentialIdentity, CredentialKey, CredentialMetadata, CredentialPath, CredentialRef, CredentialStatus,
    CredentialVersion,
};
use trogon_std::SecretString;

use super::{
    SecretDestroyReason, SecretMaterial, SecretStoreDestroy, SecretStoreError, SecretStoreGet, SecretStoreMetadata,
    SecretStorePut, SecretStoreRevoke, SecretStoreRotate, fingerprint,
};

#[derive(Clone, Default)]
pub struct MockOpenBaoSecretStore {
    state: Arc<Mutex<MockOpenBaoSecretStoreState>>,
}

#[derive(Default)]
struct MockOpenBaoSecretStoreState {
    versions: BTreeMap<CredentialRef, MockOpenBaoVersion>,
}

#[derive(Clone)]
struct MockOpenBaoVersion {
    material: Option<SecretMaterial>,
    metadata: CredentialMetadata,
    destroy_reason: Option<SecretDestroyReason>,
}

impl MockOpenBaoSecretStore {
    pub fn data_path(key: &CredentialKey) -> String {
        format!("secret/data/{}", CredentialPath::new(key.clone()))
    }

    pub fn metadata_path(key: &CredentialKey) -> String {
        format!("secret/metadata/{}", CredentialPath::new(key.clone()))
    }
}

impl MockOpenBaoSecretStoreState {
    fn next_credential_ref(&mut self, identity: CredentialIdentity) -> CredentialRef {
        let key = identity.key();
        let current = self
            .versions
            .keys()
            .filter(|credential| credential.key() == key)
            .map(CredentialRef::version)
            .max();
        let version = current.map_or_else(CredentialVersion::initial, CredentialVersion::next);

        CredentialRef::new(identity, version)
    }
}

impl SecretStorePut for MockOpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn put(&self, identity: CredentialIdentity, value: SecretString) -> Result<CredentialRef, Self::Error> {
        let mut state = self.state.lock().await;
        let credential = state.next_credential_ref(identity);
        let active_metadata = metadata(&credential, CredentialStatus::Active)?;
        for (stored_ref, stored) in &mut state.versions {
            if stored_ref.key() == credential.key() && stored.metadata.status() == CredentialStatus::Active {
                stored.metadata = metadata(stored_ref, CredentialStatus::Previous)?;
            }
        }
        state.versions.insert(
            credential.clone(),
            MockOpenBaoVersion {
                material: Some(SecretMaterial::plaintext(value)),
                metadata: active_metadata,
                destroy_reason: None,
            },
        );
        Ok(credential)
    }
}

impl SecretStoreGet for MockOpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn get(&self, credential: &CredentialRef) -> Result<SecretMaterial, Self::Error> {
        let state = self.state.lock().await;
        let stored = state
            .versions
            .get(credential)
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })?;
        let status = stored.metadata.status();
        match &stored.material {
            Some(material) if status.is_readable() => Ok(material.clone()),
            _ => Err(SecretStoreError::Unreadable {
                credential: credential.clone(),
                status,
            }),
        }
    }
}

impl SecretStoreRotate for MockOpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn rotate(&self, credential: &CredentialRef, value: SecretString) -> Result<CredentialRef, Self::Error> {
        let mut state = self.state.lock().await;
        let stored = state
            .versions
            .get_mut(credential)
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })?;
        let status = stored.metadata.status();
        if !status.is_writable() {
            return Err(SecretStoreError::Unwritable {
                credential: credential.clone(),
                status,
            });
        }
        stored.metadata = metadata(credential, CredentialStatus::Previous)?;

        let new_credential = credential.next_version();
        let metadata = metadata(&new_credential, CredentialStatus::Active)?;
        state.versions.insert(
            new_credential.clone(),
            MockOpenBaoVersion {
                material: Some(SecretMaterial::plaintext(value)),
                metadata,
                destroy_reason: None,
            },
        );

        Ok(new_credential)
    }
}

impl SecretStoreRevoke for MockOpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn revoke(&self, credential: &CredentialRef) -> Result<(), Self::Error> {
        let mut state = self.state.lock().await;
        let stored = state
            .versions
            .get(credential)
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })?;
        let status = stored.metadata.status();
        if status == CredentialStatus::Destroyed {
            return Err(SecretStoreError::Unwritable {
                credential: credential.clone(),
                status,
            });
        }
        for (stored_ref, stored) in &mut state.versions {
            if stored_ref.key() == credential.key() && stored.metadata.status() != CredentialStatus::Destroyed {
                stored.metadata = metadata(stored_ref, CredentialStatus::Revoked)?;
            }
        }
        Ok(())
    }
}

impl SecretStoreDestroy for MockOpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn destroy(&self, credential: &CredentialRef, reason: &SecretDestroyReason) -> Result<(), Self::Error> {
        let mut state = self.state.lock().await;
        let stored = state
            .versions
            .get_mut(credential)
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })?;
        if stored.metadata.status() == CredentialStatus::Destroyed {
            return Ok(());
        }
        stored.material = None;
        stored.destroy_reason = Some(reason.clone());
        stored.metadata = metadata(credential, CredentialStatus::Destroyed)?;
        Ok(())
    }
}

impl SecretStoreMetadata for MockOpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        let state = self.state.lock().await;
        state
            .versions
            .get(credential)
            .map(|stored| stored.metadata.clone())
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })
    }
}

fn metadata(credential: &CredentialRef, status: CredentialStatus) -> Result<CredentialMetadata, SecretStoreError> {
    let fingerprint = fingerprint::digest(
        "openbao",
        format!(
            "{}#{}",
            MockOpenBaoSecretStore::metadata_path(&credential.key()),
            credential.version().get()
        ),
    )
    .map_err(SecretStoreError::invalid_generated)?;

    Ok(CredentialMetadata::new(credential.clone(), status, fingerprint))
}

#[cfg(test)]
mod tests;
