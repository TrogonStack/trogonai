use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_std::SecretString;

use super::{
    SecretMaterial, SecretStoreError, SecretStoreGet, SecretStoreMetadata, SecretStorePut, SecretStoreRevoke,
    SecretStoreRotate,
};
use crate::credential::commands::domain::{
    CredentialFingerprint, CredentialId, CredentialKind, CredentialMetadata, CredentialRef, CredentialScope,
    CredentialStatus, CredentialVersion, StorageBackend,
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
    material: SecretMaterial,
    metadata: CredentialMetadata,
}

impl InMemorySecretStoreState {
    fn next_credential_ref(&mut self, scope: CredentialScope, kind: CredentialKind) -> CredentialRef {
        self.next_id += 1;
        let id = CredentialId::new(format!(
            "memory:{}:{}:{}",
            scope.scope_key(),
            kind.as_str(),
            self.next_id
        ))
        .expect("generated in-memory credential id is valid");

        CredentialRef::new(id, CredentialVersion::initial(), &scope, kind)
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
        let credential = state.next_credential_ref(scope, kind);
        let metadata = metadata(&credential, CredentialStatus::Active);
        state.entries.insert(
            credential.clone(),
            StoredCredential {
                material: SecretMaterial::plaintext(value),
                metadata,
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
        if !status.is_readable() {
            return Err(SecretStoreError::Unreadable {
                credential: credential.clone(),
                status,
            });
        }
        Ok(stored.material.clone())
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
        stored.metadata = metadata(credential, CredentialStatus::Previous);

        let new_credential = credential.next_version();
        let metadata = metadata(&new_credential, CredentialStatus::Active);
        state.entries.insert(
            new_credential.clone(),
            StoredCredential {
                material: SecretMaterial::plaintext(value),
                metadata,
            },
        );

        Ok(new_credential)
    }
}

impl SecretStoreRevoke for InMemorySecretStore {
    type Error = SecretStoreError;

    async fn revoke(&self, credential: &CredentialRef) -> Result<(), Self::Error> {
        let mut state = self.state.lock().await;
        let stored = state
            .entries
            .get_mut(credential)
            .ok_or_else(|| SecretStoreError::Missing {
                credential: credential.clone(),
            })?;
        stored.metadata = metadata(credential, CredentialStatus::Revoked);
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

fn metadata(credential: &CredentialRef, status: CredentialStatus) -> CredentialMetadata {
    let fingerprint = CredentialFingerprint::new(format!("memory:{}", credential))
        .expect("generated credential fingerprint is valid");
    CredentialMetadata::new(credential.clone(), status, StorageBackend::InMemory, fingerprint)
}

#[cfg(test)]
mod tests {
    use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};

    use super::*;

    #[tokio::test]
    async fn put_get_rotate_and_revoke_secret() {
        let store = InMemorySecretStore::default();
        let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
        let credential = store
            .put(
                scope,
                CredentialKind::BotToken,
                SecretString::new("Bot old-token").unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            store.get(&credential).await.unwrap().as_plaintext().unwrap().as_str(),
            "Bot old-token"
        );

        let rotated = store
            .rotate(&credential, SecretString::new("Bot new-token").unwrap())
            .await
            .unwrap();

        assert_eq!(rotated.version().get(), 2);
        assert_eq!(
            store.metadata(&credential).await.unwrap().status(),
            CredentialStatus::Previous
        );
        assert_eq!(
            store.metadata(&rotated).await.unwrap().status(),
            CredentialStatus::Active
        );

        store.revoke(&rotated).await.unwrap();

        assert!(matches!(
            store.get(&rotated).await,
            Err(SecretStoreError::Unreadable {
                status: CredentialStatus::Revoked,
                ..
            })
        ));
    }
}
