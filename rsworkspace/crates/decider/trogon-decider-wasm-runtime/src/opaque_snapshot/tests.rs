use trogon_decider_runtime::{
    SnapshotPayloadData, SnapshotPayloadDecode as _, SnapshotPayloadEncode as _, SnapshotType as _,
};

use super::*;

#[test]
fn snapshot_type_name_is_stable() {
    let name = OpaqueSnapshotPayload::snapshot_type().expect("snapshot type name builds");
    assert_eq!(name.as_str(), "wasm-decider-opaque.v1");
}

#[test]
fn encode_decode_round_trips_bytes_untouched() {
    let payload = OpaqueSnapshotPayload::new(vec![9, 8, 7]);
    assert_eq!(payload.as_bytes(), &[9, 8, 7]);
    let encoded = payload.encode().expect("encode is infallible");
    assert_eq!(encoded, vec![9, 8, 7]);

    let decoded = OpaqueSnapshotPayload::decode(SnapshotPayloadData::new(&encoded)).expect("decode is infallible");
    assert_eq!(decoded, payload);
    assert_eq!(decoded.into_bytes(), vec![9, 8, 7]);
}
