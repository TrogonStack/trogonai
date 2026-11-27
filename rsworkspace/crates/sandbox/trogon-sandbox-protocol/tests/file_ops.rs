use trogon_sandbox_contract::{
    Base64Data, FileContentHashing, FileOp, FileOpErrorKind, FileOpResponse, FilePresence, FileReadEnd, FileRecursion,
    FileResultTruncation,
};
use trogon_sandbox_protocol::{
    FileOpClass, encode_file_op_response_within, file_op_token, unsupported_file_op_response,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn parse(json: &str) -> Result<FileOp, serde_json::Error> {
    serde_json::from_str(json)
}

fn every_op() -> Result<Vec<(FileOp, &'static str, FileOpClass)>, serde_json::Error> {
    Ok(vec![
        (parse(r#"{"op":"list","path":"/"}"#)?, "list", FileOpClass::Plain),
        (parse(r#"{"op":"stat","path":"/"}"#)?, "stat", FileOpClass::Plain),
        (parse(r#"{"op":"read","path":"/"}"#)?, "read", FileOpClass::Plain),
        (
            parse(r#"{"op":"write","path":"/","data":""}"#)?,
            "write",
            FileOpClass::Plain,
        ),
        (
            parse(r#"{"op":"edit","path":"/","find":"a","replace":"b"}"#)?,
            "edit",
            FileOpClass::Plain,
        ),
        (
            parse(r#"{"op":"search","path":"/","pattern":"x"}"#)?,
            "search",
            FileOpClass::Plain,
        ),
        (parse(r#"{"op":"remove","path":"/"}"#)?, "remove", FileOpClass::Plain),
        (parse(r#"{"op":"mkdir","path":"/"}"#)?, "mkdir", FileOpClass::Plain),
        (
            parse(r#"{"op":"rename","from":"/a","to":"/b"}"#)?,
            "rename",
            FileOpClass::Plain,
        ),
        (
            parse(r#"{"op":"archive_get","path":"/"}"#)?,
            "archive_get",
            FileOpClass::Plain,
        ),
        (
            parse(r#"{"op":"archive_put","path":"/","data":""}"#)?,
            "archive_put",
            FileOpClass::Plain,
        ),
        (
            parse(r#"{"op":"checkpoint","path":"/"}"#)?,
            "checkpoint",
            FileOpClass::ChangeTracking,
        ),
        (parse(r#"{"op":"changes"}"#)?, "changes", FileOpClass::ChangeTracking),
        (
            parse(r#"{"op":"watch_start","path":"/"}"#)?,
            "watch_start",
            FileOpClass::FileWatch,
        ),
        (
            parse(r#"{"op":"watch_poll","watch_id":"w"}"#)?,
            "watch_poll",
            FileOpClass::FileWatch,
        ),
        (
            parse(r#"{"op":"watch_stop","watch_id":"w"}"#)?,
            "watch_stop",
            FileOpClass::FileWatch,
        ),
        (
            parse(r#"{"op":"checksum","path":"/"}"#)?,
            "checksum",
            FileOpClass::DirTransfer,
        ),
    ])
}

#[test]
fn every_operation_has_a_token_and_a_class() -> TestResult {
    for (op, token, class) in every_op()? {
        assert_eq!(file_op_token(&op), token);
        assert_eq!(FileOpClass::of(&op), class);
    }
    Ok(())
}

#[test]
fn tracking_ops_apply_their_defaults() -> TestResult {
    assert_eq!(
        parse(r#"{"op":"changes"}"#)?,
        FileOp::Changes {
            since: None,
            path: None
        }
    );
    assert_eq!(
        parse(r#"{"op":"watch_poll","watch_id":"w"}"#)?,
        FileOp::WatchPoll {
            watch_id: "w".into(),
            after_seq: 0,
            wait_ms: None
        }
    );
    assert!(matches!(
        parse(r#"{"op":"watch_start","path":"/w"}"#)?,
        FileOp::WatchStart {
            recursive: FileRecursion::Flat,
            ..
        }
    ));
    assert!(matches!(
        parse(r#"{"op":"checkpoint","path":"/w"}"#)?,
        FileOp::Checkpoint {
            hash: FileContentHashing::Metadata,
            ..
        }
    ));
    Ok(())
}

#[test]
fn unsupported_names_provider_and_operation() -> TestResult {
    let op = FileOp::Stat { path: "x".into() };
    let FileOpResponse::Unsupported { message } = unsupported_file_op_response("v8_wasm", &op) else {
        return Err("expected an unsupported reply".into());
    };
    assert!(message.contains("stat") && message.contains("v8_wasm"));
    Ok(())
}

#[test]
fn oversized_reply_becomes_a_typed_failure() -> TestResult {
    let reply = FileOpResponse::Content {
        data: Base64Data::new("AAAA".repeat(2048))?,
        offset: 0,
        total_size: 6144,
        eof: FileReadEnd::End,
        truncated: FileResultTruncation::Complete,
    };
    let body = encode_file_op_response_within(&reply, 1024)?;
    assert!(matches!(
        serde_json::from_slice::<FileOpResponse>(&body)?,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::TooLarge,
            ..
        }
    ));
    Ok(())
}

#[test]
fn a_reply_within_the_limit_is_encoded_unchanged() -> TestResult {
    let small = FileOpResponse::Renamed;
    let body = encode_file_op_response_within(&small, 1024)?;
    assert_eq!(serde_json::from_slice::<FileOpResponse>(&body)?, small);
    let exact = encode_file_op_response_within(&small, body.len())?;
    assert_eq!(exact, body);
    let removed = FileOpResponse::Removed {
        existed: FilePresence::Present,
    };
    assert!(!encode_file_op_response_within(&removed, 1024)?.is_empty());
    Ok(())
}
