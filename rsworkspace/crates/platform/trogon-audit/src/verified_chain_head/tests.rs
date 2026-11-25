use uuid::Uuid;

use super::*;

fn key() -> ChainKey {
    let chain = ChainId::from_uuid(Uuid::parse_str("01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f01").unwrap()).unwrap();
    ChainKey::new(WriterId::new("gateway-1").unwrap(), chain)
}

#[test]
fn reports_where_the_chain_stood() {
    let hash = RecordHash::of(b"head");
    let head = VerifiedChainHead::new(key(), 4, hash, 40);

    assert_eq!(head.key(), &key());
    assert_eq!(head.writer(), key().writer());
    assert_eq!(head.chain(), key().chain());
    assert_eq!(head.sequence(), 4);
    assert_eq!(head.hash(), hash);
    assert_eq!(head.stream_sequence(), 40);
}

#[test]
fn a_restored_copy_moves_only_the_stream_sequence() {
    let hash = RecordHash::of(b"head");
    let head = VerifiedChainHead::new(key(), 4, hash, 40).restored_at(41);

    assert_eq!(head, VerifiedChainHead::new(key(), 4, hash, 41));
}
