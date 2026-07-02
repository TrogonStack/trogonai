use super::*;

#[test]
fn try_new_rejects_zero() {
    assert_eq!(ChainSequence::try_new(0), Err(InvalidChainSequence));
}

#[test]
fn try_new_accepts_one() {
    let sequence = ChainSequence::try_new(1).expect("1 is valid");
    assert_eq!(sequence, ChainSequence::FIRST);
}

#[test]
fn next_increments() {
    let sequence = ChainSequence::try_new(5).expect("5 is valid");
    assert_eq!(sequence.next().as_u64(), 6);
}

#[test]
fn next_saturates_at_u64_max() {
    let sequence = ChainSequence::try_new(u64::MAX).expect("u64::MAX is valid");
    assert_eq!(sequence.next().as_u64(), u64::MAX);
}

#[test]
fn display_matches_value() {
    let sequence = ChainSequence::try_new(42).expect("42 is valid");
    assert_eq!(sequence.to_string(), "42");
}

#[test]
fn ordering_is_numeric() {
    let a = ChainSequence::try_new(1).expect("valid");
    let b = ChainSequence::try_new(2).expect("valid");
    assert!(a < b);
}
