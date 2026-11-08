use std::error::Error;

use super::*;

#[test]
fn faults_name_positions_and_never_bytes() {
    assert_eq!(
        ChainVerificationError::EmptyRange.to_string(),
        "chain range holds no record"
    );
    assert_eq!(
        ChainVerificationError::RangeTooLong.to_string(),
        "chain range holds more than 10000 records"
    );
    assert_eq!(
        ChainVerificationError::SequenceGap {
            stream_sequence: 9,
            expected: 2,
            observed: 3,
        }
        .to_string(),
        "expected sequence 2, but record at stream sequence 9 is sequence 3"
    );
    assert_eq!(
        ChainVerificationError::MissingGenesis {
            stream_sequence: 9,
            observed: 2,
        }
        .to_string(),
        "chain must open at sequence 1, but record at stream sequence 9 is sequence 2"
    );
    assert_eq!(
        ChainVerificationError::HeaderMismatch {
            stream_sequence: 9,
            header: HeaderName::from_static("Trogon-Audit-Seq"),
        }
        .to_string(),
        "record at stream sequence 9 has a Trogon-Audit-Seq header that disagrees with its payload"
    );
}

#[test]
fn a_seal_fault_keeps_its_source() {
    let error = ChainVerificationError::Unsealed {
        stream_sequence: 9,
        source: SealError::HashMismatch,
    };

    assert_eq!(error.to_string(), "record at stream sequence 9 is not sealed");
    assert_eq!(
        error.source().map(ToString::to_string),
        Some(SealError::HashMismatch.to_string())
    );
}
