use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, OwnedView};
use serde_json::{Value, json};

use super::assert_json_codec;
use super::gateway_tests::{
    active_state, command_response, credential_ref, events, metadata, snapshots, state_metadata,
};
use crate::gateway::credentials::{checkpoints_v1, idempotency_v1, state_v1, v1};

macro_rules! assert_retained {
    ($owned:ty, $retained:ty, $fixture:expr, |$handle:ident| $checks:block) => {{
        let fixture = $fixture;
        let expected = assert_json_codec::<$owned>(fixture.clone());
        let retained = {
            let mut source = expected.clone();
            let retained = <$retained>::from_owned(&source).expect("retain owned message");
            source.clear();
            retained
        };
        let independent = retained.clone();
        drop(retained);
        let $handle = std::thread::spawn(move || independent)
            .join()
            .expect("retained message can cross a thread boundary");
        $checks
        assert_eq!(serde_json::to_value($handle.view()).expect("retained view JSON"), fixture);
        assert_eq!($handle.to_owned_message(), expected);
        let mut wire = $handle.into_bytes().to_vec();
        wire.extend_from_slice(&[0xf8, 0x07, 0x01]);
        let too_small = DecodeOptions::new().with_max_message_size(wire.len() - 1);
        assert_eq!(
            <$retained>::decode_with_options(Bytes::copy_from_slice(&wire), &too_small).err(),
            Some(DecodeError::MessageTooLarge)
        );
        let options = DecodeOptions::new().with_max_message_size(wire.len());
        let retained = <$retained>::decode_with_options(Bytes::copy_from_slice(&wire), &options)
            .expect("bounded retained message");
        assert_eq!(retained.bytes().as_ref(), wire.as_slice());
        let pointer = retained.bytes().as_ptr();
        let raw: OwnedView<<$owned as HasMessageView>::View<'static>> = retained.into();
        let retained = <$retained>::from(raw);
        let transferred = retained.into_bytes();
        assert_eq!(transferred.as_ptr(), pointer, "ownership transfer must retain the allocation");
        assert_eq!(transferred.as_ref(), wire.as_slice(), "unknown wire fields must survive transfer");
        let decoded = <$retained>::decode(transferred).expect("transferred retained message");
        assert_eq!(decoded.to_owned_message(), expected);
    }};
}

fn payload(value: &Value) -> Value {
    value
        .as_object()
        .expect("oneof")
        .values()
        .next()
        .expect("payload")
        .clone()
}

#[test]
fn credential_reference_and_metadata_handles_survive_source_drop() {
    assert_retained!(
        v1::CredentialRef,
        v1::CredentialRefOwnedView,
        credential_ref(),
        |handle| {
            assert_eq!(handle.id(), "cred-1");
            assert_eq!(handle.version(), Some(2));
            assert_eq!(handle.owner_id(), "owner-1");
            assert_eq!(handle.scope_key(), "org/repo");
            assert_eq!(
                handle.source().and_then(|value| value.as_known()),
                Some(v1::CredentialSource::Github)
            );
            assert_eq!(
                handle.kind().and_then(|value| value.as_known()),
                Some(v1::CredentialKind::WebhookSecret)
            );
        }
    );
    assert_retained!(
        v1::CredentialMetadata,
        v1::CredentialMetadataOwnedView,
        metadata(),
        |handle| {
            assert!(handle.reference().as_option().is_some());
            assert_eq!(
                handle.status().and_then(|value| value.as_known()),
                Some(v1::CredentialStatus::Active)
            );
            assert_eq!(handle.fingerprint(), "sha256:abc");
        }
    );
    assert_retained!(
        state_v1::CredentialMetadata,
        state_v1::CredentialMetadataOwnedView,
        state_metadata(),
        |handle| {
            assert!(handle.reference().as_option().is_some());
            assert_eq!(
                handle.status().and_then(|value| value.as_known()),
                Some(v1::CredentialStatus::Active)
            );
            assert_eq!(
                handle.storage_backend().and_then(|value| value.as_known()),
                Some(state_v1::StorageBackend::Openbao)
            );
            assert_eq!(handle.fingerprint(), "sha256:abc");
        }
    );
}

