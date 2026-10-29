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
