use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::error::Category::Data;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    Base64Data, FileCaseSensitivity, FileContentHashing, FileKind, FileOp, FileOpErrorKind, FileOpRequest,
    FileOpResponse, FileOverwrite, FileParentCreation, FilePresence, FileReadEnd, FileRecursion, FileResultTruncation,
    FileSearchSyntax, FileSha256, FileSha256Error, FileStat, FileWatchEvent, FileWatchEventKind, FileWatchOverflow,
    FileWatchOverflowEvidence, FileWatchTarget, FileWriteTruncation, LeaseError, LeaseId, SandboxUserName, SearchMatch,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn roundtrip<T>(value: &T, expected: Value) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value)?, expected);
    assert_eq!(&serde_json::from_value::<T>(expected)?, value);
    Ok(())
}

fn flag_roundtrip<T>(off: T, on: T) -> TestResult
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug + Default + Copy + Into<bool>,
{
    assert_eq!(T::default(), off);
    assert!(!off.into());
    assert!(on.into());
    roundtrip(&off, json!(false))?;
    roundtrip(&on, json!(true))?;
    assert_eq!(
        serde_json::from_value::<T>(json!("yes")).err().map(|e| e.classify()),
        Some(Data)
    );
    Ok(())
}

#[test]
fn flag_enums_serialize_as_booleans_and_default_to_the_private_defaults() -> TestResult {
    flag_roundtrip(FileRecursion::Flat, FileRecursion::Recursive)?;
    flag_roundtrip(FileWriteTruncation::Keep, FileWriteTruncation::Truncate)?;
    flag_roundtrip(FileSearchSyntax::Literal, FileSearchSyntax::Regex)?;
    flag_roundtrip(FileCaseSensitivity::Sensitive, FileCaseSensitivity::Insensitive)?;
    flag_roundtrip(FileContentHashing::Metadata, FileContentHashing::Content)?;
    flag_roundtrip(FileOverwrite::Refuse, FileOverwrite::Overwrite)?;
    flag_roundtrip(FileResultTruncation::Complete, FileResultTruncation::Truncated)?;
    flag_roundtrip(FileReadEnd::More, FileReadEnd::End)?;
    flag_roundtrip(FilePresence::Absent, FilePresence::Present)?;
    flag_roundtrip(FileWatchOverflow::Intact, FileWatchOverflow::Overflowed)?;
    flag_roundtrip(FileWatchTarget::File, FileWatchTarget::Directory)?;
    Ok(())
}

#[test]
fn parent_creation_defaults_to_create() -> TestResult {
    assert_eq!(FileParentCreation::default(), FileParentCreation::Create);
    roundtrip(&FileParentCreation::Create, json!(true))?;
    roundtrip(&FileParentCreation::Require, json!(false))?;
    assert!(FileParentCreation::Create.creates());
    assert!(!FileParentCreation::Require.creates());
    Ok(())
}

#[test]
fn flag_predicates_report_their_variant() {
    assert!(FileRecursion::Recursive.is_recursive());
    assert!(FileWriteTruncation::Truncate.truncates());
    assert!(FileSearchSyntax::Regex.is_regex());
    assert!(FileCaseSensitivity::Insensitive.ignores_case());
    assert!(FileContentHashing::Content.hashes_content());
    assert!(FileOverwrite::Overwrite.overwrites());
    assert!(FileResultTruncation::Truncated.is_truncated());
    assert!(FileReadEnd::End.reached_end());
    assert!(FilePresence::Present.was_present());
    assert!(FileWatchOverflow::Overflowed.overflowed());
    assert!(FileWatchTarget::Directory.is_directory());
}