#[test]
fn event_handles_survive_source_drop() {
    let events = events();
    assert_retained!(
        v1::CredentialEvent,
        v1::CredentialEventOwnedView,
        events[0].clone(),
        |handle| {
            assert!(handle.event().is_some());
        }
    );
    assert_retained!(
        v1::CredentialWriteRequested,
        v1::CredentialWriteRequestedOwnedView,
        payload(&events[0]),
        |handle| {
            assert_eq!(handle.credential_id(), "cred-1");
            assert_eq!(handle.owner_id(), "owner-1");
            assert_eq!(
                handle.source().and_then(|value| value.as_known()),
                Some(v1::CredentialSource::Slack)
            );
            assert_eq!(
                handle.kind().and_then(|value| value.as_known()),
                Some(v1::CredentialKind::BotToken)
            );
            assert_eq!(handle.scope_key(), "team");
        }
    );
    assert_retained!(
        v1::CredentialWriteFailed,
        v1::CredentialWriteFailedOwnedView,
        payload(&events[1]),
        |handle| {
            assert_eq!(handle.credential_id(), "cred-1");
            assert_eq!(handle.reason(), "vault down");
        }
    );
    assert_retained!(
        v1::CredentialActivated,
        v1::CredentialActivatedOwnedView,
        payload(&events[2]),
        |handle| {
            assert!(handle.metadata().as_option().is_some());
        }
    );
    assert_retained!(
        v1::CredentialRotationRequested,
        v1::CredentialRotationRequestedOwnedView,
        payload(&events[3]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
        }
    );
    assert_retained!(
        v1::CredentialRotationFailed,
        v1::CredentialRotationFailedOwnedView,
        payload(&events[4]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
            assert_eq!(handle.reason(), "conflict");
        }
    );
    assert_retained!(
        v1::CredentialRotated,
        v1::CredentialRotatedOwnedView,
        payload(&events[5]),
        |handle| {
            assert!(handle.previous_credential_ref().as_option().is_some());
            assert!(handle.metadata().as_option().is_some());
        }
    );
    assert_retained!(
        v1::CredentialRevoked,
        v1::CredentialRevokedOwnedView,
        payload(&events[6]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
        }
    );
    assert_retained!(
        v1::CredentialDestroyRequested,
        v1::CredentialDestroyRequestedOwnedView,
        payload(&events[7]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
            assert_eq!(handle.reason(), "expired");
        }
    );
    assert_retained!(
        v1::CredentialDestroyed,
        v1::CredentialDestroyedOwnedView,
        payload(&events[8]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
        }
    );
    assert_retained!(
        v1::CredentialDestroyFailed,
        v1::CredentialDestroyFailedOwnedView,
        payload(&events[9]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
            assert_eq!(handle.reason(), "timeout");
        }
    );
}

