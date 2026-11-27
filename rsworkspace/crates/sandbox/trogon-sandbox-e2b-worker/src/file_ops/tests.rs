use std::collections::BTreeSet;
use std::error::Error;
use std::time::Duration;

use serde_json::Value;
use trogon_sandbox_contract::{
    Base64Data, FileContentHashing, FileKind, FileOp, FileOpErrorKind, FileOpResponse, FileRecursion,
    FileResultTruncation,
};
use trogon_sandbox_file_ops::FileOpError;
use url::Url;

use super::archive_upload::ArchiveUpload;
use super::guest_entry::GuestEntry;
use super::path::{failure, validate};
use super::payload::{decode, encode};
use super::script_failure::script_failure;
use super::search::with_truncation;
use super::staging::{check_relative, create_staged, staging_failure};
use super::stderr_kind::stderr_kind;
use super::tar_size::tar_entry_bytes;
use super::write::split_parent;
use crate::envd::{Envd, EnvdTarget, ProcessResult};
use crate::{E2bConfig, file_ops};

type TestResult = Result<(), Box<dyn Error>>;

fn entry(overrides: &[(&str, Value)]) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::from_str::<Value>(r#"{"name": "a", "path": "/w/a", "type": "FILE_TYPE_FILE"}"#)?;
    for (key, replacement) in overrides {
        value[*key] = replacement.clone();
    }
    Ok(value)
}

fn output(exit_code: i32, stderr: &str) -> ProcessResult {
    ProcessResult {
        exit_code,
        stderr: stderr.as_bytes().to_vec(),
        ..ProcessResult::default()
    }
}

#[test]
fn entries_parse_sizes_kinds_and_times() -> TestResult {
    let stat = GuestEntry::parse(entry(&[
        ("size", "17".into()),
        ("mode", 384.into()),
        ("modifiedTime", "2026-01-02T03:04:05Z".into()),
    ])?)?;
    assert_eq!((stat.kind, stat.size, stat.mode), (FileKind::File, 17, Some(0o600)));
    assert_eq!(stat.modified_unix_ms, Some(1_767_323_045_000));
    assert_eq!(GuestEntry::parse(entry(&[("size", 5.into())])?)?.size, 5);
    assert_eq!(GuestEntry::parse(entry(&[])?)?.size, 0);
    let directory = GuestEntry::parse(entry(&[("type", "FILE_TYPE_DIRECTORY".into())])?)?;
    assert_eq!(directory.kind, FileKind::Directory);
    let link = GuestEntry::parse(entry(&[("type", "FILE_TYPE_SYMLINK".into())])?)?;
    assert_eq!(link.kind, FileKind::Symlink);
    let flagged = GuestEntry::parse(entry(&[("isSymlink", true.into())])?)?;
    assert_eq!(flagged.kind, FileKind::Symlink);
    let other = GuestEntry::parse(entry(&[("type", "FILE_TYPE_UNSPECIFIED".into())])?)?;
    assert_eq!(other.kind, FileKind::Other);
    Ok(())
}

#[test]
fn malformed_entries_are_refused() -> TestResult {
    for overrides in [
        [("size", "many".into())],
        [("size", (-1).into())],
        [("size", true.into())],
        [("modifiedTime", "yesterday".into())],
        [("name", 3.into())],
    ] {
        let failed = FileOpError::from(GuestEntry::parse(entry(&overrides)?).err().ok_or("accepted")?);
        assert!(matches!(
            failed,
            FileOpError::Failed { kind: FileOpErrorKind::Io, ref message } if message.starts_with("invalid envd metadata")
        ));
    }
    Ok(())
}

#[test]
fn stderr_names_the_failure_kind() {
    for (text, kind) in [
        (
            "rm: cannot remove 'x': Permission denied",
            FileOpErrorKind::PermissionDenied,
        ),
        ("touch: Read-only file system", FileOpErrorKind::PermissionDenied),
        ("cd: Not a directory", FileOpErrorKind::NotADirectory),
        ("cat: Is a directory", FileOpErrorKind::IsADirectory),
        ("rmdir: Directory not empty", FileOpErrorKind::DirectoryNotEmpty),
        ("mkdir: File exists", FileOpErrorKind::AlreadyExists),
        ("something else", FileOpErrorKind::Io),
    ] {
        assert_eq!(stderr_kind(text), kind, "{text}");
    }
}

#[test]
fn script_failures_carry_the_exit_and_the_stderr() {
    let message = |failure: FileOpError| match failure {
        FileOpError::Failed { kind, message } => (kind, message),
        other => (FileOpErrorKind::Io, other.to_string()),
    };
    let (kind, text) = message(script_failure("file write", &output(1, "dd: Permission denied")));
    assert_eq!(kind, FileOpErrorKind::PermissionDenied);
    assert_eq!(text, "file write failed (exit 1): dd: Permission denied");
    let (kind, _) = message(script_failure("file write", &output(21, "")));
    assert_eq!(kind, FileOpErrorKind::IsADirectory);
}

#[test]
fn path_scripts_map_exit_codes_to_typed_failures() {
    let path = "/w/a";
    assert_eq!(
        failure(&output(44, ""), path),
        FileOpError::NotFound { path: path.into() }
    );
    assert_eq!(
        failure(&output(1, "rm: cannot remove: No such file or directory"), path),
        FileOpError::NotFound { path: path.into() }
    );
    assert!(matches!(
        failure(&output(64, ""), path),
        FileOpError::Failed {
            kind: FileOpErrorKind::InvalidPath,
            ..
        }
    ));
    assert!(matches!(
        failure(&output(17, ""), path),
        FileOpError::Failed {
            kind: FileOpErrorKind::AlreadyExists,
            ..
        }
    ));
    assert!(matches!(
        failure(&output(127, ""), path),
        FileOpError::Unsupported { .. }
    ));
    assert!(matches!(
        failure(&output(1, "mv: boom"), path),
        FileOpError::Failed {
            kind: FileOpErrorKind::Io,
            ..
        }
    ));
}

