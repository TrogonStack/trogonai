use super::*;

#[test]
fn read_varint_decodes_multibyte_value() {
    // 150 encodes as 0x96 0x01.
    assert!(matches!(read_varint(&[0x96, 0x01], 0), Ok((150, 2))));
}

#[test]
fn read_varint_accepts_max_u64() {
    // u64::MAX is a 10-byte varint terminating in 0x01.
    let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    assert!(matches!(read_varint(&bytes, 0), Ok((u64::MAX, 10))));
}

#[test]
fn read_varint_rejects_overflow_in_tenth_byte() {
    // A 10th byte payload above 0x01 would truncate, so it must be rejected.
    let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
    assert!(matches!(
        read_varint(&bytes, 0),
        Err(SnapshotDecodeError::VarintOverflow)
    ));
}

#[test]
fn read_varint_rejects_truncated_input() {
    assert!(matches!(
        read_varint(&[0x80], 0),
        Err(SnapshotDecodeError::UnexpectedEof)
    ));
}
