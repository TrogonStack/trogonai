use async_nats::HeaderMap;
use buffa::{Message, MessageField};
use bytes::Bytes;
use trogonai_proto::platform::audit::v1::{AuditRecord, Integrity};

use super::*;
use crate::constants::{HEADER_CHAIN, HEADER_WRITER};
use crate::fixtures::{
    CHAIN, OTHER_CHAIN, chain_builder, chain_builder_for, deny, stamper, stored_again, stored_headers, valid_chain,
    with_payload,
};
use crate::seal_error::SealError;

/// Renumbers the records of several chains into one stream, in the order given.
fn interleave(chains: &[&[StoredAuditRecord]], order: &[usize]) -> Vec<StoredAuditRecord> {
    let mut next = vec![0; chains.len()];
    order
        .iter()
        .zip(1..)
        .map(|(&chain, stream_sequence)| {
            let record = &chains[chain][next[chain]];
            next[chain] += 1;
            stored_again(record, stream_sequence)
        })
        .collect()
}

fn key(writer: &str, chain: &str) -> ChainKey {
    ChainKey::new(WriterId::new(writer).unwrap(), ChainId::parse(chain).unwrap())
}

#[test]
fn verifies_two_interleaved_writers_separately() {
    let first = valid_chain(&mut chain_builder_for("gateway-1", CHAIN), 3);
    let second = valid_chain(&mut chain_builder_for("gateway-2", OTHER_CHAIN), 2);
    let stream = interleave(&[&first, &second], &[1, 0, 0, 1, 0]);

    let verification = verify_chains([], stream).unwrap();

    assert!(verification.is_verified());
    let chains = verification.chains();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0].key(), &key("gateway-2", OTHER_CHAIN));
    assert_eq!(chains[1].key(), &key("gateway-1", CHAIN));
    let second_head = chains[0].result().as_ref().unwrap().head();
    assert_eq!((second_head.sequence(), second_head.stream_sequence()), (2, 4));
    let first_head = chains[1].clone().into_result().unwrap().into_head();
    assert_eq!((first_head.sequence(), first_head.stream_sequence()), (3, 5));
}

#[test]
fn verifies_two_chains_of_one_writer_separately() {
    let before_restart = valid_chain(&mut chain_builder_for("gateway-1", CHAIN), 2);
    let after_restart = valid_chain(&mut chain_builder_for("gateway-1", OTHER_CHAIN), 2);
    let stream = interleave(&[&before_restart, &after_restart], &[0, 1, 0, 1]);

    let verification = verify_chains([], stream).unwrap();

    assert!(verification.is_verified());
    let keys: Vec<_> = verification.chains().iter().map(|chain| chain.key().clone()).collect();
    assert_eq!(keys, [key("gateway-1", CHAIN), key("gateway-1", OTHER_CHAIN)]);
}

#[test]
fn a_fault_in_one_chain_leaves_the_others_verified() {
    let first = valid_chain(&mut chain_builder_for("gateway-1", CHAIN), 3);
    let second = valid_chain(&mut chain_builder_for("gateway-2", OTHER_CHAIN), 2);
    let mut stream = interleave(&[&first, &second], &[0, 1, 0, 1, 0]);
    stream.remove(2);

    let verification = verify_chains([], stream).unwrap();

    assert!(!verification.is_verified());
    let chains = verification.chains();
    assert_eq!(
        chains[0].result(),
        &Err(ChainVerificationError::SequenceGap {
            stream_sequence: 5,
            expected: 2,
            observed: 3,
        })
    );
    assert!(chains[1].result().is_ok());
}

#[test]
fn continues_a_chain_from_its_trusted_head_and_opens_the_rest_at_genesis() {
    let mut builder = chain_builder();
    let first = valid_chain(&mut builder, 2);
    let head = verify_chain_range(builder.genesis(), first.clone())
        .unwrap()
        .into_head();
    let stamper = stamper();
    let continued = builder.record(deny(&stamper), 3);
    let second = valid_chain(&mut chain_builder_for("gateway-2", OTHER_CHAIN), 1);
    let stream = vec![continued, stored_again(&second[0], 4)];

    let verification = verify_chains([head], stream).unwrap();

    assert!(verification.is_verified());
    assert_eq!(verification.chains()[0].result().as_ref().unwrap().head().sequence(), 3);
    assert_eq!(verification.chains()[1].result().as_ref().unwrap().head().sequence(), 1);
}

