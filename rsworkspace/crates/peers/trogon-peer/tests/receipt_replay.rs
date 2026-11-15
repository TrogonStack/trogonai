#![allow(clippy::unwrap_used)]

mod support;
use support::*;
use trogon_peer::*;
use trogonai_proto::peers::v1alpha1 as proto;

#[test]
fn disposed_replay_binds_present_sender_to_immutable_intake() {
    for disposition in [TerminalDisposition::Expired, TerminalDisposition::Unavailable] {
        let before = run(enrolled("recipient", 0), &intake(0, 1));
        let disposed = run(
            before.clone(),
            &DisposeMessage::new(evidence(0, 1), disposition, time(3601)),
        );
        let original = disposed.dispositions[0].clone();
        assert_eq!(
            original.sender_peer_id.as_deref(),
            Some(identity("sender").peer_id().as_str())
        );
        for sender in [
            None,
            Some(identity("sender").peer_id().to_string()),
            Some(identity("other_sender").peer_id().to_string()),
            Some(String::new()),
        ] {
            let mut receipt = original.clone();
            receipt.sender_peer_id = sender.clone();
            let event = proto::PeerEvent {
                event: Some(
                    proto::PeerMessageDisposed {
                        receipt: buffa::MessageField::some(receipt),
                    }
                    .into(),
                ),
            };
            let replayed = commands::state::evolve(before.clone(), &event);
            match sender.as_deref() {
                None => assert!(replayed.unwrap().dispositions[0].sender_peer_id.is_none()),
                Some(sender) if sender == identity("sender").peer_id().as_str() => assert!(replayed.is_ok()),
                Some("") => assert!(matches!(
                    replayed,
                    Err(EvolveError::InvalidValue {
                        field: "receipt.sender_peer_id",
                        source: PeerValueError::InvalidPeerId
                    })
                )),
                _ => assert_eq!(replayed, Err(EvolveError::InvalidMessageTransition)),
            }
        }
    }
}

#[test]
fn terminal_receipt_replay_without_intake_preserves_legacy_absence_and_rejects_malformed_sender() {
    let before = enrolled("recipient", 0);
    let disposed = run(
        before.clone(),
        &DisposeMessage::new(evidence(0, 1), TerminalDisposition::Unavailable, time(3)),
    );
    for sender in [None, Some("malformed".into())] {
        let mut receipt = disposed.dispositions[0].clone();
        receipt.sender_peer_id = sender.clone();
        let event = proto::PeerEvent {
            event: Some(
                proto::PeerMessageDisposed {
                    receipt: buffa::MessageField::some(receipt),
                }
                .into(),
            ),
        };
        let replayed = commands::state::evolve(before.clone(), &event);
        if sender.is_none() {
            let replayed = replayed.unwrap();
            assert!(replayed.inbox.is_empty());
            assert!(replayed.dispositions[0].sender_peer_id.is_none());
        } else {
            assert!(matches!(
                replayed,
                Err(EvolveError::InvalidValue {
                    field: "receipt.sender_peer_id",
                    source: PeerValueError::InvalidPeerId
                })
            ));
        }
    }
}

#[test]
fn prepared_replay_binds_present_sender_to_immutable_intake() {
    let before = inbox(&[0], 1);
    let prepared = run(before.clone(), &prepare(&[0], 20, 3));
    let original = prepared.dispositions[0].clone();
    assert_eq!(
        original.sender_peer_id.as_deref(),
        Some(identity("sender").peer_id().as_str())
    );
    for sender in [
        None,
        Some(identity("sender").peer_id().to_string()),
        Some(identity("other_sender").peer_id().to_string()),
        Some(String::new()),
    ] {
        let mut receipt = original.clone();
        receipt.sender_peer_id = sender.clone();
        let event = proto::PeerEvent {
            event: Some(
                proto::PeerBatchPrepared {
                    batch: buffa::MessageField::some(prepared.batches[0].clone()),
                    receipts: vec![receipt],
                }
                .into(),
            ),
        };
        let replayed = commands::state::evolve(before.clone(), &event);
        match sender.as_deref() {
            None => assert!(replayed.unwrap().dispositions[0].sender_peer_id.is_none()),
            Some(sender) if sender == identity("sender").peer_id().as_str() => assert!(replayed.is_ok()),
            Some("") => assert!(matches!(
                replayed,
                Err(EvolveError::InvalidValue {
                    field: "receipt.sender_peer_id",
                    source: PeerValueError::InvalidPeerId
                })
            )),
            _ => assert_eq!(replayed, Err(EvolveError::InvalidMessageTransition)),
        }
    }
}