#[test]
fn file_sha256_requires_sixty_four_lowercase_hex_characters() -> TestResult {
    let sha = FileSha256::new(SHA)?;
    assert_eq!(sha.as_str(), SHA);
    assert_eq!(sha.to_string(), SHA);
    assert_eq!(SHA.parse::<FileSha256>()?, sha);
    assert_eq!(FileSha256::try_from(SHA.to_string())?, sha);
    assert_eq!(String::from(sha.clone()), SHA);
    assert_eq!(FileSha256::HEX_LEN, 64);
    roundtrip(&sha, json!(SHA))?;

    let short = FileSha256::new("abc").err().ok_or("expected error")?;
    assert_eq!(short, FileSha256Error::Length { expected: 64 });
    assert_eq!(short.to_string(), "file checksum must be exactly 64 characters");
    let upper = SHA.to_uppercase();
    let bad = FileSha256::new(upper.as_str()).err().ok_or("expected error")?;
    assert_eq!(bad, FileSha256Error::NotLowercaseHex);
    assert_eq!(bad.to_string(), "file checksum must be lowercase hexadecimal");
    let non_hex = "g".repeat(64);
    assert_eq!(
        FileSha256::new(non_hex.as_str()).err(),
        Some(FileSha256Error::NotLowercaseHex)
    );
    let decoded = serde_json::from_value::<FileSha256>(json!("abc"))
        .err()
        .ok_or("expected error")?;
    assert_eq!(decoded.classify(), Data);
    assert_eq!(decoded.to_string(), "file checksum must be exactly 64 characters");
    Ok(())
}

#[test]
fn simple_enums_use_snake_case_names() -> TestResult {
    roundtrip(&FileKind::File, json!("file"))?;
    roundtrip(&FileKind::Directory, json!("directory"))?;
    roundtrip(&FileKind::Symlink, json!("symlink"))?;
    roundtrip(&FileKind::Other, json!("other"))?;
    roundtrip(&FileWatchEventKind::Created, json!("created"))?;
    roundtrip(&FileWatchEventKind::Modified, json!("modified"))?;
    roundtrip(&FileWatchEventKind::Deleted, json!("deleted"))?;
    let kinds = [
        (FileOpErrorKind::InvalidPath, "invalid_path"),
        (FileOpErrorKind::PermissionDenied, "permission_denied"),
        (FileOpErrorKind::AlreadyExists, "already_exists"),
        (FileOpErrorKind::NotADirectory, "not_a_directory"),
        (FileOpErrorKind::IsADirectory, "is_a_directory"),
        (FileOpErrorKind::DirectoryNotEmpty, "directory_not_empty"),
        (FileOpErrorKind::TooLarge, "too_large"),
        (FileOpErrorKind::InvalidRequest, "invalid_request"),
        (FileOpErrorKind::Io, "io"),
        (FileOpErrorKind::UnknownCheckpoint, "unknown_checkpoint"),
        (FileOpErrorKind::UnknownWatch, "unknown_watch"),
    ];
    for (kind, name) in kinds {
        roundtrip(&kind, json!(name))?;
    }
    Ok(())
}

#[test]
fn stats_matches_events_and_evidence_round_trip() -> TestResult {
    roundtrip(
        &FileStat {
            path: "a".to_string(),
            name: "a".to_string(),
            kind: FileKind::File,
            size: 3,
            mode: None,
            modified_unix_ms: None,
            symlink_target: None,
        },
        json!({"path": "a", "name": "a", "kind": "file", "size": 3}),
    )?;
    roundtrip(
        &FileStat {
            path: "l".to_string(),
            name: "l".to_string(),
            kind: FileKind::Symlink,
            size: 0,
            mode: Some(0o777),
            modified_unix_ms: Some(-5),
            symlink_target: Some("a".to_string()),
        },
        json!({
            "path": "l", "name": "l", "kind": "symlink", "size": 0, "mode": 511,
            "modified_unix_ms": -5, "symlink_target": "a"
        }),
    )?;
    roundtrip(
        &SearchMatch {
            path: "a".to_string(),
            line: None,
            text: None,
        },
        json!({"path": "a"}),
    )?;
    roundtrip(
        &SearchMatch {
            path: "a".to_string(),
            line: Some(4),
            text: Some("hit".to_string()),
        },
        json!({"path": "a", "line": 4, "text": "hit"}),
    )?;
    roundtrip(
        &FileWatchEvent {
            seq: 2,
            kind: FileWatchEventKind::Created,
            path: "d".to_string(),
            is_dir: FileWatchTarget::Directory,
        },
        json!({"seq": 2, "kind": "created", "path": "d", "is_dir": true}),
    )?;
    roundtrip(
        &FileWatchOverflowEvidence {
            watch_id: "w1".to_string(),
            dropped_through: 9,
        },
        json!({"watch_id": "w1", "dropped_through": 9}),
    )?;
    Ok(())
}

