#![cfg(unix)]

mod file_ops_guest_support;

use file_ops_guest_support::{Guest, TestResult, directory, entry, file, no_content, ok, payload, status, symlink};
use serde_json::json;
use trogon_sandbox_contract::{
    FileOp, FileOpErrorKind, FileOpResponse, FileOverwrite, FileParentCreation, FilePresence, FileRecursion,
    FileWriteTruncation,
};

fn stat(path: &str) -> FileOp {
    FileOp::Stat { path: path.into() }
}

fn list(path: &str) -> FileOp {
    FileOp::List {
        path: path.into(),
        recursive: FileRecursion::Flat,
        max_entries: None,
    }
}

fn write(
    path: &str,
    mode: Option<u32>,
    create_parents: FileParentCreation,
) -> Result<FileOp, Box<dyn std::error::Error>> {
    Ok(FileOp::Write {
        path: path.into(),
        data: payload(b"abc")?,
        offset: 0,
        truncate: FileWriteTruncation::Truncate,
        mode,
        create_parents,
    })
}

fn archive_get(path: &str) -> FileOp {
    FileOp::ArchiveGet {
        path: path.into(),
        max_bytes: None,
    }
}

fn failed(reply: &FileOpResponse, expected: FileOpErrorKind, fragment: &str) -> bool {
    matches!(reply, FileOpResponse::Failed { kind, message } if *kind == expected && message.contains(fragment))
}

