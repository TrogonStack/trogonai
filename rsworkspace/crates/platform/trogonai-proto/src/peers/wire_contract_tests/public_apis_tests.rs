use std::fmt::Debug;

use buffa::bytes::Bytes;
use buffa::json_helpers::ProtoElemJson;
use buffa::type_registry::TypeRegistry;
use buffa::{Enumeration, HasMessageView, Message, MessageName};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::fixtures;
use crate::peers::{state_v1alpha1 as state, v1alpha1 as proto};

#[derive(Serialize, Deserialize)]
#[serde(bound = "M: ProtoElemJson")]
struct ProtoBatch<M> {
    #[serde(with = "buffa::json_helpers::proto_seq")]
    messages: Vec<M>,
}

fn public_contract<M>(registry: &TypeRegistry, fixture: Value, diagnostic_fragments: &[&str])
where
    M: Message + MessageName + HasMessageView + ProtoElemJson + Serialize + DeserializeOwned + Debug + PartialEq,
    for<'a> M::View<'a>: Debug + Serialize,
    M::ViewHandle: Debug + Serialize,
{
    let message: M = serde_json::from_value(fixture.clone()).unwrap();
    let wire = message.encode_to_vec();
    let view = M::decode_view(&wire).unwrap();
    let owned_diagnostic = format!("{message:?}");
    let view_diagnostic = format!("{view:?}");
    drop(view);
    let retained = M::decode_view_handle(Bytes::from(wire.clone())).unwrap();
    for diagnostic in [owned_diagnostic, view_diagnostic, format!("{retained:?}")] {
        assert!(
            diagnostic.contains(M::NAME),
            "diagnostic lost the protobuf message identity"
        );
        for fragment in diagnostic_fragments {
            assert!(diagnostic.contains(fragment), "missing {fragment:?} from {diagnostic}");
        }
    }

    assert_eq!(M::TYPE_URL, format!("type.googleapis.com/{}", M::FULL_NAME));
    let entry = registry
        .json_any_by_url(M::TYPE_URL)
        .expect("registered peer Any payload");
    assert_eq!((entry.from_json)(fixture.clone()).unwrap(), wire);
    assert_eq!((entry.to_json)(&wire).unwrap(), fixture);
    assert!((entry.to_json)(b"\0").is_err());
    assert!((entry.from_json)(json!(false)).is_err());

    let absent: M = serde_json::from_value(json!({})).unwrap();
    let batch = ProtoBatch {
        messages: vec![message, absent],
    };
    let json = json!({"messages": [fixture, {}]});
    assert_eq!(serde_json::to_value(&batch).unwrap(), json);
    let decoded: ProtoBatch<M> = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.messages, batch.messages);
    let empty: ProtoBatch<M> = serde_json::from_value(json!({"messages": null})).unwrap();
    assert!(empty.messages.is_empty());
    assert!(serde_json::from_value::<ProtoBatch<M>>(json!({"messages": [null]})).is_err());
    super::writer_failures::assert_message_writes::<M>(&fixture);
}

fn registry() -> TypeRegistry {
    let mut registry = TypeRegistry::new();
    proto::register_types(&mut registry);
    state::register_types(&mut registry);
    registry
}

fn event_payload(name: &str) -> Value {
    fixtures::events()
        .into_iter()
        .find_map(|event| event.get(name).cloned())
        .unwrap()
}

#[test]
fn registered_identity_and_presence_messages_keep_diagnostic_identity_and_proto_json_presence() {
    let registry = registry();
    public_contract::<proto::AttachmentRef>(
        &registry,
        fixtures::attachment_ref(),
        &["peer_alpha", "connector_alpha", "generation"],
    );
    public_contract::<proto::PeerEnrollment>(
        &registry,
        fixtures::enrollment(),
        &[
            "tenant_alpha",
            "caller_alpha",
            "installation_alpha",
            "native_alpha",
            "collaboration_alpha",
        ],
    );
    public_contract::<proto::PeerAttachment>(
        &registry,
        fixtures::attachment(),
        &["connector_alpha", "observed_at", "expires_at"],
    );
    public_contract::<proto::PeerPresence>(
        &registry,
        fixtures::presence(),
        &["peer_alpha", "Review", "active_at", "observed_at", "expires_at"],
    );
}

