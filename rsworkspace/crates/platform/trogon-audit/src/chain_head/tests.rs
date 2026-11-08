use super::*;

#[test]
fn keeps_the_acknowledged_position() {
    let hash = RecordHash::of(b"record");
    let head = ChainHead::new(3, hash, "AUDIT", 41);
    assert_eq!(head.sequence(), 3);
    assert_eq!(head.hash(), hash);
    assert_eq!(head.stream(), "AUDIT");
    assert_eq!(head.stream_sequence(), 41);
}
