#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;
use trogon_proto_preflight::{WireLimits, WireMessageDescriptor, WirePreflightError, WireSchemaPool, preflight};

fn descriptor() -> WireMessageDescriptor {
    let descriptor = json!({"file":[{"syntax":"proto3","package":"telemetry","enumType":[
        {"name":"State","value":[{"name":"UNSPECIFIED","number":0},{"name":"FAILED","number":-1}]}
    ],"messageType":[{"name":"Readings","field":[
        {"name":"counts","number":1,"type":"TYPE_UINT32","label":"LABEL_REPEATED"},
        {"name":"checksums","number":2,"type":"TYPE_FIXED32","label":"LABEL_REPEATED"},
        {"name":"measurements","number":3,"type":"TYPE_DOUBLE","label":"LABEL_REPEATED"},
        {"name":"offset","number":4,"type":"TYPE_INT32"},
        {"name":"signed_offset","number":5,"type":"TYPE_SINT32"},
        {"name":"state","number":6,"type":"TYPE_ENUM","typeName":".telemetry.State"},
        {"name":"sequence","number":7,"type":"TYPE_UINT64"},
        {"name":"payload","number":8,"type":"TYPE_BYTES"}
    ]}]}]});
    WireSchemaPool::from_descriptor_set_json(&serde_json::to_vec(&descriptor).unwrap())
        .unwrap()
        .message_by_name("telemetry.Readings")
        .unwrap()
}

fn limits() -> WireLimits {
    WireLimits::new(4096, 8, 128).unwrap()
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value >= 128 {
        bytes.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes
}

fn packed(tag: u8, values: &[u8]) -> Vec<u8> {
    assert!(values.len() < 128);
    [&[tag, values.len() as u8], values].concat()
}

#[test]
fn repeated_numbers_accept_packed_unpacked_and_mixed_segments() {
    let descriptor = descriptor();
    let counts = [vec![0, 127], varint(u64::from(u32::MAX))].concat();
    let checksums = [0x12345678u32.to_le_bytes(), u32::MAX.to_le_bytes()].concat();
    let measurements = [1.5f64.to_le_bytes(), (-2.25f64).to_le_bytes()].concat();
    let bytes = [
        packed(0x0a, &counts),
        vec![0x08, 1],
        packed(0x0a, &[]),
        packed(0x12, &checksums),
        [&[0x15], &0u32.to_le_bytes()[..]].concat(),
        packed(0x1a, &measurements),
        [&[0x19], &0.0f64.to_le_bytes()[..]].concat(),
    ]
    .concat();
    assert_eq!(preflight(&descriptor, &bytes, limits()), Ok(()));
}

#[test]
fn packed_payloads_preserve_scalar_validation_and_item_budgets() {
    let descriptor = descriptor();
    for bytes in [packed(0x12, &[1, 2, 3]), packed(0x1a, &[0; 7]), vec![0x0a, 2, 1]] {
        assert_eq!(
            preflight(&descriptor, &bytes, limits()),
            Err(WirePreflightError::Truncated)
        );
    }
    assert_eq!(
        preflight(&descriptor, &packed(0x0a, &varint(1u64 << 32)), limits()),
        Err(WirePreflightError::InvalidInteger { field_number: 1 })
    );
    let bytes = packed(0x0a, &[1, 2]);
    assert_eq!(
        preflight(&descriptor, &bytes, WireLimits::new(4096, 8, 2).unwrap()),
        Err(WirePreflightError::FieldLimit)
    );
    assert_eq!(
        preflight(&descriptor, &bytes, WireLimits::new(4096, 8, 3).unwrap()),
        Ok(())
    );
}

#[test]
fn integer_domains_refuse_overflow_without_truncating_to_a_valid_value() {
    let descriptor = descriptor();
    for (tag, number, invalid) in [
        (0x08, 1, 1u64 << 32),
        (0x28, 5, 1u64 << 32),
        (0x20, 4, i32::MAX as u64 + 1),
        (0x30, 6, i32::MAX as u64 + 1),
        (0x20, 4, (i64::from(i32::MIN) - 1) as u64),
    ] {
        let bytes = [vec![tag], varint(invalid)].concat();
        assert_eq!(
            preflight(&descriptor, &bytes, limits()),
            Err(WirePreflightError::InvalidInteger { field_number: number })
        );
    }
    for (tag, value) in [
        (0x08, u64::from(u32::MAX)),
        (0x28, u64::from(u32::MAX)),
        (0x20, i32::MAX as u64),
        (0x20, i64::from(i32::MIN) as u64),
        (0x30, u64::MAX),
    ] {
        assert_eq!(
            preflight(&descriptor, &[vec![tag], varint(value)].concat(), limits()),
            Ok(())
        );
    }
}

#[test]
fn tenth_varint_byte_admits_only_the_remaining_unsigned_bit() {
    let descriptor = descriptor();
    for terminal in 0..=u8::MAX {
        let bytes = [vec![0x38], vec![0x80; 9], vec![terminal]].concat();
        let expected = if terminal <= 1 {
            Ok(())
        } else {
            Err(WirePreflightError::MalformedVarint)
        };
        assert_eq!(
            preflight(&descriptor, &bytes, limits()),
            expected,
            "terminal byte {terminal}"
        );
    }
    for continuations in 0..=9 {
        let bytes = [vec![0x38], vec![0x80; continuations]].concat();
        assert_eq!(
            preflight(&descriptor, &bytes, limits()),
            Err(WirePreflightError::Truncated)
        );
    }
}

#[test]
fn byte_payloads_keep_arbitrary_content_and_refuse_truncated_lengths() {
    let descriptor = descriptor();
    assert_eq!(preflight(&descriptor, &[66, 2, 0xff, 0], limits()), Ok(()));
    assert_eq!(
        preflight(&descriptor, &[66, 2, 0xff], limits()),
        Err(WirePreflightError::Truncated)
    );
}
