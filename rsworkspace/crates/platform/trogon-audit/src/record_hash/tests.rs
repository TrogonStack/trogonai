use super::*;

#[test]
fn is_the_sha256_of_the_bytes() {
    let hash = RecordHash::of(b"abc");
    let expected = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23, 0xb0, 0x03,
        0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
    ];
    assert_eq!(hash.as_bytes(), &expected);
    assert_eq!(hash, RecordHash::new(expected));
}

#[test]
fn accepts_exactly_thirty_two_bytes() {
    let bytes = [7; RECORD_HASH_BYTES];
    assert_eq!(RecordHash::from_slice(&bytes).unwrap().as_bytes(), &bytes);
}

#[test]
fn rejects_any_other_length() {
    assert_eq!(RecordHash::from_slice(&[]), Err(RecordHashError::WrongLength(0)));
    assert_eq!(RecordHash::from_slice(&[7; 31]), Err(RecordHashError::WrongLength(31)));
    assert_eq!(RecordHash::from_slice(&[7; 33]), Err(RecordHashError::WrongLength(33)));
}

#[test]
fn the_error_message_states_the_rule() {
    assert_eq!(
        RecordHashError::WrongLength(3).to_string(),
        "record hash must be 32 bytes, got 3"
    );
}
