use super::*;

#[test]
fn hash_mismatch_display_includes_sequence_and_hashes() {
    let sequence = ChainSequence::FIRST;
    let expected = ChainHash::digest(b"expected");
    let found = ChainHash::digest(b"found");
    let err = ChainBreak::HashMismatch {
        sequence,
        expected: expected.clone(),
        found: found.clone(),
    };
    let message = err.to_string();
    assert!(message.contains(&sequence.to_string()));
    assert!(message.contains(expected.as_str()));
    assert!(message.contains(found.as_str()));
}

#[test]
fn does_not_start_at_one_display_includes_found_sequence() {
    let found = ChainSequence::try_new(2).expect("valid sequence");
    let err = ChainBreak::DoesNotStartAtOne { found };
    assert!(err.to_string().contains('2'));
}

#[test]
fn sequence_gap_display_includes_both_sequences() {
    let expected = ChainSequence::try_new(2).expect("valid sequence");
    let found = ChainSequence::try_new(4).expect("valid sequence");
    let err = ChainBreak::SequenceGap { expected, found };
    let message = err.to_string();
    assert!(message.contains('2'));
    assert!(message.contains('4'));
}
