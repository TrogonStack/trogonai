use buffa::{Message, MessageField};
use trogonai_proto::platform::audit::v1::{AuditRecord, Integrity};

use super::*;
use crate::constants::RECORD_HASH_BYTES;
use crate::fixtures::{deny, stamper};

const CHAIN: &str = "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f01";

fn chained_message() -> AuditRecord {
    let (_, _, _, mut message) = deny(&stamper()).into_message();
    message.integrity = MessageField::some(Integrity {
        writer_id: "gateway-1".to_string(),
        sequence: 2,
        previous_hash: Some(vec![9; RECORD_HASH_BYTES]),
        record_hash: None,
        chain_id: Some(CHAIN.to_string()),
    });
    message
}

fn sealed() -> (Bytes, RecordHash, Bytes) {
    let body = Bytes::from(chained_message().encode_to_vec());
    let (hash, payload) = seal(body.clone());
    (body, hash, payload)
}

#[test]
fn the_trailer_decodes_as_the_record_hash_of_the_body() {
    let (body, hash, payload) = sealed();

    assert_eq!(hash, RecordHash::of(&body));
    assert_eq!(&payload[..body.len()], &body[..]);

    let decoded = AuditRecord::decode_from_slice(&payload).unwrap();
    let mut expected = chained_message();
    expected.integrity.get_or_insert_default().record_hash = Some(hash.as_bytes().to_vec());
    assert_eq!(decoded, expected);
    assert_eq!(decoded.integrity.writer_id, "gateway-1");
    assert_eq!(decoded.integrity.sequence, 2);
    assert_eq!(decoded.integrity.previous_hash, Some(vec![9; RECORD_HASH_BYTES]));
    assert_eq!(decoded.integrity.chain_id.as_deref(), Some(CHAIN));
}

#[test]
fn the_body_alone_decodes_without_a_record_hash() {
    let (body, _, _) = sealed();

    let decoded = AuditRecord::decode_from_slice(&body).unwrap();
    assert_eq!(decoded, chained_message());
    assert_eq!(decoded.integrity.record_hash, None);
}

#[test]
fn the_trailer_is_a_length_delimited_record_hash_inside_integrity() {
    assert_eq!(SEAL_TRAILER_PREFIX, [0x62, 0x22, 0x22, 0x20]);
    assert_eq!(SEAL_TRAILER_BYTES, 36);
}

#[test]
fn opens_what_it_sealed() {
    let (body, hash, payload) = sealed();

    assert_eq!(open(&payload), Ok((&body[..], hash)));
}

#[test]
fn opens_an_empty_body() {
    let (hash, payload) = seal(Bytes::new());

    assert_eq!(payload.len(), SEAL_TRAILER_BYTES);
    assert_eq!(open(&payload), Ok((&[][..], hash)));
}

#[test]
fn rejects_a_payload_shorter_than_the_trailer() {
    let (_, _, payload) = sealed();
    let tail = &payload[payload.len() - (SEAL_TRAILER_BYTES - 1)..];

    assert_eq!(open(tail), Err(SealError::TooShort(SEAL_TRAILER_BYTES - 1)));
    assert_eq!(open(&[]), Err(SealError::TooShort(0)));
}

#[test]
fn rejects_a_wrong_prefix() {
    let (body, _, payload) = sealed();

    for index in 0..SEAL_TRAILER_PREFIX.len() {
        let mut tampered = payload.to_vec();
        tampered[body.len() + index] ^= 0x01;
        assert_eq!(open(&tampered), Err(SealError::MissingTrailer));
    }
}

#[test]
fn rejects_unsealed_bytes() {
    let (body, _, _) = sealed();

    assert_eq!(open(&body), Err(SealError::MissingTrailer));
}

#[test]
fn rejects_a_flipped_body_bit() {
    let (body, _, payload) = sealed();
    let mut tampered = payload.to_vec();
    tampered[body.len() / 2] ^= 0x01;

    assert_eq!(open(&tampered), Err(SealError::HashMismatch));
}

#[test]
fn rejects_a_flipped_hash_bit() {
    let (_, _, payload) = sealed();
    let mut tampered = payload.to_vec();
    *tampered.last_mut().unwrap() ^= 0x80;

    assert_eq!(open(&tampered), Err(SealError::HashMismatch));
}
