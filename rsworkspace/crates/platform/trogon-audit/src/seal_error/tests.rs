use super::*;

#[test]
fn the_error_messages_state_the_rule() {
    assert_eq!(
        SealError::TooShort(3).to_string(),
        "sealed record must be at least 36 bytes, got 3"
    );
    assert_eq!(
        SealError::MissingTrailer.to_string(),
        "sealed record does not end in a record hash trailer"
    );
    assert_eq!(
        SealError::HashMismatch.to_string(),
        "record hash does not match the record's bytes"
    );
}
