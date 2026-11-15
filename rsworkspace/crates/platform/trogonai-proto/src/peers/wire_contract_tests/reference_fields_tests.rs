use serde_json::json;

use buffa::Message;

use super::{assert_json_codec, assert_wire_codec, fixtures};
use crate::peers::v1alpha1 as proto;

#[test]
fn attachment_fencing_tokens_use_reference_json_names() {
    let reference = json!({"attachmentRef": fixtures::attachment_ref()});
    assert_json_codec::<proto::HeartbeatRequest>(reference.clone());
    assert_json_codec::<proto::DetachRequest>(reference.clone());
    assert_json_codec::<proto::RosterRequest>(reference.clone());
    assert_json_codec::<proto::SendRequest>(reference.clone());
    assert_json_codec::<proto::StatusRequest>(reference.clone());
    assert_json_codec::<proto::PrepareBatchRequest>(reference.clone());
    assert_json_codec::<proto::AcknowledgeBatchRequest>(reference.clone());
    assert_json_codec::<proto::PeerAttachment>(reference.clone());
    assert_json_codec::<proto::PeerDetached>(reference);
}

#[test]
fn inline_attachment_leases_keep_their_json_names() {
    let lease = json!({"attachment": fixtures::attachment()});
    assert_json_codec::<proto::EnrollResponse>(lease.clone());
    assert_json_codec::<proto::HeartbeatResponse>(lease.clone());
    assert_json_codec::<proto::PeerAttached>(lease.clone());
    assert_json_codec::<proto::PeerHeartbeatRecorded>(lease);
}

#[test]
fn content_reference_binds_the_locator_and_digest_in_one_json_value() {
    let mut metadata = fixtures::metadata();
    metadata["contentRef"] = json!({
        "storageRef": {"value": "content_outgoing"},
        "digest": {"algorithm": "sha256", "value": fixtures::DIGEST}
    });
    metadata.as_object_mut().unwrap().remove("contentDigest");
    assert_json_codec::<proto::MessageMetadata>(metadata);
}

#[test]
fn retired_content_fields_do_not_decode_as_a_structured_reference() {
    let legacy = [0x0a, 3, b'm', b's', b'g', 0x2a, 3, b'o', b'l', b'd', 0x3a, 2, 0, 1];
    let expected = proto::MessageMetadata::default().with_message_id("msg");
    assert_wire_codec(&legacy, &expected);
}

#[test]
fn semantic_fingerprint_uses_a_new_message_field_without_changing_claim_check_integrity() {
    let reference: proto::ClaimCheck = serde_json::from_value(fixtures::content_ref()).unwrap();
    let reference_wire = reference.encode_to_vec();
    let semantic = proto::ContentSemanticFingerprint::default().with_value(vec![1; 32]);
    let semantic_wire = [vec![0x0a, 32], vec![1; 32]].concat();
    assert_wire_codec(&semantic_wire, &semantic);

    let mut wire = vec![0x5a, u8::try_from(reference_wire.len()).unwrap()];
    wire.extend_from_slice(&reference_wire);
    wire.extend_from_slice(&[0x62, 34]);
    wire.extend_from_slice(&semantic_wire);
    let metadata = proto::MessageMetadata {
        content_ref: buffa::MessageField::some(reference),
        semantic_fingerprint: buffa::MessageField::some(semantic),
        ..Default::default()
    };
    assert_wire_codec(&wire, &metadata);
    let json = serde_json::to_value(&metadata).unwrap();
    assert_eq!(json["contentRef"], fixtures::content_ref());
    assert_eq!(json["semanticFingerprint"], fixtures::semantic_fingerprint());
    assert!(json.get("contentDigest").is_none());

    let mut extended = wire;
    extended.extend_from_slice(&[0x2a, 3, b'o', b'l', b'd', 0x3a, 2, 9, 9]);
    assert_wire_codec(&extended, &metadata);
}

#[test]
fn semantic_fingerprint_presence_and_json_aliases_do_not_infer_integrity_or_semantic_values() {
    let integrity_only = assert_json_codec::<proto::MessageMetadata>(json!({"contentRef": fixtures::content_ref()}));
    assert!(!integrity_only.semantic_fingerprint.is_set());
    let semantic_only =
        assert_json_codec::<proto::MessageMetadata>(json!({"semanticFingerprint": fixtures::semantic_fingerprint()}));
    assert!(!semantic_only.content_ref.is_set());
    let empty = assert_json_codec::<proto::ContentSemanticFingerprint>(json!({"value": ""}));
    assert_eq!(empty.value, Some(Vec::new()));
    assert_ne!(empty, proto::ContentSemanticFingerprint::default());
    assert_json_codec::<proto::MessageMetadata>(json!({"semanticFingerprint": {}}));
    assert_json_codec::<proto::MessageMetadata>(json!({"semanticFingerprint": {"value": ""}}));
    assert_json_codec::<proto::ContentSemanticFingerprint>(json!({"value": "/w=="}));
    let alias: proto::MessageMetadata = serde_json::from_value(json!({
        "semantic_fingerprint": fixtures::semantic_fingerprint()
    }))
    .unwrap();
    assert_eq!(alias, semantic_only);
    let nulls: proto::MessageMetadata = serde_json::from_value(json!({"semanticFingerprint": null})).unwrap();
    assert!(!nulls.semantic_fingerprint.is_set());
    for invalid in [
        r#"{"semanticFingerprint":{"value":"!"}}"#,
        r#"{"semanticFingerprint":{"value":true}}"#,
        r#"{"semanticFingerprint":{"value":"AQ==","value":"Ag=="}}"#,
        r#"{"semanticFingerprint":{},"semantic_fingerprint":{}}"#,
    ] {
        assert!(serde_json::from_str::<proto::MessageMetadata>(invalid).is_err());
    }
}
