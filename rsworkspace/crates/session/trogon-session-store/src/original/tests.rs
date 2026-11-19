#![allow(clippy::unwrap_used)]

use super::{OriginalFields, read_varint};
use crate::SessionError;

#[test]
fn original_copier_refuses_malformed_unvalidated_fields() {
    let invalid = [
        vec![0x09, 0, 0, 0, 0, 0, 0, 0, 0],
        vec![0x0b],
        vec![0x08, 0x01, 0x08, 0x02],
        vec![0x00, 0x00],
        vec![0x0a, 0x03, 0x01],
        vec![0x0a, 0x80],
        vec![0x80],
    ];
    for original in invalid {
        assert!(matches!(
            OriginalFields::parse(&original),
            Err(SessionError::MalformedWire)
        ));
    }
}

#[test]
fn nested_original_evidence_requires_an_existing_length_delimited_field() {
    let original = [0x08, 0x01];
    let fields = OriginalFields::parse(&original).unwrap();
    assert!(matches!(fields.message(1), Err(SessionError::MalformedWire)));
    assert!(matches!(fields.message(2), Err(SessionError::MalformedWire)));
    assert!(matches!(
        fields.copy(2, 1, &mut Vec::new()),
        Err(SessionError::MalformedWire)
    ));
}

#[test]
fn ten_byte_varints_accept_only_the_final_bit_that_fits_in_u64() {
    for final_byte in 0u8..=255 {
        let mut original = vec![0x80; 9];
        original.push(final_byte);
        let mut offset = 0;
        let result = read_varint(&original, &mut offset);
        match final_byte {
            0 => assert_eq!(result.unwrap(), 0),
            1 => assert_eq!(result.unwrap(), 1u64 << 63),
            _ => assert!(matches!(result, Err(SessionError::MalformedWire))),
        }
        assert_eq!(offset, 10);
    }
    for length in 0..10 {
        let mut offset = 0;
        assert!(matches!(
            read_varint(&vec![0x80; length], &mut offset),
            Err(SessionError::MalformedWire)
        ));
    }
}

#[test]
fn optional_original_copy_preserves_large_nested_evidence_and_present_empty_values() {
    let mut original = vec![0x0a, 0x80, 0x01];
    original.extend_from_slice(&[0x17; 128]);
    original.extend_from_slice(&[0x12, 0x00]);
    let fields = OriginalFields::parse(&original).unwrap();
    let mut copied = Vec::new();
    fields.copy_optional(1, 1, &mut copied).unwrap();
    fields.copy_optional(2, 2, &mut copied).unwrap();
    fields.copy_optional(3, 3, &mut copied).unwrap();
    assert_eq!(copied, original);
}

#[test]
fn varint_tenth_byte_accepts_only_the_remaining_high_bit() {
    for final_byte in 0u8..=255 {
        let mut original = vec![0xff; 9];
        original.push(final_byte);
        let mut offset = 0;
        let result = read_varint(&original, &mut offset);
        match final_byte {
            0 => assert_eq!(result.unwrap(), i64::MAX as u64),
            1 => assert_eq!(result.unwrap(), u64::MAX),
            _ => assert!(matches!(result, Err(SessionError::MalformedWire))),
        }
        assert_eq!(offset, 10);
    }
    for count in 0..10 {
        assert!(matches!(
            read_varint(&vec![0x80; count], &mut 0),
            Err(SessionError::MalformedWire)
        ));
    }
}
