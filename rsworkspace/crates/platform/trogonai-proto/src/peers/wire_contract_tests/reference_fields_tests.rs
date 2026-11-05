use serde_json::json;

use super::{assert_json_codec, fixtures};
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
