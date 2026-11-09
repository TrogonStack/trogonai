use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_std::SecretString;

use super::{
    SecretDestroyReason, SecretMaterial, SecretStoreDestroy, SecretStoreError, SecretStoreGet, SecretStoreMetadata,
    SecretStorePut, SecretStoreRevoke, SecretStoreRotate, fingerprint,
};
use crate::credential::commands::domain::{
    CredentialId, CredentialKind, CredentialMetadata, CredentialRef, CredentialScope, CredentialStatus,
    CredentialVersion, StorageBackend,
};

#[derive(Clone, Default)]
pub struct InMemorySecretStore {
    state: Arc<Mutex<InMemorySecretStoreState>>,
}

#[derive(Default)]
struct InMemorySecretStoreState {
    next_id: u64,
    entries: BTreeMap<CredentialRef, StoredCredential>,
}

#[derive(Clone)]
struct StoredCredential {
    material: Option<SecretMaterial>,
    metadata: CredentialMetadata,
    destroy_reason: Option<SecretDestroyReason>,
}

impl InMemorySecretStoreState {
    fn next_credential_ref(
        &mut self,
        scope: CredentialScope,
        kind: CredentialKind,
    ) -> Result<CredentialRef, SecretStoreError> {
        self.next_id += 1;
        let id = CredentialId::new(format!(
            "memory:{}:{}:{}",
            scope.scope_key(),
            kind.as_str(),
            self.next_id
        ))
        .map_err(SecretStoreError::invalid_generated(StorageBackend::InMemory))?;

        Ok(CredentialRef::new(id, CredentialVersion::initial(), &scope, kind))
    }
}

impl SecretStorePut for InMemorySecretStore {
    type Error = SecretStoreError;

    async fn put(
        &self,
        scope: CredentialScope,
        kind: CredentialKind,
        value: SecretString,
    ) -> Result<CredentialRef, Self::Error> {
        let mut state = self.state.lock().await;
        let credential = state.next_credential_ref(scope, kind)?;
        let metadata = metadata(&credential, CredentialStatus::Active)?;
        state.entries.insert(
            credential.clone(),
            StoredCredential {
                material: Some(SecretMaterial::plaintext(value)),
                metadata,
                destroy_reason: None,
            },
        );
        Ok(credential)
    }
}

impl SecretStoreGet for InMemorySecretStore {
    type Error = SecretStoreError;

    async fn get(&self, credential: &CredentialRef) -> Result<SecretMaterial, Self::Error> {
        let state = self.state.lock().await;
        let stored = state.entries.get(credential).ok_or_else(|| SecretStoreError::Missing {
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

impl SecretStoreRotate for InMemorySecretStore {
    type Error = SecretStoreError;

    async fn rotate(&self, credential: &CredentialRef, value: SecretString) -> Result<CredentialRef, Self::Error> {
        let mut state = self.state.lock().await;
        let stored = state
            .entries
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
        state.entries.insert(
            new_credential.clone(),
            StoredCredential {
                material: Some(SecretMaterial::plaintext(value)),
                metadata,
                destroy_reason: None,
            },
        );

        Ok(new_credential)
    }
}

impl SecretStoreRevoke for InMemorySecretStore {
    type Error = SecretStoreError;

    async fn revoke(&self, credential: &CredentialRef) -> Result<(), Self::Error> {
        let mut state = self.state.lock().await;
        let stored = state.entries.get(credential).ok_or_else(|| SecretStoreError::Missing {
            credential: credential.clone(),
        })?;
        let status = stored.metadata.status();
        if status == CredentialStatus::Destroyed {
            return Err(SecretStoreError::Unwritable {
                credential: credential.clone(),
                status,
            });
        }
        for (stored_ref, stored) in &mut state.entries {
            if stored_ref.id() == credential.id() && stored.metadata.status() != CredentialStatus::Destroyed {
                stored.metadata = metadata(stored_ref, CredentialStatus::Revoked)?;
            }
        }
        Ok(())
    }
}

impl SecretStoreDestroy for InMemorySecretStore {
    type Error = SecretStoreError;

    async fn destroy(&self, credential: &CredentialRef, reason: &SecretDestroyReason) -> Result<(), Self::Error> {
        let mut state = self.state.lock().await;
        let stored = state
            .entries
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

impl SecretStoreMetadata for InMemorySecretStore {
    type Error = SecretStoreError;

    async fn metadata(&self, credential: &CredentialRef) -> Result<CredentialMetadata, Self::Error> {
        let state = self.state.lock().await;
        state
            .entries
            .get(credential)
            .map(|stored| stored.metadata.clone())
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })
    }
}

fn metadata(credential: &CredentialRef, status: CredentialStatus) -> Result<CredentialMetadata, SecretStoreError> {
    let fingerprint = fingerprint::digest("memory", credential)
        .map_err(SecretStoreError::invalid_generated(StorageBackend::InMemory))?;
    Ok(CredentialMetadata::new(
        credential.clone(),
        status,
        StorageBackend::InMemory,
        fingerprint,
    ))
}

#[cfg(test)]
mod tests;
