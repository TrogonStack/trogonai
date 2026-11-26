#![allow(clippy::unwrap_used, clippy::panic)]

mod support;

use support::*;
use trogon_peer::{
    ContentSemanticFingerprint, EvolveError, PeerValueError, SealedContentRef, SendMessage, TrustedContent,
};
use trogonai_proto::peers::v1alpha1 as proto;

fn proposal(locator: &str, stored: &[u8], fingerprint: u8) -> SendMessage {
    SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        trogon_peer::IdempotencyKey::new("stable_key").unwrap(),
        trogon_peer::MessageId::new(format!("proposal_{fingerprint}")).unwrap(),
        identity("recipient").peer_id().clone(),
        trogon_peer::CollaborationRef::new("collaboration").unwrap(),
        TrustedContent::from_sealed_port(
            SealedContentRef::new(claim_check(locator, stored)).unwrap(),
            3,
            vec![fingerprint; 32],
        )
        .unwrap(),
        None,
        time(1),
    )
}

#[test]
fn storage_integrity_can_change_without_changing_semantic_retry_identity() {
    let first = run(
        enrolled("sender", 0),
        &proposal("replica_a", b"stored representation A", 7),
    );
    let original = first.outbox[0].clone();
    let reference = original.message.content_ref.as_option().unwrap();
    assert_eq!(
        original.message.semantic_fingerprint.value.as_deref(),
        Some(&[7; 32][..])
    );
    assert_eq!(
        reference.digest,
        claim_check("replica_a", b"stored representation A").digest
    );
    assert_ne!(
        reference.digest.value.as_deref(),
        original.message.semantic_fingerprint.value.as_deref()
    );
    let changed_storage = proposal("replica_b", b"stored representation B with a different nonce", 7);
    assert_eq!(
        error(first.clone(), &changed_storage),
        trogon_peer::MessageError::AlreadyAccepted(Box::new(original))
    );
    assert_eq!(
        error(first, &proposal("replica_a", b"stored representation A", 8)),
        trogon_peer::MessageError::Conflict
    );
}

#[test]
fn missing_or_malformed_semantic_fingerprint_never_falls_back_to_storage_integrity() {
    let accepted = run(enrolled("sender", 0), &proposal("replica", b"stored representation", 7));
    for bytes in [None, Some(Vec::new()), Some(vec![7; 31]), Some(vec![7; 33])] {
        let mut entry = accepted.outbox[0].clone();
        entry.message.as_option_mut().unwrap().semantic_fingerprint =
            proto::ContentSemanticFingerprint { value: bytes }.into();
        let result = trogon_peer::commands::state::evolve(
            enrolled("sender", 0),
            &proto::PeerEvent {
                event: Some(proto::PeerMessageAccepted { entry: entry.into() }.into()),
            },
        );
        assert!(
            matches!(
                result,
                Err(EvolveError::MissingField { .. } | EvolveError::InvalidValue { .. })
            ),
            "invalid fingerprint bypassed typed admission: {result:?}"
        );
    }
    let mut entry = accepted.outbox[0].clone();
    entry.message.as_option_mut().unwrap().semantic_fingerprint = Default::default();
    assert!(matches!(
        trogon_peer::commands::state::evolve(
            enrolled("sender", 0),
            &proto::PeerEvent {
                event: Some(proto::PeerMessageAccepted { entry: entry.into() }.into()),
            }
        ),
        Err(EvolveError::MissingField {
            field: "semantic_fingerprint"
        })
    ));
}

#[test]
fn semantic_fingerprint_cannot_replace_a_missing_storage_integrity_digest() {
    let accepted = run(enrolled("sender", 0), &proposal("replica", b"stored representation", 7));
    let mut entry = accepted.outbox[0].clone();
    entry
        .message
        .as_option_mut()
        .unwrap()
        .content_ref
        .as_option_mut()
        .unwrap()
        .digest = Default::default();
    assert!(matches!(
        trogon_peer::commands::state::evolve(
            enrolled("sender", 0),
            &proto::PeerEvent {
                event: Some(proto::PeerMessageAccepted { entry: entry.into() }.into()),
            }
        ),
        Err(EvolveError::InvalidValue {
            source: PeerValueError::InvalidContentReference,
            ..
        })
    ));
}

#[test]
fn storage_integrity_profile_and_semantic_fingerprint_are_separate_admission_types() {
    let original = claim_check("stored", b"actual stored fixture bytes");
    for algorithm in [
        None,
        Some(String::new()),
        Some("SHA256".into()),
        Some("md5".into()),
        Some("unknown".into()),
    ] {
        let mut invalid = original.clone();
        invalid.digest.as_option_mut().unwrap().algorithm = algorithm;
        assert!(matches!(
            SealedContentRef::new(invalid),
            Err(PeerValueError::InvalidContentReference)
        ));
    }
    for bytes in [None, Some(Vec::new()), Some(vec![0; 31]), Some(vec![0; 33])] {
        let mut invalid = original.clone();
        invalid.digest.as_option_mut().unwrap().value = bytes;
        assert!(matches!(
            SealedContentRef::new(invalid),
            Err(PeerValueError::InvalidContentReference)
        ));
    }
    let reference = SealedContentRef::new(original.clone()).unwrap();
    assert_eq!(reference.to_claim_check(), original);
    assert_eq!(reference.as_str(), "stored");
    assert_eq!(reference.integrity_digest().len(), 32);
    for bytes in [Vec::new(), vec![0; 31], vec![0; 33]] {
        assert!(matches!(
            ContentSemanticFingerprint::new(bytes),
            Err(PeerValueError::InvalidSemanticFingerprint)
        ));
    }
    let fingerprint = ContentSemanticFingerprint::new(vec![7; 32]).unwrap();
    assert_eq!(fingerprint.as_bytes(), &[7; 32]);
    assert_eq!(fingerprint.to_wire().value.as_deref(), Some(&[7; 32][..]));
}

#[test]
fn migrated_receipt_preserves_the_frozen_original_v1_semantic_digest() {
    const ORIGINAL_V1_DIGEST: &str = "238bb8d65b8167875ada78f83fae5bbcd2ed094bae0071d5c7994a8a5db9c2cf";
    let accepted = run(enrolled("sender", 0), &send(0, 1, 3));
    assert_eq!(
        hex::encode(accepted.outbox[0].semantic_digest.as_deref().unwrap()),
        ORIGINAL_V1_DIGEST,
        "the persisted v1 semantic framing or input order changed"
    );
    let mut migrated = accepted.outbox[0].clone();
    migrated.semantic_digest = Some(hex::decode(ORIGINAL_V1_DIGEST).unwrap());
    let replay = trogon_peer::commands::state::evolve(
        enrolled("sender", 0),
        &proto::PeerEvent {
            event: Some(
                proto::PeerMessageAccepted {
                    entry: migrated.clone().into(),
                }
                .into(),
            ),
        },
    )
    .unwrap();
    assert_eq!(
        replay.outbox,
        vec![migrated],
        "migrated carrier rejected the persisted v1 receipt"
    );
}
