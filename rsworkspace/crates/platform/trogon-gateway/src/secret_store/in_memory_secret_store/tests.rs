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
async fn revoke_closes_every_version_of_the_credential() {
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

    store.revoke(&rotated).await.unwrap();

    for version in [&credential, &rotated] {
        assert_eq!(
            store.metadata(version).await.unwrap().status(),
            CredentialStatus::Revoked
        );
        assert!(matches!(
            store.get(version).await,
            Err(SecretStoreError::Unreadable {
                status: CredentialStatus::Revoked,
                ..
            })
        ));
    }
}

#[tokio::test]
async fn revoke_leaves_destroyed_versions_destroyed() {
    let store = InMemorySecretStore::default();
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
    let third = store
        .rotate(&second, SecretString::new("Bot third-token").unwrap())
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
    assert_eq!(
        store.metadata(&third).await.unwrap().status(),
        CredentialStatus::Revoked
    );
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

fn unknown_credential() -> CredentialRef {
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    CredentialRef::new(
        CredentialId::new("memory:tenant-1:discord:bot_token:missing").unwrap(),
        CredentialVersion::initial(),
        &scope,
        CredentialKind::BotToken,
    )
}

#[tokio::test]
async fn get_of_unknown_ref_returns_missing() {
    let store = InMemorySecretStore::default();
    let unknown = unknown_credential();

    assert_eq!(
        store.get(&unknown).await.unwrap_err(),
        SecretStoreError::Missing { credential: unknown }
    );
}

#[tokio::test]
async fn rotate_of_unknown_ref_returns_missing() {
    let store = InMemorySecretStore::default();
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
    let store = InMemorySecretStore::default();
    let unknown = unknown_credential();

    assert_eq!(
        store.revoke(&unknown).await.unwrap_err(),
        SecretStoreError::Missing { credential: unknown }
    );
}

#[tokio::test]
async fn metadata_of_unknown_ref_returns_missing() {
    let store = InMemorySecretStore::default();
    let unknown = unknown_credential();

    assert_eq!(
        store.metadata(&unknown).await.unwrap_err(),
        SecretStoreError::Missing { credential: unknown }
    );
}

#[tokio::test]
async fn revoke_of_a_destroyed_version_is_unwritable() {
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

    assert_eq!(
        store.revoke(&credential).await.unwrap_err(),
        SecretStoreError::Unwritable {
            credential,
            status: CredentialStatus::Destroyed,
        }
    );
}

#[tokio::test]
async fn rotate_of_a_previous_version_is_unwritable() {
    let store = InMemorySecretStore::default();
    let scope = CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::Discord);
    let credential = store
        .put(scope, CredentialKind::BotToken, SecretString::new("Bot token").unwrap())
        .await
        .unwrap();
    store
        .rotate(&credential, SecretString::new("Bot new-token").unwrap())
        .await
        .unwrap();

    assert_eq!(
        store
            .rotate(&credential, SecretString::new("Bot newer-token").unwrap())
            .await
            .unwrap_err(),
        SecretStoreError::Unwritable {
            credential,
            status: CredentialStatus::Previous,
        }
    );
}
