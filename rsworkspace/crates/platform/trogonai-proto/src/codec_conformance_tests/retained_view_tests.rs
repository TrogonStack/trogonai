use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, Message, ViewEncode};
use serde_json::json;

use crate::content::v1alpha1::ContentOwnedView;
use crate::scheduler::schedules::v1::{Schedule, ScheduleOwnedView};

#[test]
fn retained_buffer_outlives_original_owner_and_transfers_original_wire() {
    let wire = b"\x12\x03\x00\xff\x01\xf8\x07\x01\x0a\x03old\x0a\x0atext/plain";
    let handle = {
        let original = Bytes::copy_from_slice(wire);
        ContentOwnedView::decode(original.clone()).expect("retained content")
    };
    assert_eq!(
        serde_json::to_value(handle.view()).expect("borrowed content JSON"),
        json!({"contentType": "text/plain", "data": "AP8B"})
    );
    assert_eq!(handle.bytes().as_ref(), wire);
    let transferred = handle.into_bytes();
    assert_eq!(transferred.as_ref(), wire);
    let decoded = ContentOwnedView::decode(transferred).expect("transferred content");
    assert_eq!(
        serde_json::to_value(decoded.to_owned_message()).expect("owned content JSON"),
        json!({"contentType": "text/plain", "data": "AP8B"})
    );
}

#[test]
fn from_owned_retains_nested_data_after_source_is_cleared_and_dropped() {
    let expected = json!({"cron": {
        "expr": "0 9 * * *", "timezone": {"id": "Europe/Paris", "version": "2025b"}
    }});
    let handle = {
        let mut source: Schedule = serde_json::from_value(expected.clone()).expect("schedule");
        let handle = ScheduleOwnedView::from_owned(&source).expect("retained schedule");
        source.clear();
        handle
    };
    assert_eq!(serde_json::to_value(handle.view()).expect("nested view JSON"), expected);
    let transferred = handle.into_bytes();
    let decoded = Schedule::decode_from_slice(&transferred).expect("transferred schedule");
    assert_eq!(serde_json::to_value(decoded).expect("owned schedule JSON"), expected);
}

#[test]
fn decode_limits_measure_original_wire_and_reencoding_does_not_mutate_it() {
    let wire = b"\x12\x03\x00\xff\x01\xf8\x07\x01\x0a\x03old\x0a\x0atext/plain";
    let encoded = b"\x0a\x0atext/plain\x12\x03\x00\xff\x01";
    let logical_size = DecodeOptions::new().with_max_message_size(encoded.len());
    assert_eq!(
        ContentOwnedView::decode_with_options(Bytes::copy_from_slice(wire), &logical_size).err(),
        Some(DecodeError::MessageTooLarge)
    );
    let original_size = DecodeOptions::new().with_max_message_size(wire.len());
    let handle = ContentOwnedView::decode_with_options(Bytes::copy_from_slice(wire), &original_size)
        .expect("original wire fits limit");
    assert_eq!(handle.view().try_encode_to_vec().expect("view encode"), encoded);
    assert_eq!(handle.into_bytes().as_ref(), wire);
}
