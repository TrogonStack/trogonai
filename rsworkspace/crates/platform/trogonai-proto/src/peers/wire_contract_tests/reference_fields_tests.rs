use serde_json::json;

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
    let legacy = [
        0x0a, 3, b'm', b's', b'g', 0x2a, 3, b'o', b'l', b'd', 0x3a, 2, 0, 1,
    ];
    let expected = proto::MessageMetadata::default().with_message_id("msg");
    assert_wire_codec(&legacy, &expected);
}
