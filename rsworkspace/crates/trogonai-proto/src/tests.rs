use buffa::{Message as _, MessageName as _};

use crate::example::v1::LightTurnedOn;

#[test]
fn decode_event_to_json_is_canonical_across_wire_orderings() {
    let event = LightTurnedOn {
        light_id: "kitchen".to_string(),
        turn_on_count: 3,
    };
    let canonical = event.encode_to_vec();

    // The same message with fields written in reverse wire order (field 2
    // before field 1). Protobuf permits any field order, so this is a valid
    // alternate encoding that differs on the wire.
    let reordered = {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x10, 0x03]); // field 2 (turn_on_count) varint = 3
        bytes.push(0x0A); // field 1 (light_id) length-delimited
        bytes.push(7);
        bytes.extend_from_slice(b"kitchen");
        bytes
    };
    assert_ne!(canonical, reordered, "encodings must differ on the wire");

    let from_canonical = super::decode_event_to_json(LightTurnedOn::FULL_NAME, &canonical);
    let from_reordered = super::decode_event_to_json(LightTurnedOn::FULL_NAME, &reordered);

    assert!(from_canonical.is_some());
    assert_eq!(from_canonical, from_reordered);
}

#[test]
fn decode_event_to_json_returns_none_for_unknown_type() {
    assert_eq!(
        super::decode_event_to_json("type.googleapis.com/trogonai.example.light.v1.Unknown", &[]),
        None
    );
}
