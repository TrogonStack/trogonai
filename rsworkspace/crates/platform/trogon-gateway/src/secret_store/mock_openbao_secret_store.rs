use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_std::SecretString;

use super::{
    SecretDestroyReason, SecretMaterial, SecretStoreDestroy, SecretStoreError, SecretStoreGet, SecretStoreMetadata,
    SecretStorePut, SecretStoreRevoke, SecretStoreRotate, fingerprint,
};
use crate::credential::commands::domain::{
    CredentialKind, CredentialMetadata, CredentialPath, CredentialRef, CredentialScope, CredentialStatus,
    CredentialVersion, StorageBackend,
};
use crate::secret_store::openbao_secret_store::openbao_credential_id;

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
    pub fn data_path(credential: &CredentialRef) -> String {
        format!("secret/data/{}", CredentialPath::from(credential))
    }

    pub fn metadata_path(credential: &CredentialRef) -> String {
        format!("secret/metadata/{}", CredentialPath::from(credential))
    }
}

impl MockOpenBaoSecretStoreState {
    fn next_credential_ref(
        &mut self,
        scope: CredentialScope,
        kind: CredentialKind,
    ) -> Result<CredentialRef, SecretStoreError> {
        let id = openbao_credential_id(&scope, kind)
            .map_err(|error| SecretStoreError::invalid_generated(StorageBackend::OpenBao, "credential id", error))?;
        let current = self
            .versions
            .keys()
            .filter(|credential| credential.id() == &id)
            .map(|credential| credential.version())
            .max();
        let version = current.map_or_else(CredentialVersion::initial, CredentialVersion::next);

        Ok(CredentialRef::new(id, version, &scope, kind))
    }
}

impl SecretStorePut for MockOpenBaoSecretStore {
    type Error = SecretStoreError;

    async fn put(
        &self,
        scope: CredentialScope,
        kind: CredentialKind,
        value: SecretString,
    ) -> Result<CredentialRef, Self::Error> {
        let mut state = self.state.lock().await;
        let credential = state.next_credential_ref(scope, kind)?;
        let active_metadata = metadata(&credential, CredentialStatus::Active)?;
        for (stored_ref, stored) in &mut state.versions {
            if stored_ref.id() == credential.id() && stored.metadata.status() == CredentialStatus::Active {
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
            if stored_ref.id() == credential.id() && stored.metadata.status() != CredentialStatus::Destroyed {
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
            MockOpenBaoSecretStore::metadata_path(credential),
            credential.version().get()
        ),
    )
    .map_err(SecretStoreError::invalid_fingerprint(StorageBackend::OpenBao))?;

    Ok(CredentialMetadata::new(
        credential.clone(),
        status,
        StorageBackend::OpenBao,
        fingerprint,
    ))
}

#[cfg(test)]
mod tests;
