#![cfg(unix)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use support::{Fixture, TestResult, entry_names};

use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, LeaseId};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};

fn request(fixture: &Fixture, path: &str, find: &str, replace: &str, count: u32) -> FileOpRequest {
    FileOpRequest {
        lease_id: LeaseId::new("lease-files"),
        as_user: None,
        op: FileOp::Edit {
            path: fixture.path(path).to_string_lossy().into_owned(),
            find: find.into(),
            replace: replace.into(),
            expected_occurrences: count,
        },
    }
}

#[tokio::test]
async fn edits_replace_exact_occurrences_preserve_mode_and_leave_no_staging_files() -> TestResult {
    let fixture = Fixture::new().await?;
    let path = fixture.path("text");
    std::fs::write(&path, "one λ one")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
    let before = entry_names(&fixture.path(""))?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(&fixture, "text", "one", "two", 1))
            .await,
        FileOpResponse::EditRejected {
            occurrences: 2,
            expected: 1
        }
    );
    assert_eq!(std::fs::read(&path)?, "one λ one".as_bytes());
    assert_eq!(fixture.shell_calls(), 0);
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(&fixture, "text", "one", "two", 2))
            .await,
        FileOpResponse::Edited {
            occurrences: 2,
            size: 10
        }
    );
    assert_eq!(std::fs::read(&path)?, "two λ two".as_bytes());
    assert_eq!(std::fs::metadata(path)?.permissions().mode() & 0o7777, 0o640);
    assert_eq!(entry_names(&fixture.path(""))?, before);
    Ok(())
}

#[tokio::test]
async fn edits_refuse_stale_original_content_without_overwriting_the_guest_writer() -> TestResult {
    let fixture = Fixture::new().await?;
    std::fs::write(fixture.path("text"), b"before")?;
    let before = entry_names(&fixture.path(""))?;
    fixture.overwrite_before_shell("text", b"guest writer")?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(&fixture.sandbox, &request(&fixture, "text", "before", "after", 1))
        .await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::Io,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(std::fs::read(fixture.path("text"))?, b"guest writer");
    assert_eq!(entry_names(&fixture.path(""))?, before);
    Ok(())
}

#[tokio::test]
async fn edits_follow_a_final_symlink_without_replacing_the_link() -> TestResult {
    let fixture = Fixture::new().await?;
    std::fs::write(fixture.path("target"), b"before")?;
    std::os::unix::fs::symlink("target", fixture.path("link"))?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(&fixture, "link", "before", "after", 1))
            .await,
        FileOpResponse::Edited {
            occurrences: 1,
            size: 5
        }
    );
    assert_eq!(std::fs::read(fixture.path("target"))?, b"after");
    assert!(std::fs::symlink_metadata(fixture.path("link"))?.is_symlink());
    Ok(())
}

#[tokio::test]
async fn edits_reject_invalid_text_requests_and_preserve_missing_or_directory_errors() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    std::fs::write(fixture.path("binary"), [255])?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    for (path, find, count) in [("binary", "a", 1), ("binary", "", 1), ("binary", "a", 0)] {
        let reply = exec
            .handle_file_op(&fixture.sandbox, &request(&fixture, path, find, "b", count))
            .await;
        assert!(
            matches!(
                reply,
                FileOpResponse::Failed {
                    kind: FileOpErrorKind::InvalidRequest,
                    ..
                }
            ),
            "{reply:?}"
        );
    }
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(&fixture, "missing", "a", "b", 1))
            .await,
        FileOpResponse::NotFound {
            path: fixture.path("missing").to_string_lossy().into_owned()
        }
    );
    let reply = exec
        .handle_file_op(&fixture.sandbox, &request(&fixture, "home", "a", "b", 1))
        .await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::IsADirectory,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(fixture.shell_calls(), 0);
    Ok(())
}

#[tokio::test]
async fn edits_bound_replacement_expansion_before_allocating_or_uploading() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    std::fs::write(fixture.path("text"), b"aa")?;
    let before = entry_names(&fixture.path(""))?;
    let replacement = "b".repeat(FileOp::MAX_WRITE_BYTES as usize / 2 + 1);
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(&fixture.sandbox, &request(&fixture, "text", "a", &replacement, 2))
        .await;
    assert!(
        matches!(
            reply,
            FileOpResponse::Failed {
                kind: FileOpErrorKind::TooLarge,
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(std::fs::read(fixture.path("text"))?, b"aa");
    assert_eq!(entry_names(&fixture.path(""))?, before);
    assert_eq!(fixture.shell_calls(), 0);
    Ok(())
}
