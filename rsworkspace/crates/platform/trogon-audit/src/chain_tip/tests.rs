use super::*;
use crate::fixtures::{CHAIN, chain_id, deny, identity, stamper, writer_id};

fn genesis() -> SealedRecord {
    let record = Record::chain_reset(&identity(), stamper().stamp());
    SealedRecord::seal(record, &writer_id(), chain_id(CHAIN), 1, None)
}

#[test]
fn opens_with_the_genesis_pending() {
    let genesis = genesis();
    let tip = ChainTip::opened_by(genesis.clone());
    assert_eq!(tip.pending.unwrap().record, genesis);
}

#[test]
fn numbers_the_next_record_after_the_last_and_links_its_hash() {
    let genesis = genesis();
    let mut tip = ChainTip::opened_by(genesis.clone());
    tip.pending = None;

    tip.append(deny(&stamper()), &writer_id(), chain_id(CHAIN)).unwrap();

    let next = tip.pending.unwrap().record;
    assert_eq!(next.sequence(), 2);
    assert_eq!(next.previous_hash(), Some(genesis.hash()));
    assert_eq!(next.chain(), chain_id(CHAIN));
}

#[test]
fn a_chain_at_the_last_sequence_hands_the_record_back() {
    let mut tip = ChainTip::at(u64::MAX, RecordHash::of(b"last"));
    let record = deny(&stamper());

    assert_eq!(
        tip.append(record.clone(), &writer_id(), chain_id(CHAIN)),
        Err(Box::new(record))
    );
    assert!(tip.pending.is_none());
}
