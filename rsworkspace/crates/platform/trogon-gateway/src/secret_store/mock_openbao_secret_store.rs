use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_std::SecretString;

use super::{
    SecretDestroyReason, SecretMaterial, SecretStoreDestroy, SecretStoreError, SecretStoreGet, SecretStoreMetadata,
    SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
};
use crate::credential::commands::domain::{
    CredentialFingerprint, CredentialKind, CredentialMetadata, CredentialPath, CredentialRef, CredentialScope,
    CredentialStatus, CredentialVersion, StorageBackend,
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
        let id = openbao_credential_id(&scope, kind).map_err(|error| SecretStoreError::BackendUnavailable {
            backend: StorageBackend::OpenBao,
            message: format!("generated credential id is invalid: {error}"),
        })?;
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
            if stored_ref.id() == credential.id() {
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
    let fingerprint = CredentialFingerprint::new(format!(
        "openbao:{}#{}",
        MockOpenBaoSecretStore::metadata_path(credential),
        credential.version().get()
    ))
    .map_err(|error| SecretStoreError::BackendUnavailable {
        backend: StorageBackend::OpenBao,
        message: format!("generated credential fingerprint is invalid: {error}"),
    })?;

    Ok(CredentialMetadata::new(
        credential.clone(),
        status,
        StorageBackend::OpenBao,
        fingerprint,
    ))
}

#[cfg(test)]
mod tests {
    use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};

    use super::*;

    #[tokio::test]
    async fn writes_secret_with_openbao_kv_v2_paths() {
        let store = MockOpenBaoSecretStore::default();
        let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
        let credential = store
            .put(scope, CredentialKind::BotToken, SecretString::new("Bot token").unwrap())
            .await
            .unwrap();

        assert_eq!(
            MockOpenBaoSecretStore::data_path(&credential),
            format!("secret/data/trogonai/tenant-1/credentials/{}", credential.id())
        );
        assert_eq!(
            MockOpenBaoSecretStore::metadata_path(&credential),
            format!("secret/metadata/trogonai/tenant-1/credentials/{}", credential.id())
        );
        assert_eq!(
            store.metadata(&credential).await.unwrap().storage_backend(),
            StorageBackend::OpenBao
        );
        assert_eq!(
            store.get(&credential).await.unwrap().as_plaintext().unwrap().as_str(),
            "Bot token"
        );
    }

    #[tokio::test]
    async fn rotation_creates_new_version_on_same_openbao_path() {
        let store = MockOpenBaoSecretStore::default();
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

        assert_eq!(credential.id(), rotated.id());
        assert_eq!(rotated.version().get(), 2);
        assert_eq!(
            MockOpenBaoSecretStore::data_path(&credential),
            MockOpenBaoSecretStore::data_path(&rotated)
        );
        assert_eq!(
            store.metadata(&credential).await.unwrap().status(),
            CredentialStatus::Previous
        );
        assert_eq!(
            store.metadata(&rotated).await.unwrap().status(),
            CredentialStatus::Active
        );
        assert_eq!(
            store.get(&rotated).await.unwrap().as_plaintext().unwrap().as_str(),
            "Bot new-token"
        );
    }

    #[tokio::test]
    async fn rotation_requires_active_version() {
        let store = MockOpenBaoSecretStore::default();
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

        assert!(matches!(
            store
                .rotate(&credential, SecretString::new("Bot ignored-token").unwrap())
                .await,
            Err(SecretStoreError::Unwritable {
                status: CredentialStatus::Previous,
                ..
            })
        ));
        assert_eq!(
            store.get(&rotated).await.unwrap().as_plaintext().unwrap().as_str(),
            "Bot new-token"
        );
    }

    #[tokio::test]
    async fn revoke_closes_every_version_on_the_openbao_path() {
        let store = MockOpenBaoSecretStore::default();
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

        store.revoke(&rotated).await.unwrap();

        assert!(matches!(
            store.get(&credential).await,
            Err(SecretStoreError::Unreadable {
                status: CredentialStatus::Revoked,
                ..
            })
        ));
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
        let store = MockOpenBaoSecretStore::default();
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
        let store = MockOpenBaoSecretStore::default();
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
        let store = MockOpenBaoSecretStore::default();
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
    async fn revoke_after_destroy_fails() {
        let store = MockOpenBaoSecretStore::default();
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
            store.revoke(&credential).await,
            Err(SecretStoreError::Unwritable {
                status: CredentialStatus::Destroyed,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn destroy_of_unknown_ref_returns_missing() {
        let store = MockOpenBaoSecretStore::default();
        let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
        let unknown = CredentialRef::new(
            openbao_credential_id(&scope, CredentialKind::BotToken).unwrap(),
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
        let store = MockOpenBaoSecretStore::default();
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
