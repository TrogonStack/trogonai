use super::*;
use crate::canonical_event_bytes::CanonicalEventBytes;

fn event(json: &str) -> CanonicalEventBytes {
    CanonicalEventBytes::from_json_slice(json.as_bytes()).expect("valid json fixture")
}

/// Builds a valid chain of `events.len()` entries, hashing each one in order.
fn build_valid_chain(events: &[&str]) -> Vec<ChainEntry> {
    let mut previous_hash = ChainHash::genesis();
    let mut sequence = ChainSequence::FIRST;
    let mut entries = Vec::with_capacity(events.len());

    for raw in events {
        let event_bytes = event(raw);
        let hash = extend(&previous_hash, &event_bytes);
        entries.push(ChainEntry::new(
            sequence,
            previous_hash.clone(),
            event_bytes,
            hash.clone(),
        ));
        previous_hash = hash;
        sequence = sequence.next();
    }

    entries
}

#[test]
fn empty_chain_verifies() {
    assert_eq!(verify_chain(Vec::new()), Ok(()));
}

#[test]
fn single_entry_chain_verifies() {
    let chain = build_valid_chain(&[r#"{"a":1}"#]);
    assert_eq!(verify_chain(chain), Ok(()));
}

#[test]
fn multi_entry_chain_verifies() {
    let chain = build_valid_chain(&[r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#, r#"{"a":4}"#]);
    assert_eq!(verify_chain(chain), Ok(()));
}

#[test]
fn tamper_modified_payload_is_detected() {
    let mut chain = build_valid_chain(&[r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#]);
    let first_hash = chain[0].hash().clone();
    let original_hash_at_2 = chain[1].hash().clone();

    // Mutate the middle entry's event bytes without recomputing its hash,
    // simulating an attacker editing stored payload bytes in place.
    chain[1] = ChainEntry::new(
        chain[1].sequence(),
        chain[1].previous_hash().clone(),
        event(r#"{"a":999}"#),
        original_hash_at_2.clone(),
    );

    let expected_hash = extend(&first_hash, &event(r#"{"a":999}"#));
    let found_hash = extend(&first_hash, &event(r#"{"a":2}"#));
    assert_eq!(
        found_hash, original_hash_at_2,
        "sanity check on the fixture's original hash"
    );

    assert_eq!(
        verify_chain(chain),
        Err(ChainBreak::HashMismatch {
            sequence: ChainSequence::try_new(2).expect("valid"),
            expected: expected_hash,
            found: found_hash,
        })
    );
}

#[test]
fn tamper_reordered_entries_is_detected() {
    // Swapping whole entries carries each entry's own recorded sequence
    // number along with it, so the reorder first surfaces as a sequence gap
    // (position 2 now holds the entry recorded as sequence 3) rather than as
    // a broken hash link.
    let chain = build_valid_chain(&[r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#]);
    let mut reordered = chain.clone();
    reordered.swap(1, 2);

    let result = verify_chain(reordered);
    assert_eq!(
        result,
        Err(ChainBreak::SequenceGap {
            expected: ChainSequence::try_new(2).expect("valid"),
            found: ChainSequence::try_new(3).expect("valid"),
        })
    );
}

#[test]
fn tamper_dropped_entry_is_detected() {
    let chain = build_valid_chain(&[r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#]);
    let mut with_gap = chain.clone();
    with_gap.remove(1);

    let result = verify_chain(with_gap);
    assert!(matches!(
        result,
        Err(ChainBreak::SequenceGap { expected, found })
            if expected == ChainSequence::try_new(2).expect("valid")
                && found == ChainSequence::try_new(3).expect("valid")
    ));
}

#[test]
fn tamper_forged_header_with_correct_hash_but_wrong_previous_is_detected() {
    // An attacker who recomputes a self-consistent hash for a forged entry
    // (i.e. hash == extend(claimed_previous, event)) but cannot reproduce the
    // real previous entry's hash still gets caught by the previous-hash
    // linkage check, not the per-entry hash check.
    let chain = build_valid_chain(&[r#"{"a":1}"#, r#"{"a":2}"#]);
    let forged_previous = ChainHash::digest(b"forged");
    let forged_event = event(r#"{"a":2}"#);
    let forged_hash = extend(&forged_previous, &forged_event);
    let forged_entry = ChainEntry::new(
        ChainSequence::try_new(2).expect("valid"),
        forged_previous.clone(),
        forged_event,
        forged_hash,
    );

    let mut tampered = chain;
    tampered[1] = forged_entry;

    let result = verify_chain(tampered);
    assert!(matches!(
        result,
        Err(ChainBreak::PreviousHashMismatch { sequence, found, .. })
            if sequence == ChainSequence::try_new(2).expect("valid") && found == forged_previous
    ));
}

#[test]
fn tamper_forged_genesis_previous_hash_is_detected() {
    let event_bytes = event(r#"{"a":1}"#);
    let forged_previous = ChainHash::digest(b"not genesis");
    let hash = extend(&forged_previous, &event_bytes);
    let entry = ChainEntry::new(ChainSequence::FIRST, forged_previous.clone(), event_bytes, hash);

    let result = verify_chain(vec![entry]);
    assert_eq!(
        result,
        Err(ChainBreak::PreviousHashMismatch {
            sequence: ChainSequence::FIRST,
            expected: ChainHash::genesis(),
            found: forged_previous,
        })
    );
}

#[test]
fn chain_not_starting_at_sequence_one_is_detected() {
    let event_bytes = event(r#"{"a":1}"#);
    let hash = extend(&ChainHash::genesis(), &event_bytes);
    let entry = ChainEntry::new(
        ChainSequence::try_new(2).expect("valid"),
        ChainHash::genesis(),
        event_bytes,
        hash,
    );

    let result = verify_chain(vec![entry]);
    assert_eq!(
        result,
        Err(ChainBreak::DoesNotStartAtOne {
            found: ChainSequence::try_new(2).expect("valid"),
        })
    );
}

#[test]
fn tamper_swapped_hash_between_two_entries_is_detected() {
    let chain = build_valid_chain(&[r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#]);
    let mut tampered = chain.clone();
    let hash_at_1 = tampered[0].hash().clone();
    let hash_at_2 = tampered[1].hash().clone();
    tampered[0] = ChainEntry::new(
        tampered[0].sequence(),
        tampered[0].previous_hash().clone(),
        tampered[0].event_bytes().clone(),
        hash_at_2,
    );
    tampered[1] = ChainEntry::new(
        tampered[1].sequence(),
        tampered[1].previous_hash().clone(),
        tampered[1].event_bytes().clone(),
        hash_at_1,
    );

    let result = verify_chain(tampered);
    assert!(matches!(
        result,
        Err(ChainBreak::HashMismatch { sequence, .. }) if sequence == ChainSequence::FIRST
    ));
}

#[test]
fn first_break_is_reported_when_multiple_breaks_exist() {
    // Two independent tamper points exist (sequence 2 and sequence 4); only
    // the first one encountered during the fold should be reported.
    let chain = build_valid_chain(&[r#"{"a":1}"#, r#"{"a":2}"#, r#"{"a":3}"#, r#"{"a":4}"#]);
    let mut tampered = chain;
    tampered[1] = ChainEntry::new(
        tampered[1].sequence(),
        tampered[1].previous_hash().clone(),
        event(r#"{"a":999}"#),
        tampered[1].hash().clone(),
    );
    tampered[3] = ChainEntry::new(
        tampered[3].sequence(),
        tampered[3].previous_hash().clone(),
        event(r#"{"a":888}"#),
        tampered[3].hash().clone(),
    );

    let result = verify_chain(tampered);
    assert!(matches!(
        result,
        Err(ChainBreak::HashMismatch { sequence, .. }) if sequence == ChainSequence::try_new(2).expect("valid")
    ));
}