#[tokio::test]
async fn envd_failures_map_to_permission_and_io_errors() -> TestResult {
    let guest = Guest::new().await?;
    guest.stat_reply("/w/denied", status(403)).await;
    guest.stat_reply("/w/broken", status(500)).await;
    let reply = guest.run(stat("/w/denied")).await;
    assert!(
        failed(&reply, FileOpErrorKind::PermissionDenied, "scripted"),
        "{reply:?}"
    );
    let reply = guest.run(stat("/w/broken")).await;
    assert!(failed(&reply, FileOpErrorKind::Io, "unavailable"), "{reply:?}");
    let reply = guest.run(stat("/w/absent")).await;
    assert!(
        matches!(reply, FileOpResponse::NotFound { ref path } if path == "/w/absent"),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn stat_refuses_nul_paths_without_asking_envd() -> TestResult {
    let guest = Guest::new().await?;
    let reply = guest.run(stat("/w/a\0b")).await;
    assert!(failed(&reply, FileOpErrorKind::InvalidPath, "NUL"), "{reply:?}");
    assert_eq!(guest.requests().await, 0);
    Ok(())
}

#[tokio::test]
async fn list_refuses_files_and_reads_sparse_or_malformed_directory_replies() -> TestResult {
    let guest = Guest::new().await?;
    guest.stat("/w/file", file("/w/file", 3)).await;
    let reply = guest.run(list("/w/file")).await;
    assert!(failed(&reply, FileOpErrorKind::NotADirectory, "/w/file"), "{reply:?}");

    guest.stat("/w/empty", directory("/w/empty")).await;
    guest.list_dir("/w/empty", json!({})).await;
    let reply = guest.run(list("/w/empty")).await;
    assert!(
        matches!(reply, FileOpResponse::Listing { ref entries, .. } if entries.is_empty()),
        "{reply:?}"
    );

    guest.stat("/w/scalar", directory("/w/scalar")).await;
    guest.list_dir("/w/scalar", json!({"entries": 3})).await;
    let reply = guest.run(list("/w/scalar")).await;
    assert!(
        failed(&reply, FileOpErrorKind::Io, "invalid envd directory entries"),
        "{reply:?}"
    );

    let mut misnamed = file("/w/odd/a", 1);
    misnamed["name"] = "b".into();
    guest.stat("/w/odd", directory("/w/odd")).await;
    guest.list_dir("/w/odd", json!({"entries": [misnamed]})).await;
    let reply = guest.run(list("/w/odd")).await;
    assert!(
        failed(&reply, FileOpErrorKind::Io, "malformed directory entry"),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn write_refuses_malformed_requests_before_asking_envd() -> TestResult {
    let guest = Guest::new().await?;
    let reply = guest.run(write("", None, FileParentCreation::Create)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::InvalidPath, "nonempty path"),
        "{reply:?}"
    );
    let reply = guest.run(write("/w/a\0b", None, FileParentCreation::Create)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::InvalidPath, "nonempty path"),
        "{reply:?}"
    );
    let reply = guest
        .run(write("/w/a", Some(0o10000), FileParentCreation::Create)?)
        .await;
    assert!(
        failed(&reply, FileOpErrorKind::InvalidRequest, "non-permission bits"),
        "{reply:?}"
    );
    let reply = guest.run(write("/w/dir/", None, FileParentCreation::Create)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::IsADirectory, "needs a file path"),
        "{reply:?}"
    );
    assert_eq!(guest.requests().await, 0);
    Ok(())
}

#[tokio::test]
async fn write_checks_the_target_and_its_parent() -> TestResult {
    let guest = Guest::new().await?;
    guest.stat("/w/dir", directory("/w/dir")).await;
    let reply = guest.run(write("/w/dir", None, FileParentCreation::Create)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::IsADirectory, "/w/dir is a directory"),
        "{reply:?}"
    );

    guest.stat_reply("/w/broken", status(500)).await;
    let reply = guest.run(write("/w/broken", None, FileParentCreation::Create)?).await;
    assert!(failed(&reply, FileOpErrorKind::Io, "unavailable"), "{reply:?}");

    guest.stat("/blocked", file("/blocked", 1)).await;
    let reply = guest.run(write("/blocked/a", None, FileParentCreation::Require)?).await;
    assert!(
        failed(&reply, FileOpErrorKind::NotADirectory, "/blocked is not a directory"),
        "{reply:?}"
    );

    guest.stat_reply("/flaky", status(500)).await;
    let reply = guest.run(write("/flaky/a", None, FileParentCreation::Require)?).await;
    assert!(failed(&reply, FileOpErrorKind::Io, "unavailable"), "{reply:?}");

    let reply = guest.run(write("/gone/a", None, FileParentCreation::Require)?).await;
    assert!(
        matches!(reply, FileOpResponse::NotFound { ref path } if path == "/gone/a"),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn write_creates_a_file_under_an_existing_parent() -> TestResult {
    let guest = Guest::new().await?;
    guest.stat_after_creation("/w/new", file("/w/new", 3)).await;
    guest.stat("/w", directory("/w")).await;
    guest.upload(no_content()).await;
    guest.remove(ok()).await;
    guest.script(0, "").await?;
    let reply = guest
        .run(write("/w/new", Some(0o640), FileParentCreation::Require)?)
        .await;
    assert_eq!(reply, FileOpResponse::Written { bytes: 3, size: 3 });
    Ok(())
}

#[tokio::test]
async fn write_reports_staging_and_script_failures() -> TestResult {
    let refused = Guest::new().await?;
    refused.stat("/w/a", file("/w/a", 1)).await;
    refused.upload(status(500)).await;
    refused.remove(ok()).await;
    let reply = refused.run(write("/w/a", None, FileParentCreation::Create)?).await;
    assert!(failed(&reply, FileOpErrorKind::Io, "unavailable"), "{reply:?}");

    let broken = Guest::new().await?;
    broken.stat("/w/a", file("/w/a", 1)).await;
    broken.upload(no_content()).await;
    broken.remove(status(500)).await;
    broken.script(1, "dd: failed to open").await?;
    let reply = broken.run(write("/w/a", None, FileParentCreation::Create)?).await;
    assert!(
        failed(
            &reply,
            FileOpErrorKind::Io,
            "file write failed (exit 1): dd: failed to open"
        ),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn edit_refuses_oversized_files_and_reports_script_failures() -> TestResult {
    let find = |path: &str| FileOp::Edit {
        path: path.into(),
        find: "a".into(),
        replace: "b".into(),
        expected_occurrences: 3,
    };
    let guest = Guest::new().await?;
    guest.stat("/w/big", file("/w/big", 16 * 1024 * 1024 + 1)).await;
    let reply = guest.run(find("/w/big")).await;
    assert!(
        failed(&reply, FileOpErrorKind::TooLarge, "edit byte limit"),
        "{reply:?}"
    );

    guest.stat("/w/a", file("/w/a", 3)).await;
    guest.download("/w/a", b"aaa").await;
    guest.upload(no_content()).await;
    guest.remove(ok()).await;
    guest.script(1, "cp: cannot create").await?;
    let reply = guest.run(find("/w/a")).await;
    assert!(
        failed(&reply, FileOpErrorKind::Io, "file edit failed (exit 1)"),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn path_scripts_report_missing_entries() -> TestResult {
    let guest = Guest::new().await?;
    guest.script(44, "").await?;
    let reply = guest
        .run(FileOp::Remove {
            path: "/w/gone".into(),
            recursive: FileRecursion::Flat,
        })
        .await;
    assert_eq!(
        reply,
        FileOpResponse::Removed {
            existed: FilePresence::Absent
        }
    );
    let reply = guest
        .run(FileOp::Rename {
            from: "/w/a".into(),
            to: "/w/b".into(),
            overwrite: FileOverwrite::Refuse,
        })
        .await;
    assert!(
        matches!(reply, FileOpResponse::NotFound { ref path } if path == "/w/a"),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn collection_roots_follow_symlinks_and_refuse_other_kinds() -> TestResult {
    let guest = Guest::new().await?;
    guest.stat("/w/link", symlink("/w/link", "real")).await;
    guest.stat("/w/real", file("/w/real", 3)).await;
    guest.download("/w/real", b"abc").await;
    let reply = guest.run(archive_get("/w/link")).await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Archive {
                entries: 1,
                bytes: 3,
                ..
            }
        ),
        "{reply:?}"
    );

    guest.stat("/w/nul", symlink("/w/nul", "a\0b")).await;
    let reply = guest.run(archive_get("/w/nul")).await;
    assert!(failed(&reply, FileOpErrorKind::InvalidPath, "NUL"), "{reply:?}");

    guest.stat("/w/loop", symlink("/w/loop", "/w/loop")).await;
    let reply = guest.run(archive_get("/w/loop")).await;
    assert!(
        failed(&reply, FileOpErrorKind::InvalidPath, "too many symlinks"),
        "{reply:?}"
    );

    guest
        .stat("/w/device", entry("/w/device", "FILE_TYPE_UNSPECIFIED", 0))
        .await;
    let reply = guest.run(archive_get("/w/device")).await;
    assert!(
        failed(
            &reply,
            FileOpErrorKind::InvalidRequest,
            "not a regular file or directory"
        ),
        "{reply:?}"
    );
    Ok(())
}

#[tokio::test]
async fn rename_without_overwrite_reports_the_move() -> TestResult {
    let guest = Guest::new().await?;
    guest.script(0, "").await?;
    let reply = guest
        .run(FileOp::Rename {
            from: "/w/a".into(),
            to: "/w/b".into(),
            overwrite: FileOverwrite::Refuse,
        })
        .await;
    assert_eq!(reply, FileOpResponse::Renamed);
    Ok(())
}
