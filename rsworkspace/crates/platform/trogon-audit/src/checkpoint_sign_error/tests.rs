use super::*;

#[test]
fn each_failure_has_a_stable_code() {
    assert_eq!(CheckpointSignError::Timeout.as_str(), "timeout");
    assert_eq!(
        CheckpointSignError::Timeout.to_string(),
        "checkpoint signer did not finish in time"
    );
    assert_eq!(CheckpointSignError::Unavailable.as_str(), "unavailable");
    assert_eq!(CheckpointSignError::Refused.as_str(), "refused");
    assert_eq!(CheckpointSignError::InvalidSignature.as_str(), "invalid_signature");
    assert_eq!(
        CheckpointSignError::InvalidSignature.to_string(),
        "checkpoint signature does not verify under the key id it came with"
    );
}