#[test]
fn state_handles_survive_source_drop() {
    let snapshots = snapshots();
    assert_retained!(
        state_v1::CredentialStateSnapshot,
        state_v1::CredentialStateSnapshotOwnedView,
        snapshots[2].clone(),
        |handle| {
            assert!(handle.state().is_some());
        }
    );
    assert_retained!(
        state_v1::CredentialMissingState,
        state_v1::CredentialMissingStateOwnedView,
        payload(&snapshots[0]),
        |_handle| {}
    );
    assert_retained!(
        state_v1::PendingCredentialWriteState,
        state_v1::PendingCredentialWriteStateOwnedView,
        payload(&snapshots[1]),
        |handle| {
            assert_eq!(handle.credential_id(), "cred-1");
            assert_eq!(handle.owner_id(), "owner-1");
            assert_eq!(
                handle.source().and_then(|value| value.as_known()),
                Some(v1::CredentialSource::Zoom)
            );
            assert_eq!(
                handle.kind().and_then(|value| value.as_known()),
                Some(v1::CredentialKind::ClientSecret)
            );
            assert_eq!(handle.scope_key(), "acct");
        }
    );
    assert_retained!(
        state_v1::ActiveCredentialState,
        state_v1::ActiveCredentialStateOwnedView,
        active_state(),
        |handle| {
            assert!(handle.metadata().as_option().is_some());
            assert_eq!(handle.previous_versions().len(), 2);
        }
    );
    assert_retained!(
        state_v1::FailedCredentialWriteState,
        state_v1::FailedCredentialWriteStateOwnedView,
        payload(&snapshots[3]),
        |handle| {
            assert_eq!(handle.credential_id(), "cred-1");
            assert_eq!(handle.reason(), "denied");
        }
    );
    assert_retained!(
        state_v1::RotationPendingCredentialState,
        state_v1::RotationPendingCredentialStateOwnedView,
        payload(&snapshots[4]),
        |handle| {
            assert!(handle.active().as_option().is_some());
        }
    );
    assert_retained!(
        state_v1::RevokedCredentialState,
        state_v1::RevokedCredentialStateOwnedView,
        payload(&snapshots[5]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
        }
    );
    assert_retained!(
        state_v1::DestroyRequestedCredentialState,
        state_v1::DestroyRequestedCredentialStateOwnedView,
        payload(&snapshots[6]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
            assert_eq!(handle.reason(), "policy");
        }
    );
    assert_retained!(
        state_v1::DestroyedCredentialState,
        state_v1::DestroyedCredentialStateOwnedView,
        payload(&snapshots[7]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
        }
    );
    assert_retained!(
        state_v1::CleanupFailedCredentialState,
        state_v1::CleanupFailedCredentialStateOwnedView,
        payload(&snapshots[8]),
        |handle| {
            assert!(handle.credential_ref().as_option().is_some());
            assert_eq!(handle.reason(), "orphaned");
        }
    );
}

#[test]
fn checkpoint_and_idempotency_handles_survive_source_drop() {
    assert_retained!(
        checkpoints_v1::RuntimeProjectionCheckpoint,
        checkpoints_v1::RuntimeProjectionCheckpointOwnedView,
        json!({"lastScannedSequence": "18446744073709551615"}),
        |handle| {
            assert_eq!(handle.last_scanned_sequence(), Some(u64::MAX));
        }
    );
    assert_retained!(
        checkpoints_v1::CredentialRecoveryWorkerCheckpoint,
        checkpoints_v1::CredentialRecoveryWorkerCheckpointOwnedView,
        json!({
            "lastScannedSequence": "7", "consecutiveFailureCount": 3,
            "firstFailureUnixSeconds": "1700000000", "retryAfterUnixSeconds": "1700000060"
        }),
        |handle| {
            assert_eq!(handle.last_scanned_sequence(), Some(7));
            assert_eq!(handle.consecutive_failure_count(), Some(3));
            assert_eq!(handle.first_failure_unix_seconds(), Some(1_700_000_000));
            assert_eq!(handle.retry_after_unix_seconds(), Some(1_700_000_060));
        }
    );
    assert_retained!(
        idempotency_v1::CredentialCommandResponse,
        idempotency_v1::CredentialCommandResponseOwnedView,
        command_response(),
        |handle| {
            assert_eq!(handle.state(), "active");
            assert_eq!(handle.stream_position(), Some(u64::MAX));
            assert!(handle.credential_ref().as_option().is_some());
        }
    );
    assert_retained!(
        idempotency_v1::CredentialRefResponse,
        idempotency_v1::CredentialRefResponseOwnedView,
        command_response()["credentialRef"].clone(),
        |handle| {
            assert_eq!(handle.id(), "cred-1");
            assert_eq!(handle.version(), Some(3));
            assert_eq!(handle.owner_id(), "owner-1");
            assert_eq!(handle.source(), "github");
            assert_eq!(handle.scope_key(), "org/repo");
            assert_eq!(handle.kind(), "webhook_secret");
            assert_eq!(handle.status(), "active");
        }
    );
    assert_retained!(
        idempotency_v1::CredentialManagementIdempotencyRecord,
        idempotency_v1::CredentialManagementIdempotencyRecordOwnedView,
        json!({
            "fingerprint": "fp", "status": "CREDENTIAL_MANAGEMENT_IDEMPOTENCY_STATUS_COMPLETED",
            "response": command_response()
        }),
        |handle| {
            assert_eq!(handle.fingerprint(), "fp");
            assert!(handle.status().is_some());
            assert!(handle.response().as_option().is_some());
        }
    );
}