#[test]
fn registered_mailbox_messages_keep_provenance_and_order_in_diagnostics_and_proto_json() {
    let registry = registry();
    public_contract::<proto::ContentSemanticFingerprint>(&registry, fixtures::semantic_fingerprint(), &["value"]);
    public_contract::<proto::StorageLocator>(&registry, json!({"value": "content_outgoing"}), &["content_outgoing"]);
    public_contract::<proto::Digest>(
        &registry,
        json!({"algorithm": "sha256", "value": fixtures::DIGEST}),
        &["sha256"],
    );
    public_contract::<proto::ClaimCheck>(
        &registry,
        fixtures::content_ref(),
        &["content_outgoing", "sha256", "storage_ref"],
    );
    public_contract::<proto::MessageMetadata>(
        &registry,
        fixtures::metadata(),
        &[
            "message_outgoing",
            "peer_alpha",
            "peer_beta",
            "content_outgoing",
            "message_previous",
        ],
    );
    public_contract::<proto::MessageContent>(&registry, json!({"text": "hello peer"}), &["text", "hello peer"]);
    public_contract::<proto::PreparedMessage>(
        &registry,
        fixtures::prepared_message("message_incoming_one"),
        &["message_incoming_one", "peer_alpha", "hello peer"],
    );
    public_contract::<proto::RecipientReceipt>(
        &registry,
        fixtures::receipt(),
        &["message_outgoing", "peer_beta", "batch_remote", "recipient_revision"],
    );
    public_contract::<proto::OutboxEntry>(
        &registry,
        fixtures::outbox(),
        &["send_outgoing", "message_outgoing", "settlement"],
    );
    public_contract::<proto::InboxEntry>(
        &registry,
        fixtures::inbox("message_incoming_one", 7),
        &["message_incoming_one", "intake_ordinal", "disposition"],
    );
    public_contract::<proto::PeerBatch>(
        &registry,
        fixtures::acknowledged_batch(),
        &[
            "batch_local",
            "message_incoming_one",
            "message_incoming_two",
            "acknowledged_at",
        ],
    );
    public_contract::<state::PeerState>(
        &registry,
        fixtures::state(),
        &[
            "peer_alpha",
            "connector_alpha",
            "outbox",
            "inbox",
            "batches",
            "next_intake_ordinal",
        ],
    );
}

#[test]
fn registered_event_payloads_dispatch_without_erasing_the_fact_kind() {
    let registry = registry();
    public_contract::<proto::PeerEnrolled>(
        &registry,
        event_payload("peerEnrolled"),
        &["PeerEnrollment", "caller_alpha"],
    );
    public_contract::<proto::PeerAttached>(
        &registry,
        event_payload("peerAttached"),
        &["PeerAttachment", "connector_alpha"],
    );
    public_contract::<proto::PeerHeartbeatRecorded>(
        &registry,
        event_payload("peerHeartbeatRecorded"),
        &["availability", "condition", "active_at"],
    );
    public_contract::<proto::PeerDetached>(
        &registry,
        event_payload("peerDetached"),
        &["connector_alpha", "detached_at"],
    );
    public_contract::<proto::PeerMessageAccepted>(
        &registry,
        event_payload("peerMessageAccepted"),
        &["OutboxEntry", "send_outgoing"],
    );
    public_contract::<proto::PeerMessageIntakeRecorded>(
        &registry,
        event_payload("peerMessageIntakeRecorded"),
        &["InboxEntry", "intake_ordinal"],
    );
    public_contract::<proto::PeerBatchPrepared>(
        &registry,
        event_payload("peerBatchPrepared"),
        &["PeerBatch", "receipts", "message_incoming_one", "message_incoming_two"],
    );
    public_contract::<proto::PeerBatchAcknowledged>(
        &registry,
        event_payload("peerBatchAcknowledged"),
        &["batch_local", "acknowledged_at"],
    );
    public_contract::<proto::PeerMessageDisposed>(
        &registry,
        event_payload("peerMessageDisposed"),
        &["message_expired", "outcome"],
    );
    public_contract::<proto::PeerMessageSettled>(
        &registry,
        event_payload("peerMessageSettled"),
        &["message_outgoing", "recipient_revision"],
    );
    let identities = [
        "PeerEnrolled",
        "PeerAttached",
        "PeerHeartbeatRecorded",
        "PeerDetached",
        "PeerMessageAccepted",
        "PeerMessageIntakeRecorded",
        "PeerBatchPrepared",
        "PeerBatchAcknowledged",
        "PeerMessageDisposed",
        "PeerMessageSettled",
    ];
    let events = fixtures::events();
    assert_eq!(events.len(), identities.len());
    for (event, identity) in events.into_iter().zip(identities) {
        public_contract::<proto::PeerEvent>(&registry, event, &[identity]);
    }
}

