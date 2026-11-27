#![cfg(unix)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use support::{Fixture, TestResult, entry_names};

use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, LeaseId};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};

fn request(op: FileOp) -> FileOpRequest {
    FileOpRequest {
        lease_id: LeaseId::new("lease-files"),
        as_user: None,
        op,
    }
}

#[tokio::test]
async fn chunk_writes_preserve_offsets_truncation_modes_and_parent_intent() -> TestResult {
    let fixture = Fixture::new().await?;
    let mut expected_names = entry_names(&fixture.path(""))?;
    expected_names.extend(["data.bin".to_string(), "nested".to_string()]);
    expected_names.sort();
    let path = fixture.path("data.bin");
    std::fs::write(&path, b"abcdef")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Write {
                path: path.to_string_lossy().into_owned(),
                data: support::data(&[0, 255])?,
                offset: 2,
                truncate: false.into(),
                mode: None,
                create_parents: false.into(),
            }),
        )
        .await;
    assert_eq!(reply, FileOpResponse::Written { bytes: 2, size: 6 });
    assert_eq!(std::fs::read(&path)?, b"ab\x00\xffef");
    assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o7777, 0o640);

    assert_eq!(
        exec.handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Write {
                path: path.to_string_lossy().into_owned(),
                data: support::data(&[1, 2, 3])?,
                offset: 0,
                truncate: true.into(),
                mode: Some(0o600),
                create_parents: false.into(),
            })
        )
        .await,
        FileOpResponse::Written { bytes: 3, size: 3 }
    );
    assert_eq!(std::fs::read(&path)?, [1, 2, 3]);
    assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o7777, 0o600);

    let nested = fixture.path("nested/data.bin");
    assert_eq!(
        exec.handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Write {
                path: nested.to_string_lossy().into_owned(),
                data: support::data(&[9])?,
                offset: 3,
                truncate: false.into(),
                mode: Some(0o644),
                create_parents: true.into(),
            })
        )
        .await,
        FileOpResponse::Written { bytes: 1, size: 4 }
    );
    assert_eq!(std::fs::read(&nested)?, [0, 0, 0, 9]);
    assert_eq!(
        exec.handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Write {
                path: path.to_string_lossy().into_owned(),
                data: support::data(&[7])?,
                offset: 2,
                truncate: true.into(),
                mode: None,
                create_parents: false.into(),
            })
        )
        .await,
        FileOpResponse::Written { bytes: 1, size: 3 }
    );
    assert_eq!(std::fs::read(&path)?, [0, 0, 7]);
    let names = entry_names(&fixture.path(""))?;
    assert_eq!(names, expected_names);
    assert!(fixture.shell_calls() > 0);
    Ok(())
}

#[tokio::test]
async fn writes_refuse_oversized_payloads_before_any_guest_call() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Write {
                path: fixture.path("large.bin").to_string_lossy().into_owned(),
                data: support::data(&vec![0; FileOp::MAX_WRITE_BYTES as usize + 1])?,
                offset: 0,
                truncate: true.into(),
                mode: None,
                create_parents: true.into(),
            }),
        )
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
    assert_eq!(fixture.calls(), 0);
    Ok(())
}

#[tokio::test]
async fn writes_without_parent_creation_preserve_missing_directories() -> TestResult {
    let fixture = Fixture::new().await?;
    let path = fixture.path("missing/data.bin");
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Write {
                path: path.to_string_lossy().into_owned(),
                data: support::data(&[1])?,
                offset: 0,
                truncate: true.into(),
                mode: None,
                create_parents: false.into(),
            }),
        )
        .await;
    assert_eq!(
        reply,
        FileOpResponse::NotFound {
            path: path.to_string_lossy().into_owned()
        }
    );
    assert!(!fixture.path("missing").exists());
    Ok(())
}

#[tokio::test]
async fn writes_refuse_overflowing_ranges_before_guest_calls() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    let path = fixture.path("existing");
    std::fs::write(&path, b"preserve")?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    let reply = exec
        .handle_file_op(
            &fixture.sandbox,
            &request(FileOp::Write {
                path: path.to_string_lossy().into_owned(),
                data: support::data(&[1])?,
                offset: u64::MAX,
                truncate: true.into(),
                mode: None,
                create_parents: false.into(),
            }),
        )
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
    assert_eq!(std::fs::read(path)?, b"preserve");
    assert_eq!(fixture.calls(), 0);
    Ok(())
}
