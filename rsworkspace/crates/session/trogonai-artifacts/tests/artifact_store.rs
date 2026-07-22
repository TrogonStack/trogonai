use bytes::Bytes;
use trogon_nats::jetstream::MockObjectStore;
use trogonai_session_contracts::{EventId, SessionId, ToolExecutionId};
use trogonai_session_kernel::SessionKernelConfig;

use trogonai_artifacts::{
    ArtifactAvailability, ArtifactStorageMode, ArtifactStore, ArtifactStoreConfig,
    ArtifactStoreError, ArtifactUnavailableReason, StoreArtifactRequest, artifact_object_key,
    artifact_storage_ref, sha256_hex,
};

fn test_store() -> ArtifactStore<MockObjectStore> {
    ArtifactStore::new(
        MockObjectStore::new(),
        ArtifactStoreConfig::from_session_kernel(&SessionKernelConfig::default()),
    )
}

#[tokio::test]
async fn inline_round_trip_stays_below_kernel_threshold() {
    let store = test_store();
    let inline_limit = store.config().inline_limit_bytes;
    let session_id = SessionId::new("sess_roundtrip_inline").unwrap();
    let event_id = EventId::new("evt_inline").unwrap();
    let content = Bytes::from("x".repeat(inline_limit - 1));

    let stored = store
        .store(StoreArtifactRequest::new(
            session_id,
            event_id,
            "text/plain",
            content.clone(),
        ))
        .await
        .unwrap();

    assert_eq!(stored.storage_mode, ArtifactStorageMode::Inline);
    assert_eq!(stored.inline_content.as_ref(), Some(&content));
    assert_eq!(stored.metadata.sha256, sha256_hex(&content));
    assert!(stored.metadata.storage_ref.is_empty());
}

#[tokio::test]
async fn claim_check_round_trip_exceeds_kernel_threshold() {
    let store = test_store();
    let inline_limit = store.config().inline_limit_bytes;
    let session_id = SessionId::new("sess_roundtrip_claim").unwrap();
    let event_id = EventId::new("evt_claim").unwrap();
    let tool_execution_id = ToolExecutionId::new("texec_bash").unwrap();
    let content = Bytes::from("y".repeat(inline_limit + 1));

    let stored = store
        .store(
            StoreArtifactRequest::new(
                session_id.clone(),
                event_id,
                "text/plain",
                content.clone(),
            )
            .with_tool_execution_id(tool_execution_id),
        )
        .await
        .unwrap();

    assert_eq!(stored.storage_mode, ArtifactStorageMode::ClaimCheck);
    assert!(stored.metadata.truncated);
    assert!(stored.metadata.storage_ref.starts_with("obj://ACP_SESSION_ARTIFACTS/"));

    let artifact_id = trogonai_session_contracts::ArtifactId::new(&stored.metadata.artifact_id).unwrap();
    let expected_key = artifact_object_key(&session_id, &artifact_id);
    assert_eq!(
        stored.metadata.storage_ref,
        artifact_storage_ref("ACP_SESSION_ARTIFACTS", &expected_key)
    );

    let retrieved = store.retrieve(&stored.metadata).await.unwrap();
    assert_eq!(retrieved.content, content);
}

#[tokio::test]
async fn retrieval_by_artifact_ref_resolves_object_store_content() {
    let store = test_store();
    let session_id = SessionId::new("sess_by_ref").unwrap();
    let event_id = EventId::new("evt_by_ref").unwrap();
    let content = Bytes::from("artifact ref payload".repeat(8_192));

    let stored = store
        .store(StoreArtifactRequest::new(
            session_id.clone(),
            event_id,
            "text/plain",
            content.clone(),
        ))
        .await
        .unwrap();

    let artifact_ref = stored.to_artifact_ref();
    let retrieved = store.retrieve_by_ref(&session_id, &artifact_ref).await.unwrap();
    assert_eq!(retrieved.content, content);
}

#[tokio::test]
async fn checksum_verification_rejects_tampered_content() {
    let store = test_store();
    let inline_limit = store.config().inline_limit_bytes;
    let session_id = SessionId::new("sess_checksum").unwrap();
    let event_id = EventId::new("evt_checksum").unwrap();
    let content = Bytes::from("verified content".repeat(inline_limit / 16 + 1));

    let stored = store
        .store(StoreArtifactRequest::new(
            session_id,
            event_id,
            "text/plain",
            content,
        ))
        .await
        .unwrap();

    let mut tampered = stored.metadata.clone();
    tampered.sha256 = "0".repeat(64);
    let err = store.retrieve(&tampered).await.unwrap_err();
    assert!(matches!(err, ArtifactStoreError::ChecksumMismatch { .. }));
}

#[tokio::test]
async fn missing_referenced_object_yields_artifact_unavailable() {
    let store = test_store();
    let inline_limit = store.config().inline_limit_bytes;
    let session_id = SessionId::new("sess_unavailable").unwrap();
    let event_id = EventId::new("evt_unavailable").unwrap();
    let content = Bytes::from("z".repeat(inline_limit + 1));

    let stored = store
        .store(StoreArtifactRequest::new(
            session_id,
            event_id,
            "text/plain",
            content,
        ))
        .await
        .unwrap();
    assert_eq!(stored.storage_mode, ArtifactStorageMode::ClaimCheck);

    // The store that persisted the object resolves it as available.
    match store.retrieve_availability(&stored.metadata).await.unwrap() {
        ArtifactAvailability::Available(retrieved) => {
            assert_eq!(retrieved.metadata.artifact_id, stored.metadata.artifact_id);
        }
        other => panic!("expected available, got {other:?}"),
    }

    // A store that never persisted the object: the reference exists in the session
    // but the bytes are gone -> explicit `artifact_unavailable`, not a hard error.
    let empty = test_store();
    match empty.retrieve_availability(&stored.metadata).await.unwrap() {
        ArtifactAvailability::Unavailable(unavailable) => {
            assert_eq!(
                unavailable.reason,
                ArtifactUnavailableReason::NotInObjectStore
            );
        }
        other => panic!("expected unavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn corrupted_referenced_object_yields_artifact_unavailable() {
    let store = test_store();
    let inline_limit = store.config().inline_limit_bytes;
    let session_id = SessionId::new("sess_corrupt").unwrap();
    let event_id = EventId::new("evt_corrupt").unwrap();
    let content = Bytes::from("c".repeat(inline_limit + 1));

    let stored = store
        .store(StoreArtifactRequest::new(
            session_id,
            event_id,
            "text/plain",
            content,
        ))
        .await
        .unwrap();

    let mut tampered = stored.metadata.clone();
    tampered.sha256 = "0".repeat(64);
    match store.retrieve_availability(&tampered).await.unwrap() {
        ArtifactAvailability::Unavailable(unavailable) => {
            assert_eq!(
                unavailable.reason,
                ArtifactUnavailableReason::ChecksumMismatch
            );
        }
        other => panic!("expected unavailable, got {other:?}"),
    }
}