#[test]
fn registered_rpc_pairs_keep_correlation_and_application_evidence_in_proto_json() {
    let registry = registry();
    public_contract::<proto::EnrollRequest>(
        &registry,
        json!({"installationId": "installation_alpha", "nativeSessionId": "native_alpha", "connectorId": "connector_alpha", "collaborationRef": "collaboration_alpha", "displayName": "Review"}),
        &["native_alpha", "connector_alpha"],
    );
    public_contract::<proto::EnrollResponse>(
        &registry,
        json!({"peerId": "peer_alpha", "attachment": fixtures::attachment()}),
        &["peer_alpha", "attachment"],
    );
    public_contract::<proto::HeartbeatRequest>(
        &registry,
        json!({"attachmentRef": fixtures::attachment_ref(), "availability": "PEER_AVAILABILITY_BLOCKED", "condition": "PEER_CONDITION_INPUT", "activeAt": fixtures::OBSERVED}),
        &["connector_alpha", "active_at"],
    );
    public_contract::<proto::HeartbeatResponse>(
        &registry,
        json!({"attachment": fixtures::attachment()}),
        &["connector_alpha", "expires_at"],
    );
    public_contract::<proto::DetachRequest>(
        &registry,
        json!({"attachmentRef": fixtures::attachment_ref()}),
        &["peer_alpha", "generation"],
    );
    public_contract::<proto::DetachResponse>(&registry, json!({}), &[]);
    public_contract::<proto::RosterRequest>(
        &registry,
        json!({"attachmentRef": fixtures::attachment_ref(), "pageSize": 16, "pageToken": "cursor_current"}),
        &["cursor_current", "page_size"],
    );
    public_contract::<proto::RosterResponse>(
        &registry,
        json!({"peers": [fixtures::presence()], "nextPageToken": "cursor_next"}),
        &["peer_alpha", "cursor_next"],
    );
    public_contract::<proto::SendRequest>(
        &registry,
        json!({"attachmentRef": fixtures::attachment_ref(), "recipientPeerId": "peer_beta", "idempotencyKey": "send_outgoing", "text": "hello peer", "replyToMessageId": "message_previous"}),
        &["peer_beta", "send_outgoing", "message_previous"],
    );
    public_contract::<proto::SendResponse>(
        &registry,
        json!({"message": fixtures::metadata()}),
        &["message_outgoing", "accepted_at", "expires_at"],
    );
    public_contract::<proto::StatusRequest>(
        &registry,
        json!({"attachmentRef": fixtures::attachment_ref(), "idempotencyKey": "send_outgoing"}),
        &["send_outgoing", "generation"],
    );
    public_contract::<proto::StatusResponse>(
        &registry,
        json!({"message": fixtures::metadata(), "status": "MESSAGE_STATUS_BATCH_PREPARED", "observedAt": fixtures::PREPARED}),
        &["message_outgoing", "status", "observed_at"],
    );
    public_contract::<proto::PrepareBatchRequest>(
        &registry,
        json!({"attachmentRef": fixtures::attachment_ref(), "promptId": "prompt_alpha"}),
        &["peer_alpha", "prompt_alpha"],
    );
    public_contract::<proto::PrepareBatchResponse>(
        &registry,
        json!({"batch": fixtures::batch(), "messages": [fixtures::prepared_message("message_incoming_one"), fixtures::prepared_message("message_incoming_two")], "contentAvailability": "CONTENT_AVAILABILITY_AVAILABLE"}),
        &["batch_local", "message_incoming_one", "content_availability"],
    );
    public_contract::<proto::AcknowledgeBatchRequest>(
        &registry,
        json!({"attachmentRef": fixtures::attachment_ref(), "batchId": "batch_local"}),
        &["connector_alpha", "batch_local"],
    );
    public_contract::<proto::AcknowledgeBatchResponse>(
        &registry,
        json!({"batch": fixtures::acknowledged_batch()}),
        &["batch_local", "acknowledged_at"],
    );
}