#[test]
fn write_round_trips_data_as_base64_and_applies_defaults() -> TestResult {
    let op = FileOp::Write {
        path: "a".to_string(),
        data: Base64Data::new("AAEC/w==")?,
        offset: 0,
        truncate: FileWriteTruncation::Keep,
        mode: None,
        create_parents: FileParentCreation::Create,
    };
    roundtrip(
        &op,
        json!({
            "op": "write", "path": "a", "data": "AAEC/w==", "offset": 0,
            "truncate": false, "create_parents": true
        }),
    )?;
    assert_eq!(
        serde_json::from_value::<FileOp>(json!({"op": "write", "path": "a", "data": "AAEC/w=="}))?,
        op
    );
    roundtrip(
        &FileOp::Write {
            path: "a".to_string(),
            data: Base64Data::new("")?,
            offset: 7,
            truncate: FileWriteTruncation::Truncate,
            mode: Some(0o600),
            create_parents: FileParentCreation::Require,
        },
        json!({
            "op": "write", "path": "a", "data": "", "offset": 7,
            "truncate": true, "mode": 384, "create_parents": false
        }),
    )?;
    Ok(())
}

#[test]
fn every_op_variant_round_trips_and_applies_defaults() -> TestResult {
    roundtrip(
        &FileOp::List {
            path: "d".to_string(),
            recursive: FileRecursion::Flat,
            max_entries: None,
        },
        json!({"op": "list", "path": "d", "recursive": false}),
    )?;
    roundtrip(
        &FileOp::List {
            path: "d".to_string(),
            recursive: FileRecursion::Recursive,
            max_entries: Some(5),
        },
        json!({"op": "list", "path": "d", "recursive": true, "max_entries": 5}),
    )?;
    roundtrip(
        &FileOp::Stat { path: "d".to_string() },
        json!({"op": "stat", "path": "d"}),
    )?;
    roundtrip(
        &FileOp::Read {
            path: "f".to_string(),
            offset: 0,
            length: None,
            max_bytes: None,
        },
        json!({"op": "read", "path": "f", "offset": 0}),
    )?;
    roundtrip(
        &FileOp::Read {
            path: "f".to_string(),
            offset: 1,
            length: Some(2),
            max_bytes: Some(3),
        },
        json!({"op": "read", "path": "f", "offset": 1, "length": 2, "max_bytes": 3}),
    )?;
    let edit = FileOp::Edit {
        path: "f".to_string(),
        find: "a".to_string(),
        replace: "b".to_string(),
        expected_occurrences: 1,
    };
    roundtrip(
        &edit,
        json!({"op": "edit", "path": "f", "find": "a", "replace": "b", "expected_occurrences": 1}),
    )?;
    assert_eq!(
        serde_json::from_value::<FileOp>(json!({"op": "edit", "path": "f", "find": "a", "replace": "b"}))?,
        edit
    );
    roundtrip(
        &FileOp::Search {
            path: "d".to_string(),
            glob: None,
            pattern: None,
            regex: FileSearchSyntax::Literal,
            case_insensitive: FileCaseSensitivity::Sensitive,
            max_results: None,
            max_file_bytes: None,
        },
        json!({"op": "search", "path": "d", "regex": false, "case_insensitive": false}),
    )?;
    roundtrip(
        &FileOp::Search {
            path: "d".to_string(),
            glob: Some("**/*.rs".to_string()),
            pattern: Some("fn".to_string()),
            regex: FileSearchSyntax::Regex,
            case_insensitive: FileCaseSensitivity::Insensitive,
            max_results: Some(10),
            max_file_bytes: Some(100),
        },
        json!({
            "op": "search", "path": "d", "glob": "**/*.rs", "pattern": "fn", "regex": true,
            "case_insensitive": true, "max_results": 10, "max_file_bytes": 100
        }),
    )?;
    roundtrip(
        &FileOp::Remove {
            path: "d".to_string(),
            recursive: FileRecursion::Flat,
        },
        json!({"op": "remove", "path": "d", "recursive": false}),
    )?;
    let mkdir = FileOp::Mkdir {
        path: "d".to_string(),
        parents: FileParentCreation::Create,
    };
    roundtrip(&mkdir, json!({"op": "mkdir", "path": "d", "parents": true}))?;
    assert_eq!(
        serde_json::from_value::<FileOp>(json!({"op": "mkdir", "path": "d"}))?,
        mkdir
    );
    let rename = FileOp::Rename {
        from: "a".to_string(),
        to: "b".to_string(),
        overwrite: FileOverwrite::Refuse,
    };
    roundtrip(
        &rename,
        json!({"op": "rename", "from": "a", "to": "b", "overwrite": false}),
    )?;
    assert_eq!(
        serde_json::from_value::<FileOp>(json!({"op": "rename", "from": "a", "to": "b"}))?,
        rename
    );
    roundtrip(
        &FileOp::ArchiveGet {
            path: "d".to_string(),
            max_bytes: None,
        },
        json!({"op": "archive_get", "path": "d"}),
    )?;
    roundtrip(
        &FileOp::ArchiveGet {
            path: "d".to_string(),
            max_bytes: Some(9),
        },
        json!({"op": "archive_get", "path": "d", "max_bytes": 9}),
    )?;
    roundtrip(
        &FileOp::ArchivePut {
            path: "d".to_string(),
            data: Base64Data::new("Zm8=")?,
        },
        json!({"op": "archive_put", "path": "d", "data": "Zm8="}),
    )?;
    roundtrip(
        &FileOp::Checkpoint {
            path: "d".to_string(),
            hash: FileContentHashing::Content,
        },
        json!({"op": "checkpoint", "path": "d", "hash": true}),
    )?;
    let changes = FileOp::Changes {
        since: None,
        path: None,
    };
    roundtrip(&changes, json!({"op": "changes"}))?;
    roundtrip(
        &FileOp::Changes {
            since: Some("cp1".to_string()),
            path: Some("d".to_string()),
        },
        json!({"op": "changes", "since": "cp1", "path": "d"}),
    )?;
    roundtrip(
        &FileOp::WatchStart {
            path: "d".to_string(),
            recursive: FileRecursion::Recursive,
        },
        json!({"op": "watch_start", "path": "d", "recursive": true}),
    )?;
    roundtrip(
        &FileOp::WatchPoll {
            watch_id: "w".to_string(),
            after_seq: 0,
            wait_ms: None,
        },
        json!({"op": "watch_poll", "watch_id": "w", "after_seq": 0}),
    )?;
    roundtrip(
        &FileOp::WatchPoll {
            watch_id: "w".to_string(),
            after_seq: 4,
            wait_ms: Some(50),
        },
        json!({"op": "watch_poll", "watch_id": "w", "after_seq": 4, "wait_ms": 50}),
    )?;
    roundtrip(
        &FileOp::WatchStop {
            watch_id: "w".to_string(),
        },
        json!({"op": "watch_stop", "watch_id": "w"}),
    )?;
    roundtrip(
        &FileOp::Checksum {
            path: "f".to_string(),
            offset: 0,
            length: None,
        },
        json!({"op": "checksum", "path": "f", "offset": 0}),
    )?;
    roundtrip(
        &FileOp::Checksum {
            path: "f".to_string(),
            offset: 2,
            length: Some(3),
        },
        json!({"op": "checksum", "path": "f", "offset": 2, "length": 3}),
    )?;
    Ok(())
}

