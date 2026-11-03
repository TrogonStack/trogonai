use buffa::{Message, MessageField, MessageName};
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

use crate::peers::{PeerEventCase, PeerEventPayloadError, v1alpha1};

fn assert_roundtrip<Payload>(inner: Payload)
where
    Payload: Message + MessageName + Clone,
    PeerEventCase: From<Payload>,
{
    let expected_bytes = inner.encode_to_vec();
    let event = v1alpha1::PeerEvent {
        event: Some(inner.into()),
    };

    let encoded = EventEncode::encode(&event).unwrap();
    assert_eq!(encoded, expected_bytes);
    assert_eq!(event.event_type().unwrap(), Payload::FULL_NAME);
    let decoded = <v1alpha1::PeerEvent as EventDecode>::decode(EventData::new(Payload::FULL_NAME, &encoded))
        .unwrap()
        .into_decoded()
        .unwrap();
    assert_eq!(decoded, event);
}

#[test]
fn enrollment_and_attachment_events_roundtrip_through_public_codec() {
    assert_roundtrip(v1alpha1::PeerEnrolled {
        enrollment: MessageField::some(v1alpha1::PeerEnrollment {
            peer_id: Some("peer-one".into()),
            ..Default::default()
        }),
    });
    assert_roundtrip(v1alpha1::PeerAttached {
        attachment: MessageField::some(v1alpha1::PeerAttachment {
            attachment: MessageField::some(v1alpha1::AttachmentRef {
                peer_id: Some("peer-one".into()),
                connector_id: Some("connector-one".into()),
                generation: Some(3),
            }),
            ..Default::default()
        }),
    });
    assert_roundtrip(v1alpha1::PeerHeartbeatRecorded {
        active_at: MessageField::some(buffa_types::google::protobuf::Timestamp::from_unix(42, 0)),
        ..Default::default()
    });
    assert_roundtrip(v1alpha1::PeerDetached {
        attachment: MessageField::some(v1alpha1::AttachmentRef {
            peer_id: Some("peer-one".into()),
            ..Default::default()
        }),
        ..Default::default()
    });
}

#[test]
fn message_and_receipt_events_roundtrip_through_public_codec() {
    let receipt = v1alpha1::RecipientReceipt {
        message_id: Some("message-one".into()),
        recipient_revision: Some(7),
        ..Default::default()
    };
    assert_roundtrip(v1alpha1::PeerMessageAccepted {
        entry: MessageField::some(v1alpha1::OutboxEntry {
            idempotency_key: Some("send-one".into()),
            ..Default::default()
        }),
    });
    assert_roundtrip(v1alpha1::PeerMessageIntakeRecorded {
        entry: MessageField::some(v1alpha1::InboxEntry {
            intake_ordinal: Some(7),
            ..Default::default()
        }),
    });
    assert_roundtrip(v1alpha1::PeerBatchPrepared {
        batch: MessageField::some(v1alpha1::PeerBatch {
            batch_id: Some("batch-one".into()),
            message_ids: vec!["message-one".into()],
            ..Default::default()
        }),
        receipts: vec![receipt.clone()],
    });
    assert_roundtrip(v1alpha1::PeerBatchAcknowledged {
        batch_id: Some("batch-one".into()),
        ..Default::default()
    });
    assert_roundtrip(v1alpha1::PeerMessageDisposed {
        receipt: MessageField::some(receipt.clone()),
    });
    assert_roundtrip(v1alpha1::PeerMessageSettled {
        receipt: MessageField::some(receipt),
    });
}

#[test]
fn absent_event_case_has_typed_encode_and_type_errors() {
    let event = v1alpha1::PeerEvent { event: None };

    assert!(matches!(
        EventEncode::encode(&event),
        Err(PeerEventPayloadError::MissingEvent)
    ));
    assert!(matches!(event.event_type(), Err(PeerEventPayloadError::MissingEvent)));
}

#[test]
fn unknown_event_types_are_skipped_without_decoding_their_payload() {
    assert!(matches!(
        <v1alpha1::PeerEvent as EventDecode>::decode(EventData::new("example.unknown.Event", b"\0")),
        Ok(EventDecodeOutcome::Skipped)
    ));
}

#[test]
fn malformed_known_event_payload_retains_the_decode_error() {
    let decoded = <v1alpha1::PeerEvent as EventDecode>::decode(EventData::new(
        v1alpha1::PeerEnrolled::FULL_NAME,
        b"\0",
    ));
    let error = decoded.unwrap_err();

    assert!(matches!(error, PeerEventPayloadError::Decode(_)));
    assert!(std::error::Error::source(&error).is_some());
}
