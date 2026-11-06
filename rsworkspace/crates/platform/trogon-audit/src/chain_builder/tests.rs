use super::*;
use crate::fixtures::{CHAIN, body, chain_builder, deny, stamper, writer_id};
use crate::verify_chain_range::verify_chain_range;

#[test]
fn builds_a_chain_the_verifier_accepts() {
    let mut builder = chain_builder();
    let stamper = stamper();
    let records = vec![
        builder.chain_reset(stamper.stamp(), 1),
        builder.record(deny(&stamper), 2),
        builder.record(deny(&stamper), 3),
    ];

    let range = verify_chain_range(builder.genesis(), records).unwrap();

    assert_eq!(range.head().sequence(), 3);
    assert_eq!(range.head().writer(), &writer_id());
}

#[test]
fn links_each_record_to_the_one_before() {
    let mut builder = chain_builder();
    let stamper = stamper();
    let genesis = builder.chain_reset(stamper.stamp(), 1);
    let next = builder.record(deny(&stamper), 2);

    let genesis_integrity = body(&genesis).integrity.as_option().cloned().unwrap();
    assert_eq!(genesis_integrity.sequence, 1);
    assert_eq!(genesis_integrity.previous_hash, None);
    assert_eq!(genesis_integrity.chain_id.as_deref(), Some(CHAIN));

    let (genesis_body, genesis_hash) = record_seal::open(genesis.payload()).unwrap();
    assert_eq!(genesis_hash, RecordHash::of(genesis_body));
    let next_integrity = body(&next).integrity.as_option().cloned().unwrap();
    assert_eq!(next_integrity.sequence, 2);
    assert_eq!(next_integrity.previous_hash, Some(genesis_hash.as_bytes().to_vec()));
}

#[test]
fn stores_records_with_the_subject_and_headers_a_writer_sets() {
    let mut builder = chain_builder();
    let genesis = builder.chain_reset(stamper().stamp(), 7);

    assert_eq!(genesis.stream_sequence(), 7);
    assert_eq!(
        genesis.subject(),
        Some("platform.v1.audit.integrity.egress_proxy.chain_reset")
    );
    assert_eq!(genesis.message_id(), Some(body(&genesis).record_id.as_str()));
    assert_eq!(genesis.writer(), Some("gateway-1"));
    assert_eq!(genesis.sequence(), Some("1"));
    assert_eq!(genesis.chain(), Some(CHAIN));
}