#[test]
fn invalid_base64_data_rejects_the_whole_op() -> TestResult {
    let error = serde_json::from_value::<FileOp>(json!({"op": "write", "path": "a", "data": "Zg="}))
        .err()
        .ok_or("expected error")?;
    assert_eq!(error.classify(), Data);
    Ok(())
}

#[test]
fn request_round_trips_with_and_without_user() -> TestResult {
    let op = FileOp::Stat { path: "f".to_string() };
    roundtrip(
        &FileOpRequest {
            lease_id: LeaseId::new("lease-1"),
            op: op.clone(),
            as_user: None,
        },
        json!({"lease_id": "lease-1", "op": {"op": "stat", "path": "f"}}),
    )?;
    roundtrip(
        &FileOpRequest {
            lease_id: LeaseId::new("lease-1"),
            op,
            as_user: Some(SandboxUserName::new("dev")?),
        },
        json!({"lease_id": "lease-1", "op": {"op": "stat", "path": "f"}, "as_user": "dev"}),
    )?;
    Ok(())
}

#[test]
fn every_response_variant_round_trips() -> TestResult {
    let stat = FileStat {
        path: "a".to_string(),
        name: "a".to_string(),
        kind: FileKind::File,
        size: 3,
        mode: None,
        modified_unix_ms: None,
        symlink_target: None,
    };
    let stat_json = json!({"path": "a", "name": "a", "kind": "file", "size": 3});
    let cases: Vec<(FileOpResponse, Value)> = vec![
        (
            FileOpResponse::Listing {
                entries: vec![stat.clone()],
                truncated: FileResultTruncation::Complete,
            },
            json!({"outcome": "listing", "entries": [stat_json.clone()], "truncated": false}),
        ),
        (
            FileOpResponse::Stat { stat },
            json!({"outcome": "stat", "stat": stat_json}),
        ),
        (
            FileOpResponse::Content {
                data: Base64Data::new("Zm9v")?,
                offset: 0,
                total_size: 3,
                eof: FileReadEnd::End,
                truncated: FileResultTruncation::Complete,
            },
            json!({
                "outcome": "content", "data": "Zm9v", "offset": 0, "total_size": 3,
                "eof": true, "truncated": false
            }),
        ),
        (
            FileOpResponse::Written { bytes: 1, size: 2 },
            json!({"outcome": "written", "bytes": 1, "size": 2}),
        ),
        (
            FileOpResponse::Edited {
                occurrences: 1,
                size: 2,
            },
            json!({"outcome": "edited", "occurrences": 1, "size": 2}),
        ),
        (
            FileOpResponse::EditRejected {
                occurrences: 2,
                expected: 1,
            },
            json!({"outcome": "edit_rejected", "occurrences": 2, "expected": 1}),
        ),
        (
            FileOpResponse::SearchResults {
                matches: vec![SearchMatch {
                    path: "a".to_string(),
                    line: Some(1),
                    text: None,
                }],
                truncated: FileResultTruncation::Truncated,
                files_scanned: 4,
            },
            json!({
                "outcome": "search_results", "matches": [{"path": "a", "line": 1}],
                "truncated": true, "files_scanned": 4
            }),
        ),
        (
            FileOpResponse::Removed {
                existed: FilePresence::Present,
            },
            json!({"outcome": "removed", "existed": true}),
        ),
        (
            FileOpResponse::Created {
                already_existed: FilePresence::Absent,
            },
            json!({"outcome": "created", "already_existed": false}),
        ),
        (FileOpResponse::Renamed, json!({"outcome": "renamed"})),
        (
            FileOpResponse::Archive {
                data: Base64Data::new("Zg==")?,
                entries: 1,
                bytes: 1,
            },
            json!({"outcome": "archive", "data": "Zg==", "entries": 1, "bytes": 1}),
        ),
        (
            FileOpResponse::Extracted { entries: 2, bytes: 3 },
            json!({"outcome": "extracted", "entries": 2, "bytes": 3}),
        ),
        (
            FileOpResponse::Checkpointed {
                checkpoint_id: "cp1".to_string(),
                entries: 5,
            },
            json!({"outcome": "checkpointed", "checkpoint_id": "cp1", "entries": 5}),
        ),
        (
            FileOpResponse::Changes {
                since: FileOpResponse::CHANGES_SINCE_START.to_string(),
                added: vec!["a".to_string()],
                modified: vec![],
                deleted: vec!["b".to_string()],
                truncated: FileResultTruncation::Complete,
            },
            json!({
                "outcome": "changes", "since": "start", "added": ["a"], "modified": [],
                "deleted": ["b"], "truncated": false
            }),
        ),
        (
            FileOpResponse::WatchStarted {
                watch_id: "w".to_string(),
            },
            json!({"outcome": "watch_started", "watch_id": "w"}),
        ),
        (
            FileOpResponse::WatchEvents {
                events: vec![FileWatchEvent {
                    seq: 1,
                    kind: FileWatchEventKind::Deleted,
                    path: "a".to_string(),
                    is_dir: FileWatchTarget::File,
                }],
                next_seq: 1,
                overflowed: FileWatchOverflow::Overflowed,
            },
            json!({
                "outcome": "watch_events",
                "events": [{"seq": 1, "kind": "deleted", "path": "a", "is_dir": false}],
                "next_seq": 1, "overflowed": true
            }),
        ),
        (
            FileOpResponse::WatchStopped {
                existed: FilePresence::Present,
            },
            json!({"outcome": "watch_stopped", "existed": true}),
        ),
        (
            FileOpResponse::Checksum {
                sha256: FileSha256::new(SHA)?,
                bytes: 0,
                total_size: 0,
            },
            json!({"outcome": "checksum", "sha256": SHA, "bytes": 0, "total_size": 0}),
        ),
        (
            FileOpResponse::NotFound { path: "a".to_string() },
            json!({"outcome": "not_found", "path": "a"}),
        ),
        (
            FileOpResponse::Failed {
                kind: FileOpErrorKind::TooLarge,
                message: "big".to_string(),
            },
            json!({"outcome": "failed", "kind": "too_large", "message": "big"}),
        ),
        (
            FileOpResponse::Unsupported {
                message: "no".to_string(),
            },
            json!({"outcome": "unsupported", "message": "no"}),
        ),
        (
            FileOpResponse::Denied(LeaseError::EntityNotEnabled),
            json!({"outcome": "denied", "kind": "entity_not_enabled"}),
        ),
    ];
    for (response, expected) in cases {
        roundtrip(&response, expected)?;
    }
    Ok(())
}

#[test]
fn limits_match_the_private_wire_contract() {
    assert_eq!(FileOp::MAX_READ_BYTES, 16 * 1024 * 1024);
    assert_eq!(FileOp::MAX_WRITE_BYTES, 16 * 1024 * 1024);
    assert_eq!(FileOp::MAX_ARCHIVE_BYTES, 64 * 1024 * 1024);
    assert_eq!(FileOp::DEFAULT_MAX_ENTRIES, 1000);
    assert_eq!(FileOp::MAX_ENTRIES, 20_000);
    assert_eq!(FileOp::DEFAULT_SEARCH_FILE_BYTES, 2 * 1024 * 1024);
    assert_eq!(FileOp::WATCH_MAX_WAIT_MS, 20_000);
    assert_eq!(FileOp::WATCH_MAX_EVENTS_PER_POLL, 1000);
    assert_eq!(FileOp::TRACKING_MAX_ENTRIES, 100_000);
    assert_eq!(FileOpResponse::CHANGES_SINCE_START, "start");
}