#[test]
fn destructive_paths_cannot_reach_the_root() {
    assert!(validate("", false).is_err());
    assert!(validate("a\0b", true).is_err());
    assert!(validate("/", true).is_err());
    assert!(validate("/a/..", true).is_err());
    assert!(validate("/./", true).is_err());
    assert!(validate("/", false).is_ok());
    assert!(validate("/a/./b/..", true).is_ok());
    assert!(validate("relative", true).is_ok());
}

#[test]
fn relative_paths_stay_plain_and_bounded() {
    assert!(check_relative("a/b.txt").is_ok());
    for path in [
        "",
        "/abs",
        "../up",
        "a/../b",
        "a\0b",
        &"a/".repeat(300),
        &"x".repeat(5000),
    ] {
        assert!(check_relative(path).is_err(), "{path}");
    }
}

#[test]
fn tar_entries_are_sized_in_blocks() {
    assert_eq!(tar_entry_bytes("a", 0), 512);
    assert_eq!(tar_entry_bytes("a", 1), 1024);
    assert_eq!(tar_entry_bytes("a", 513), 512 + 1024);
    assert_eq!(tar_entry_bytes(&"n".repeat(150), 0), 512 + 512 + 512);
}

#[test]
fn parents_split_at_the_last_separator() {
    assert_eq!(split_parent("/w/a"), ("/w", "a"));
    assert_eq!(split_parent("/a"), ("/", "a"));
    assert_eq!(split_parent("a"), (".", "a"));
}

#[test]
fn payloads_roundtrip() -> TestResult {
    let data = encode(&[0, 1, 2, 255])?;
    assert_eq!(decode(&data), [0, 1, 2, 255]);
    assert_eq!(decode(&Base64Data::new("")?), Vec::<u8>::new());
    Ok(())
}

#[test]
fn uploaded_archives_are_rewritten_canonically() -> TestResult {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, content, directory) in [("dir", &b""[..], true), ("dir/a.txt", &b"hi"[..], false)] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o4755);
        header.set_uid(1000);
        header.set_entry_type(if directory {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        header.set_cksum();
        builder.append_data(&mut header, name, content)?;
    }
    let upload = ArchiveUpload::canonical(&builder.into_inner()?)?;
    assert_eq!((upload.entries, upload.bytes), (2, 2));
    assert_eq!(
        upload.paths,
        BTreeSet::from(["dir".to_string(), "dir/a.txt".to_string()])
    );
    let mut archive = tar::Archive::new(upload.data.as_slice());
    for entry in archive.entries()? {
        let entry = entry?;
        assert_eq!((entry.header().uid()?, entry.header().mode()?), (0, 0o755));
    }
    assert!(ArchiveUpload::canonical(b"not a tar").is_err());
    Ok(())
}

#[test]
fn host_staging_failures_name_the_io_kind() {
    let failed = staging_failure(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert_eq!(
        failed,
        FileOpError::failed(FileOpErrorKind::Io, "file operation staging failed: permission denied")
    );
}

#[test]
fn staged_files_need_a_creatable_parent() -> TestResult {
    let directory = tempfile::TempDir::new()?;
    create_staged(&directory.path().join("a/b/c.txt"))?;
    assert!(directory.path().join("a/b/c.txt").is_file());
    let blocker = directory.path().join("blocker");
    std::fs::write(&blocker, b"x")?;
    assert!(create_staged(&blocker.join("child.txt")).is_err());
    assert!(create_staged(std::path::Path::new("")).is_err());
    Ok(())
}

#[test]
fn collection_truncation_marks_only_search_results() {
    let results = |truncated| FileOpResponse::SearchResults {
        matches: vec![],
        truncated,
        files_scanned: 3,
    };
    assert_eq!(
        with_truncation(results(FileResultTruncation::Complete), true),
        results(FileResultTruncation::Truncated)
    );
    assert_eq!(
        with_truncation(results(FileResultTruncation::Truncated), false),
        results(FileResultTruncation::Truncated)
    );
    assert_eq!(
        with_truncation(results(FileResultTruncation::Complete), false),
        results(FileResultTruncation::Complete)
    );
    assert_eq!(with_truncation(FileOpResponse::Renamed, true), FileOpResponse::Renamed);
}

#[tokio::test]
async fn tracking_and_watch_operations_are_not_served_here() -> TestResult {
    let base = Url::parse("http://127.0.0.1:9")?;
    let config = E2bConfig::new(base.clone(), "e2b.app", "fixture-key")?.with_envd_url(base)?;
    let target = EnvdTarget::for_sandbox(&config, "fixture", None, None)?;
    let envd = Envd::new(Duration::from_secs(1))?;
    for op in [
        FileOp::Checkpoint {
            path: "/w".into(),
            hash: FileContentHashing::default(),
        },
        FileOp::Changes {
            since: None,
            path: None,
        },
        FileOp::WatchStart {
            path: "/w".into(),
            recursive: FileRecursion::Flat,
        },
        FileOp::WatchPoll {
            watch_id: "w".into(),
            after_seq: 0,
            wait_ms: None,
        },
        FileOp::WatchStop { watch_id: "w".into() },
    ] {
        let reply = file_ops::execute(&envd, &target, "user", &op).await;
        assert!(matches!(reply, FileOpResponse::Unsupported { .. }), "{op:?}");
    }
    Ok(())
}