#[test]
fn package_registry_does_not_guess_unregistered_or_cross_package_payload_types() {
    let mut registry = registry();
    proto::register_types(&mut registry);
    state::register_types(&mut registry);
    assert!(
        registry
            .json_any_by_url("type.googleapis.com/trogonai.peers.v1alpha1.FuturePeerFact")
            .is_none()
    );
    assert!(
        registry
            .json_any_by_url("type.googleapis.com/trogonai.peers.state.v1alpha1.PeerEvent")
            .is_none()
    );
    assert!(registry.json_any_by_url(proto::PeerEvent::TYPE_URL).is_some());
    assert!(registry.json_any_by_url(state::PeerState::TYPE_URL).is_some());
}

fn enum_proto_json<E>()
where
    E: Enumeration + ProtoElemJson + Serialize + DeserializeOwned + Debug + PartialEq + Copy + 'static,
{
    let batch = ProtoBatch {
        messages: E::values().to_vec(),
    };
    let named = json!({"messages": E::values().iter().map(|value| value.proto_name()).collect::<Vec<_>>()});
    assert_eq!(serde_json::to_value(&batch).unwrap(), named);
    let decoded: ProtoBatch<E> = serde_json::from_value(named).unwrap();
    assert_eq!(decoded.messages, batch.messages);
    let numeric = json!({"messages": E::values().iter().map(|value| value.to_i32()).collect::<Vec<_>>()});
    let decoded: ProtoBatch<E> = serde_json::from_value(numeric).unwrap();
    assert_eq!(decoded.messages, batch.messages);
    assert!(serde_json::from_value::<ProtoBatch<E>>(json!({"messages": [999]})).is_err());
}

#[test]
fn proto_json_enum_collections_preserve_known_names_order_and_numeric_input() {
    enum_proto_json::<proto::AttachmentStatus>();
    enum_proto_json::<proto::PeerAvailability>();
    enum_proto_json::<proto::PeerCondition>();
    enum_proto_json::<proto::DeliveryCapability>();
    enum_proto_json::<proto::PreparationOutcome>();
    enum_proto_json::<proto::ContentAvailability>();
    enum_proto_json::<proto::MessageStatus>();
}

#[test]
fn every_event_case_rejects_ambiguous_json_while_null_and_future_fields_create_no_fact() {
    for event in fixtures::events() {
        let case = event.as_object().unwrap().keys().next().unwrap();
        let previous = if case == "peerEnrolled" {
            "peerDetached"
        } else {
            "peerEnrolled"
        };
        let ambiguous = format!("{{\"{previous}\":{{}},\"{case}\":{{}}}}");
        assert!(serde_json::from_str::<proto::PeerEvent>(&ambiguous).is_err());
        let mut null = serde_json::Map::new();
        null.insert(case.to_owned(), Value::Null);
        let absent: proto::PeerEvent = serde_json::from_value(Value::Object(null)).unwrap();
        assert!(absent.event.is_none());
    }
    let future: proto::PeerEvent = serde_json::from_value(json!({"futureFact": {"nested": ["ignored"]}})).unwrap();
    assert!(future.event.is_none());
}

#[test]
fn malformed_or_duplicate_fields_inside_each_oneof_payload_propagate_as_json_data_errors() {
    for (case, field, value) in [
        ("peerEnrolled", "enrollment", "{}"),
        ("peerAttached", "attachment", "{}"),
        ("peerHeartbeatRecorded", "attachment", "{}"),
        ("peerDetached", "attachmentRef", "{}"),
        ("peerMessageAccepted", "entry", "{}"),
        ("peerMessageIntakeRecorded", "entry", "{}"),
        ("peerBatchPrepared", "batch", "{}"),
        ("peerBatchAcknowledged", "batchId", "\"batch_local\""),
        ("peerMessageDisposed", "receipt", "{}"),
        ("peerMessageSettled", "receipt", "{}"),
    ] {
        let duplicate = format!("{{\"{case}\":{{\"{field}\":{value},\"{field}\":{value}}}}}");
        let error = serde_json::from_str::<proto::PeerEvent>(&duplicate).unwrap_err();
        assert_eq!(error.classify(), serde_json::error::Category::Data);
        assert_eq!(error.line(), 1);
        assert!(error.column() > 0);
        let malformed = format!("{{\"{case}\":false}}");
        let error = serde_json::from_str::<proto::PeerEvent>(&malformed).unwrap_err();
        assert_eq!(error.classify(), serde_json::error::Category::Data);
    }
}
