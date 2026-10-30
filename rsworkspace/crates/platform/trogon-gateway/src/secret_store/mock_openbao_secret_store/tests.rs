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

fn unknown_credential() -> CredentialRef {
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    CredentialRef::new(
        openbao_credential_id(&scope, CredentialKind::BotToken).unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::BotToken,
    )
}

#[tokio::test]
async fn get_of_unknown_ref_returns_missing() {
    let store = MockOpenBaoSecretStore::default();
    let unknown = unknown_credential();

    assert_eq!(
        store.get(&unknown).await.unwrap_err(),
        SecretStoreError::Missing { credential: unknown }
    );
}

#[tokio::test]
async fn rotate_of_unknown_ref_returns_missing() {
    let store = MockOpenBaoSecretStore::default();
    let unknown = unknown_credential();

    assert_eq!(
        store
            .rotate(&unknown, SecretString::new("Bot new-token").unwrap())
            .await
            .unwrap_err(),
        SecretStoreError::Missing { credential: unknown }
    );
}

#[tokio::test]
async fn revoke_of_unknown_ref_returns_missing() {
    let store = MockOpenBaoSecretStore::default();
    let unknown = unknown_credential();

    assert_eq!(
        store.revoke(&unknown).await.unwrap_err(),
        SecretStoreError::Missing { credential: unknown }
    );
}

#[tokio::test]
async fn metadata_of_unknown_ref_returns_missing() {
    let store = MockOpenBaoSecretStore::default();
    let unknown = unknown_credential();

    assert_eq!(
        store.metadata(&unknown).await.unwrap_err(),
        SecretStoreError::Missing { credential: unknown }
    );
}

#[tokio::test]
async fn putting_again_on_the_same_path_demotes_the_earlier_version() {
    let store = MockOpenBaoSecretStore::default();
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    let first = store
        .put(
            scope.clone(),
            CredentialKind::BotToken,
            SecretString::new("Bot first-token").unwrap(),
        )
        .await
        .unwrap();

    let second = store
        .put(
            scope,
            CredentialKind::BotToken,
            SecretString::new("Bot second-token").unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(second.version().get(), 2);
    assert_eq!(
        store.metadata(&first).await.unwrap().status(),
        CredentialStatus::Previous
    );
    assert_eq!(
        store.metadata(&second).await.unwrap().status(),
        CredentialStatus::Active
    );
}

#[tokio::test]
async fn putting_a_different_credential_leaves_other_paths_untouched() {
    let store = MockOpenBaoSecretStore::default();
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    let bot = store
        .put(
            scope.clone(),
            CredentialKind::BotToken,
            SecretString::new("Bot token").unwrap(),
        )
        .await
        .unwrap();

    store
        .put(
            scope,
            CredentialKind::VerificationToken,
            SecretString::new("verification").unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(store.metadata(&bot).await.unwrap().status(), CredentialStatus::Active);
}

#[tokio::test]
async fn revoke_leaves_destroyed_versions_destroyed() {
    let store = MockOpenBaoSecretStore::default();
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    let first = store
        .put(
            scope,
            CredentialKind::BotToken,
            SecretString::new("Bot first-token").unwrap(),
        )
        .await
        .unwrap();
    let second = store
        .rotate(&first, SecretString::new("Bot second-token").unwrap())
        .await
        .unwrap();
    store
        .destroy(&first, &SecretDestroyReason::new("compliance request").unwrap())
        .await
        .unwrap();

    store.revoke(&second).await.unwrap();

    assert_eq!(
        store.metadata(&first).await.unwrap().status(),
        CredentialStatus::Destroyed
    );
    assert_eq!(
        store.metadata(&second).await.unwrap().status(),
        CredentialStatus::Revoked
    );
}

#[tokio::test]
async fn put_after_revoke_keeps_the_revoked_version_unreadable() {
    let store = MockOpenBaoSecretStore::default();
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    let revoked = store
        .put(
            scope.clone(),
            CredentialKind::BotToken,
            SecretString::new("Bot old-token").unwrap(),
        )
        .await
        .unwrap();
    store.revoke(&revoked).await.unwrap();

    store
        .put(
            scope,
            CredentialKind::BotToken,
            SecretString::new("Bot new-token").unwrap(),
        )
        .await
        .unwrap();

    assert!(matches!(
        store.get(&revoked).await,
        Err(SecretStoreError::Unreadable {
            status: CredentialStatus::Revoked,
            ..
        })
    ));
}
