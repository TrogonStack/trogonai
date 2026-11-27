mod transfer_support;

use std::fs;

use transfer_support::{Harness, TestResult, pattern};
use trogon_sandbox_client::{ClientError, FILE_TRANSFER_CHUNK_BYTES, TransportError};
use trogon_sandbox_contract::{
    FileOp, FileOpErrorKind, FileOpResponse, FileReadEnd, FileSha256, LeaseError, SandboxUserName,
};

const CHUNK: usize = FILE_TRANSFER_CHUNK_BYTES as usize;

#[tokio::test]
async fn a_file_larger_than_one_chunk_round_trips_in_chunks_and_verifies() -> TestResult {
    let h = Harness::new()?;
    let data = pattern(CHUNK * 2 + 123);
    let source = h.local.path().join("big.bin");
    fs::write(&source, &data)?;

    let report = h.client.upload_file_from(&h.lease, &source, "/big.bin", None).await?;
    assert_eq!(report.bytes_transferred, data.len() as u64);
    assert_eq!(fs::read(h.sandbox.path().join("big.bin"))?, data);
    let chunk = FILE_TRANSFER_CHUNK_BYTES;
    assert_eq!(h.writes(), vec![(0, true), (chunk, false), (chunk * 2, false)]);

    let back = h.local.path().join("back.bin");
    let report = h.client.download_file_to(&h.lease, "/big.bin", &back, None).await?;
    assert_eq!(report.bytes_transferred, data.len() as u64);
    assert_eq!(fs::read(&back)?, data);
    Ok(())
}

#[tokio::test]
async fn an_empty_file_and_an_exact_multiple_of_the_chunk_size_transfer() -> TestResult {
    let h = Harness::new()?;
    let empty = h.local.path().join("empty");
    fs::write(&empty, b"")?;
    h.client.upload_file_from(&h.lease, &empty, "/empty", None).await?;
    assert_eq!(fs::metadata(h.sandbox.path().join("empty"))?.len(), 0);

    let exact = pattern(CHUNK * 2);
    let source = h.local.path().join("exact");
    fs::write(&source, &exact)?;
    h.client.upload_file_from(&h.lease, &source, "/exact", None).await?;
    assert_eq!(fs::read(h.sandbox.path().join("exact"))?, exact);
    Ok(())
}

#[tokio::test]
async fn an_interrupted_upload_resumes_from_the_bytes_already_there() -> TestResult {
    let h = Harness::new()?;
    let data = pattern(CHUNK * 2 + 10);
    let source = h.local.path().join("f.bin");
    fs::write(&source, &data)?;
    fs::write(h.sandbox.path().join("f.bin"), &data[..CHUNK])?;

    let report = h.client.upload_file_from(&h.lease, &source, "/f.bin", None).await?;
    assert_eq!(report.resumed_files, 1);
    assert_eq!(report.bytes_transferred, (CHUNK + 10) as u64);
    assert_eq!(h.writes(), vec![(CHUNK as u64, false), (CHUNK as u64 * 2, false)]);
    assert_eq!(fs::read(h.sandbox.path().join("f.bin"))?, data);
    Ok(())
}

#[tokio::test]
async fn a_partial_upload_that_is_not_a_prefix_restarts_from_zero() -> TestResult {
    let h = Harness::new()?;
    let data = pattern(1000);
    let source = h.local.path().join("f.bin");
    fs::write(&source, &data)?;
    fs::write(h.sandbox.path().join("f.bin"), vec![9u8; 400])?;

    let report = h.client.upload_file_from(&h.lease, &source, "/f.bin", None).await?;
    assert_eq!(report.resumed_files, 0);
    assert_eq!(h.writes(), vec![(0, true)]);
    assert_eq!(fs::read(h.sandbox.path().join("f.bin"))?, data);
    Ok(())
}

#[tokio::test]
async fn a_finished_upload_is_skipped_when_rerun() -> TestResult {
    let h = Harness::new()?;
    let source = h.local.path().join("f.bin");
    fs::write(&source, pattern(5000))?;
    h.client.upload_file_from(&h.lease, &source, "/f.bin", None).await?;
    h.clear_ops();
    let report = h.client.upload_file_from(&h.lease, &source, "/f.bin", None).await?;
    assert_eq!(report.skipped_files, 1);
    assert_eq!(report.bytes_transferred, 0);
    assert!(h.writes().is_empty());
    Ok(())
}

