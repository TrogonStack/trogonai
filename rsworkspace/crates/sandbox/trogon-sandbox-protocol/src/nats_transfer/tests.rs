use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn descriptor(inbox: &str, length: usize, chunk_size: usize) -> Vec<u8> {
    serde_json::json!({ "inbox": inbox, "length": length, "chunk_size": chunk_size })
        .to_string()
        .into_bytes()
}

#[test]
fn a_well_formed_descriptor_parses() -> TestResult {
    let transfer = Transfer::parse(&descriptor("_INBOX.abc", 10, 4))?;
    assert_eq!(
        (transfer.inbox.as_str(), transfer.length, transfer.chunk_size),
        ("_INBOX.abc", 10, 4)
    );
    Ok(())
}

#[test]
fn out_of_range_lengths_are_refused() {
    for length in [0, MAX_TRANSFER_BYTES + 1] {
        assert!(matches!(
            Transfer::parse(&descriptor("_INBOX.abc", length, 4)),
            Err(NatsTransferError::InvalidLength)
        ));
    }
}

#[test]
fn out_of_range_chunk_sizes_are_refused() {
    for chunk_size in [0, MAX_CHUNK_BYTES + 1] {
        assert!(matches!(
            Transfer::parse(&descriptor("_INBOX.abc", 10, chunk_size)),
            Err(NatsTransferError::InvalidChunkSize)
        ));
    }
}

#[test]
fn only_plain_inbox_subjects_are_accepted() {
    for inbox in ["other.abc", "_INBOX.*", "_INBOX.>", "_INBOX.a b", "_INBOX.a\r\nPUB x"] {
        assert!(
            matches!(
                Transfer::parse(&descriptor(inbox, 10, 4)),
                Err(NatsTransferError::InvalidInbox)
            ),
            "{inbox:?}"
        );
    }
}

#[test]
fn garbage_is_a_descriptor_error() {
    assert!(matches!(
        Transfer::parse(b"not json"),
        Err(NatsTransferError::Descriptor(_))
    ));
}