#[test]
fn the_higher_trusted_head_of_one_chain_is_used_when_it_comes_last() {
    let mut builder = chain_builder();
    let records = valid_chain(&mut builder, 4);
    let older = verify_chain_range(builder.genesis(), records[..2].to_vec())
        .unwrap()
        .into_head();
    let newer = verify_chain_range(builder.genesis(), records[..3].to_vec())
        .unwrap()
        .into_head();

    let verification = verify_chains([older, newer], vec![records[3].clone()]).unwrap();

    assert!(verification.is_verified());
}

#[test]
fn the_higher_trusted_head_of_one_chain_is_used_when_it_comes_first() {
    let mut builder = chain_builder();
    let records = valid_chain(&mut builder, 4);
    let older = verify_chain_range(builder.genesis(), records[..2].to_vec())
        .unwrap()
        .into_head();
    let newer = verify_chain_range(builder.genesis(), records[..3].to_vec())
        .unwrap()
        .into_head();

    let verification = verify_chains([newer, older], vec![records[3].clone()]).unwrap();

    assert!(verification.is_verified());
}

#[test]
fn a_record_whose_payload_does_not_decode_is_placed_by_its_headers() {
    let mut stream = valid_chain(&mut chain_builder(), 3);
    stream[1] = with_payload(&stream[1], Bytes::from_static(&[0xff, 0xff, 0xff]));

    let verification = verify_chains([], stream).unwrap();

    assert!(verification.unattributed().is_empty());
    assert_eq!(
        verification.chains()[0].result(),
        &Err(ChainVerificationError::Unsealed {
            stream_sequence: 2,
            source: SealError::TooShort(3),
        })
    );
}

#[test]
fn a_record_naming_no_readable_chain_is_unattributed() {
    let mut stream = valid_chain(&mut chain_builder(), 2);
    stream.push(StoredAuditRecord::new(3, Bytes::from_static(&[0xff])));
    let mut bad_writer = stored_headers(&stream[1]);
    bad_writer.insert(HEADER_WRITER, "not a writer");
    stream.push(StoredAuditRecord::new(4, Bytes::from_static(&[0xff])).with_headers(&bad_writer));
    let mut no_chain = HeaderMap::new();
    no_chain.insert(HEADER_WRITER, "gateway-1");
    stream.push(StoredAuditRecord::new(5, Bytes::new()).with_headers(&no_chain));
    let mut bad_chain = no_chain.clone();
    bad_chain.insert(HEADER_CHAIN, "not a chain");
    stream.push(StoredAuditRecord::new(6, Bytes::new()).with_headers(&bad_chain));
    let unchained = AuditRecord {
        integrity: MessageField::some(Integrity {
            writer_id: "gateway-1".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    stream.push(StoredAuditRecord::new(7, unchained.encode_to_vec()));

    let verification = verify_chains([], stream).unwrap();

    assert_eq!(verification.unattributed(), [3, 4, 5, 6, 7]);
    assert!(verification.chains()[0].result().is_ok());
    assert!(!verification.is_verified());
}

#[test]
fn a_payload_naming_an_invalid_writer_falls_back_to_its_headers() {
    let mut stream = valid_chain(&mut chain_builder(), 2);
    let mut message = crate::fixtures::body(&stream[1]);
    message.integrity.get_or_insert_default().writer_id = "not a writer".to_string();
    stream[1] = crate::fixtures::resealed(&stream[1], &message);

    let verification = verify_chains([], stream).unwrap();

    assert_eq!(
        verification.chains()[0].result(),
        &Err(ChainVerificationError::WrongWriter {
            stream_sequence: 2,
            expected: WriterId::new("gateway-1").unwrap(),
        })
    );
}

#[test]
fn an_empty_range_is_refused() {
    assert_eq!(verify_chains([], []), Err(ChainVerificationError::EmptyRange));
}

#[test]
fn a_range_longer_than_the_bound_is_refused() {
    let length = u64::try_from(MAX_CHAIN_RANGE_RECORDS).unwrap() + 1;
    let stream = valid_chain(&mut chain_builder(), length);

    assert_eq!(verify_chains([], stream), Err(ChainVerificationError::RangeTooLong));
}
