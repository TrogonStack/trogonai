use super::*;

#[test]
fn accessors_return_constructed_fields() {
    let sequence = ChainSequence::FIRST;
    let previous_hash = ChainHash::genesis();
    let event_bytes = CanonicalEventBytes::from_json_slice(br#"{"a":1}"#).expect("valid json");
    let hash = ChainHash::digest(b"anything");

    let entry = ChainEntry::new(sequence, previous_hash.clone(), event_bytes.clone(), hash.clone());

    assert_eq!(entry.sequence(), sequence);
    assert_eq!(entry.previous_hash(), &previous_hash);
    assert_eq!(entry.event_bytes(), &event_bytes);
    assert_eq!(entry.hash(), &hash);
}