#[tokio::test]
async fn an_interrupted_download_resumes_and_a_corrupt_prefix_restarts() -> TestResult {
    let h = Harness::new()?;
    let data = pattern(CHUNK + 77);
    fs::write(h.sandbox.path().join("f.bin"), &data)?;

    let dest = h.local.path().join("f.bin");
    fs::write(&dest, &data[..CHUNK])?;
    let report = h.client.download_file_to(&h.lease, "/f.bin", &dest, None).await?;
    assert_eq!(report.resumed_files, 1);
    assert_eq!(report.bytes_transferred, 77);
    assert_eq!(fs::read(&dest)?, data);

    fs::write(&dest, vec![1u8; 500])?;
    let report = h.client.download_file_to(&h.lease, "/f.bin", &dest, None).await?;
    assert_eq!(report.resumed_files, 0);
    assert_eq!(report.bytes_transferred, data.len() as u64);
    assert_eq!(fs::read(&dest)?, data);

    fs::write(&dest, vec![1u8; data.len() + 5])?;
    let report = h.client.download_file_to(&h.lease, "/f.bin", &dest, None).await?;
    assert_eq!(report.bytes_transferred, data.len() as u64);
    Ok(())
}

#[tokio::test]
async fn a_download_that_does_not_match_the_sandbox_digest_fails() -> TestResult {
    let h = Harness::tampering(transfer_support::corrupt_reads)?;
    fs::write(h.sandbox.path().join("f.bin"), pattern(2000))?;
    let error = h
        .client
        .download_file_to(&h.lease, "/f.bin", &h.local.path().join("f.bin"), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::IntegrityCheckFailed { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_upload_whose_sandbox_digest_differs_fails() -> TestResult {
    let h = Harness::tampering(|op, reply| match (op, reply) {
        (FileOp::Checksum { length: None, .. }, FileOpResponse::Checksum { bytes, total_size, .. }) => {
            match FileSha256::new("0".repeat(64)) {
                Ok(sha256) => FileOpResponse::Checksum {
                    sha256,
                    bytes,
                    total_size,
                },
                Err(_) => FileOpResponse::Renamed,
            }
        }
        (_, reply) => reply,
    })?;
    let source = h.local.path().join("f.bin");
    fs::write(&source, pattern(300))?;
    let error = h
        .client
        .upload_file_from(&h.lease, &source, "/f.bin", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::IntegrityCheckFailed { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_download_refuses_a_destination_that_is_not_a_file() -> TestResult {
    let h = Harness::new()?;
    fs::write(h.sandbox.path().join("f.bin"), b"x")?;
    let error = h
        .client
        .download_file_to(&h.lease, "/f.bin", h.local.path(), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(error, ClientError::Transport(TransportError::NotAFile { .. })));
    Ok(())
}

#[tokio::test]
async fn a_download_of_a_missing_file_is_not_found() -> TestResult {
    let h = Harness::new()?;
    let error = h
        .client
        .download_file_to(&h.lease, "/nope", &h.local.path().join("x"), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(error, ClientError::FileNotFound(_)));
    Ok(())
}

#[tokio::test]
async fn a_download_whose_parent_cannot_be_created_fails_locally() -> TestResult {
    let h = Harness::new()?;
    fs::write(h.sandbox.path().join("f.bin"), b"x")?;
    let blocker = h.local.path().join("blocker");
    fs::write(&blocker, b"")?;
    let error = h
        .client
        .download_file_to(&h.lease, "/f.bin", &blocker.join("f.bin"), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(error, ClientError::Transport(TransportError::Local { .. })));
    Ok(())
}

#[tokio::test]
async fn a_download_notices_the_file_changing_size_between_chunks() -> TestResult {
    let h = Harness::tampering(|_, reply| transfer_support::with_content(reply, |_, total, _| *total += 1))?;
    fs::write(h.sandbox.path().join("f.bin"), b"hello")?;
    let error = h
        .client
        .download_file_to(&h.lease, "/f.bin", &h.local.path().join("f.bin"), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::SizeChanged { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_download_notices_an_empty_chunk_before_the_end() -> TestResult {
    let h = Harness::tampering(|_, reply| transfer_support::with_content(reply, |raw, _, _| raw.clear()))?;
    fs::write(h.sandbox.path().join("f.bin"), b"hello")?;
    let error = h
        .client
        .download_file_to(&h.lease, "/f.bin", &h.local.path().join("f.bin"), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::EmptyChunk { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_upload_refuses_a_source_that_is_not_a_file_or_is_missing() -> TestResult {
    let h = Harness::new()?;
    let error = h
        .client
        .upload_file_from(&h.lease, h.local.path(), "/f", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(error, ClientError::Transport(TransportError::NotAFile { .. })));
    let error = h
        .client
        .upload_file_from(&h.lease, &h.local.path().join("missing"), "/f", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(error, ClientError::Transport(TransportError::Local { .. })));
    Ok(())
}

#[tokio::test]
async fn an_upload_onto_a_directory_is_refused() -> TestResult {
    let h = Harness::new()?;
    fs::create_dir(h.sandbox.path().join("d"))?;
    let source = h.local.path().join("f");
    fs::write(&source, b"x")?;
    let error = h
        .client
        .upload_file_from(&h.lease, &source, "/d", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(
        error,
        ClientError::FileOpFailed {
            kind: FileOpErrorKind::IsADirectory,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn an_upload_surfaces_a_denied_stat() -> TestResult {
    let h = Harness::tampering(|op, reply| match op {
        FileOp::Stat { .. } => FileOpResponse::Denied(LeaseError::NoProviderAvailable),
        _ => reply,
    })?;
    let source = h.local.path().join("f");
    fs::write(&source, b"x")?;
    let error = h
        .client
        .upload_file_from(&h.lease, &source, "/f", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(error, ClientError::Denied(LeaseError::NoProviderAvailable)));
    Ok(())
}

#[tokio::test]
async fn an_upload_whose_source_shrinks_while_verifying_fails() -> TestResult {
    let source_cell = std::sync::Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    let seen = source_cell.clone();
    let h = Harness::tampering(move |op, reply| {
        if matches!(op, FileOp::Checksum { length: None, .. })
            && let Ok(path) = seen.lock()
            && let Some(path) = path.as_ref()
        {
            let _ = fs::write(path, b"");
        }
        reply
    })?;
    let source = h.local.path().join("f");
    fs::write(&source, pattern(100))?;
    if let Ok(mut cell) = source_cell.lock() {
        *cell = Some(source.clone());
    }
    let error = h
        .client
        .upload_file_from(&h.lease, &source, "/f", None)
        .await
        .err()
        .ok_or("the upload should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::ShorterThanExpected { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_unexpected_reply_to_each_transfer_verb_is_a_transport_error() -> TestResult {
    let h = Harness::tampering(|_, _| FileOpResponse::Renamed)?;
    let source = h.local.path().join("f");
    fs::write(&source, b"x")?;
    let dest = h.local.path().join("g");
    let errors = [
        h.client.file_checksum(&h.lease, "/f", 0, None, None).await.err(),
        h.client.download_file(&h.lease, "/f", 10, None).await.err(),
        h.client.upload_file(&h.lease, "/f", b"x", None, None).await.err(),
        h.client.download_archive(&h.lease, "/f", None, None).await.err(),
        h.client.upload_archive(&h.lease, "/f", b"x", None).await.err(),
        h.client.download_file_to(&h.lease, "/f", &dest, None).await.err(),
        h.client.upload_dir(&h.lease, h.local.path(), "/d", None).await.err(),
    ];
    for error in errors {
        let error = error.ok_or("the call should fail")?;
        assert!(matches!(
            error,
            ClientError::Transport(TransportError::UnexpectedReply { .. })
        ));
    }
    let list = Harness::tampering(|_, _| FileOpResponse::Renamed)?;
    let error = list
        .client
        .download_dir(&list.lease, "/d", list.local.path(), None)
        .await
        .err()
        .ok_or("the call should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::UnexpectedReply { .. })
    ));
    let error = h
        .client
        .upload_file_from(&h.lease, &source, "/f", None)
        .await
        .err()
        .ok_or("the call should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::UnexpectedReply { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn a_checksum_reports_the_digest_of_a_range() -> TestResult {
    let h = Harness::new()?;
    fs::write(h.sandbox.path().join("f"), b"abc")?;
    let user = SandboxUserName::new("root")?;
    let sum = h.client.file_checksum(&h.lease, "/f", 0, None, Some(user)).await?;
    assert_eq!(
        sum.sha256.as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!((sum.bytes, sum.total_size), (3, 3));
    Ok(())
}

#[tokio::test]
async fn download_file_reads_in_chunks_and_enforces_the_limit() -> TestResult {
    let h = Harness::new()?;
    let data = pattern(CHUNK + 9);
    fs::write(h.sandbox.path().join("f"), &data)?;
    assert_eq!(h.client.download_file(&h.lease, "/f", u64::MAX, None).await?, data);

    let error = h
        .client
        .download_file(&h.lease, "/f", 10, None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(
        error,
        ClientError::FileOpFailed {
            kind: FileOpErrorKind::TooLarge,
            ..
        }
    ));

    let early = Harness::tampering(|_, reply| {
        transfer_support::with_content(reply, |raw, _, eof| {
            raw.truncate(3);
            *eof = FileReadEnd::End;
        })
    })?;
    fs::write(early.sandbox.path().join("f"), &data)?;
    assert_eq!(
        early
            .client
            .download_file(&early.lease, "/f", u64::MAX, None)
            .await?
            .len(),
        3
    );
    Ok(())
}

#[tokio::test]
async fn download_file_notices_a_size_change_and_an_empty_chunk() -> TestResult {
    let data = pattern(CHUNK + 9);
    let growing = Harness::tampering(|op, reply| match op {
        FileOp::Read { offset, .. } if *offset > 0 => transfer_support::with_content(reply, |_, total, _| *total += 1),
        _ => reply,
    })?;
    fs::write(growing.sandbox.path().join("f"), &data)?;
    let error = growing
        .client
        .download_file(&growing.lease, "/f", u64::MAX, None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::SizeChanged { .. })
    ));

    let stalled = Harness::tampering(|_, reply| transfer_support::with_content(reply, |raw, _, _| raw.clear()))?;
    fs::write(stalled.sandbox.path().join("f"), &data)?;
    let error = stalled
        .client
        .download_file(&stalled.lease, "/f", u64::MAX, None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(
        error,
        ClientError::Transport(TransportError::EmptyChunk { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn upload_file_writes_ordered_chunks_with_the_mode_on_the_first() -> TestResult {
    let h = Harness::new()?;
    let data = pattern(CHUNK + 5);
    let size = h.client.upload_file(&h.lease, "/f", &data, Some(0o600), None).await?;
    assert_eq!(size, data.len() as u64);
    assert_eq!(h.writes(), vec![(0, true), (CHUNK as u64, false)]);
    assert_eq!(fs::read(h.sandbox.path().join("f"))?, data);
    Ok(())
}

#[tokio::test]
async fn archives_round_trip_a_tree() -> TestResult {
    let h = Harness::new()?;
    fs::create_dir_all(h.sandbox.path().join("src/sub"))?;
    fs::write(h.sandbox.path().join("src/sub/a.txt"), b"alpha")?;
    let archive = h.client.download_archive(&h.lease, "/src", None, None).await?;
    assert!(!archive.is_empty());
    let entries = h.client.upload_archive(&h.lease, "/dst", &archive, None).await?;
    assert!(entries >= 1);
    assert_eq!(fs::read(h.sandbox.path().join("dst/sub/a.txt"))?, b"alpha");
    Ok(())
}

#[tokio::test]
async fn a_download_to_an_empty_path_fails_locally() -> TestResult {
    let h = Harness::new()?;
    fs::write(h.sandbox.path().join("f"), b"x")?;
    let error = h
        .client
        .download_file_to(&h.lease, "/f", std::path::Path::new(""), None)
        .await
        .err()
        .ok_or("the download should fail")?;
    assert!(matches!(error, ClientError::Transport(TransportError::Local { .. })));
    Ok(())
}
