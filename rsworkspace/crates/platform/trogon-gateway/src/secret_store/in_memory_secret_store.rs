use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_std::SecretString;

use super::{
    SecretDestroyReason, SecretMaterial, SecretStoreDestroy, SecretStoreError, SecretStoreGet, SecretStoreMetadata,
    SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
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
        .map_err(|error| SecretStoreError::BackendUnavailable {
            backend: StorageBackend::InMemory,
            message: format!("generated credential id is invalid: {error}"),
        })?;

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
        if !status.is_readable() {
            return Err(SecretStoreError::Unreadable {
                credential: credential.clone(),
                status,
            });
        }
        stored.material.clone().ok_or_else(|| SecretStoreError::Missing {
            credential: credential.clone(),
        })
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
        let stored = state
            .entries
            .get_mut(credential)
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
        stored.metadata = metadata(credential, CredentialStatus::Revoked)?;
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
    let fingerprint = CredentialFingerprint::new(format!("memory:{}", credential)).map_err(|error| {
        SecretStoreError::BackendUnavailable {
            backend: StorageBackend::InMemory,
            message: format!("generated credential fingerprint is invalid: {error}"),
        }
    })?;
    Ok(CredentialMetadata::new(
        credential.clone(),
        status,
        StorageBackend::InMemory,
        fingerprint,
    ))
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

    #[tokio::test]
    async fn destroy_drops_material_and_marks_status_destroyed() {
        let store = InMemorySecretStore::default();
        let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
        let credential = store
            .put(scope, CredentialKind::BotToken, SecretString::new("Bot token").unwrap())
            .await
            .unwrap();

        store
            .destroy(&credential, &SecretDestroyReason::new("compliance request").unwrap())
            .await
            .unwrap();

        assert!(matches!(
            store.get(&credential).await,
            Err(SecretStoreError::Unreadable {
                status: CredentialStatus::Destroyed,
                ..
            })
        ));
        assert_eq!(
            store.metadata(&credential).await.unwrap().status(),
            CredentialStatus::Destroyed
        );
    }

    #[tokio::test]
    async fn destroying_an_already_destroyed_version_is_idempotent() {
        let store = InMemorySecretStore::default();
        let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
        let credential = store
            .put(scope, CredentialKind::BotToken, SecretString::new("Bot token").unwrap())
            .await
            .unwrap();
        let reason = SecretDestroyReason::new("compliance request").unwrap();
        store.destroy(&credential, &reason).await.unwrap();

        store.destroy(&credential, &reason).await.unwrap();
    }

    #[tokio::test]
    async fn rotate_after_destroy_fails() {
        let store = InMemorySecretStore::default();
        let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
        let credential = store
            .put(scope, CredentialKind::BotToken, SecretString::new("Bot token").unwrap())
            .await
            .unwrap();
        store
            .destroy(&credential, &SecretDestroyReason::new("compliance request").unwrap())
            .await
            .unwrap();

        assert!(matches!(
            store
                .rotate(&credential, SecretString::new("Bot new-token").unwrap())
                .await,
            Err(SecretStoreError::Unwritable {
                status: CredentialStatus::Destroyed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn destroy_of_unknown_ref_returns_missing() {
        let store = InMemorySecretStore::default();
        let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
        let unknown = CredentialRef::new(
            CredentialId::new("memory:tenant-1:discord:bot_token:missing").unwrap(),
            CredentialVersion::initial(),
            &scope,
            CredentialKind::BotToken,
        );

        assert!(matches!(
            store
                .destroy(&unknown, &SecretDestroyReason::new("compliance request").unwrap())
                .await,
            Err(SecretStoreError::Missing { .. })
        ));
    }

    #[tokio::test]
    async fn destroying_previous_version_leaves_active_version_readable() {
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
        let rotated = store
            .rotate(&credential, SecretString::new("Bot new-token").unwrap())
            .await
            .unwrap();

        store
            .destroy(&credential, &SecretDestroyReason::new("compliance request").unwrap())
            .await
            .unwrap();

        assert!(matches!(
            store.get(&credential).await,
            Err(SecretStoreError::Unreadable {
                status: CredentialStatus::Destroyed,
                ..
            })
        ));
        assert_eq!(
            store.get(&rotated).await.unwrap().as_plaintext().unwrap().as_str(),
            "Bot new-token"
        );
    }
}
