#![cfg(unix)]

mod support;

use std::time::Duration;

use support::{Fixture, TestResult};

use trogon_sandbox_contract::{FileOp, FileOpErrorKind, FileOpRequest, FileOpResponse, FileSha256, LeaseId};
use trogon_sandbox_e2b_worker::{E2bExec, Envd};

fn request(fixture: &Fixture, path: &str, offset: u64, length: Option<u64>) -> FileOpRequest {
    FileOpRequest {
        lease_id: LeaseId::new("lease-files"),
        as_user: None,
        op: FileOp::Checksum {
            path: fixture.path(path).to_string_lossy().into_owned(),
            offset,
            length,
        },
    }
}

#[tokio::test]
async fn checksums_hash_the_exact_binary_range_and_empty_ranges() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    std::fs::write(fixture.path("bytes"), b"\0abc\xff")?;
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(&fixture, "bytes", 1, Some(3)))
            .await,
        FileOpResponse::Checksum {
            sha256: FileSha256::new("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")?,
            bytes: 3,
            total_size: 5,
        }
    );
    for (offset, length) in [(0, Some(0)), (u64::MAX, None)] {
        assert_eq!(
            exec.handle_file_op(&fixture.sandbox, &request(&fixture, "bytes", offset, length))
                .await,
            FileOpResponse::Checksum {
                sha256: FileSha256::new("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")?,
                bytes: 0,
                total_size: 5,
            }
        );
    }
    Ok(())
}

#[tokio::test]
async fn checksums_stream_beyond_the_read_cap_without_extra_download_requests() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    let data = vec![42; FileOp::MAX_READ_BYTES as usize + 4096];
    std::fs::write(fixture.path("large"), &data)?;
    let mut hasher = trogon_sandbox_file_ops::StreamHasher::new();
    hasher.update(&data);
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(&fixture, "large", 0, None))
            .await,
        FileOpResponse::Checksum {
            sha256: hasher.finish()?,
            bytes: data.len() as u64,
            total_size: data.len() as u64,
        }
    );
    assert_eq!(fixture.calls(), 2, "one native Stat and one streamed file GET");
    Ok(())
}

#[tokio::test]
async fn checksums_keep_missing_and_directory_failures_typed() -> TestResult {
    let fixture = Fixture::new().await?;
    fixture.disable_shell();
    let exec = E2bExec::new(Envd::new(Duration::from_secs(5))?);
    assert_eq!(
        exec.handle_file_op(&fixture.sandbox, &request(&fixture, "missing", 0, None))
            .await,
        FileOpResponse::NotFound {
            path: fixture.path("missing").to_string_lossy().into_owned(),
        }
    );
    let reply = exec
        .handle_file_op(&fixture.sandbox, &request(&fixture, "home", 0, None))
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
    Ok(())
}
