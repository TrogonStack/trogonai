use std::error::Error;

use super::*;
use crate::fixtures::{CHAIN, OTHER_CHAIN, chain_id, signing_key, writer_id};

#[test]
fn faults_name_what_was_expected_and_never_bytes() {
    let key_id = KeyId::from_verifying_key(signing_key(1).verifying_key());
    assert_eq!(
        CheckpointVerificationError::UntrustedKey {
            key_id: Box::new(key_id)
        }
        .to_string(),
        format!("checkpoint is signed by {key_id}, which is not trusted")
    );
    assert_eq!(
        CheckpointVerificationError::WrongWriter { expected: writer_id() }.to_string(),
        "checkpoint is not for writer gateway-1"
    );
    assert_eq!(
        CheckpointVerificationError::WrongChain {
            expected: chain_id(CHAIN),
            observed: chain_id(OTHER_CHAIN),
        }
        .to_string(),
        format!("checkpoint is for chain {OTHER_CHAIN}, not chain {CHAIN}")
    );
    assert_eq!(
        CheckpointVerificationError::HeadSequenceMismatch {
            expected: 4,
            observed: 3,
        }
        .to_string(),
        "checkpoint names sequence 3, not the verified head's sequence 4"
    );
    assert_eq!(
        CheckpointVerificationError::HeadHashMismatch { sequence: 4 }.to_string(),
        "checkpoint names a head hash other than the verified head's at sequence 4"
    );
    assert_eq!(
        CheckpointVerificationError::StreamSequenceMismatch {
            expected: 14,
            observed: 13,
        }
        .to_string(),
        "checkpoint names stream sequence 13, not the verified head's stream sequence 14"
    );
}

#[test]
fn a_malformed_key_id_keeps_its_cause() {
    let error = CheckpointVerificationError::MalformedKeyId(KeyIdError::NotBase58);
    assert_eq!(error.to_string(), "checkpoint JWS key id is not valid");
    assert_eq!(error.source().unwrap().to_string(), "key id is not base58btc");
}
