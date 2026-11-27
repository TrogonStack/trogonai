mod support;

use std::error::Error;
use std::time::Duration;

use support::{Plan, Reply, Rig, TestResult, exit_body};
use trogon_sandbox_contract::{LeaseId, ReadFileRequest, SandboxFileWrite, WriteFilesRequest};
use trogon_sandbox_protocol::ReadFileError;
use trogon_sandbox_space_worker::FileTransferError;

fn write(lease: &LeaseId, path: &str, content: Vec<u8>, mode: Option<u32>) -> WriteFilesRequest {
    WriteFilesRequest {
        lease_id: lease.clone(),
        files: vec![SandboxFileWrite {
            path: path.into(),
            content,
            mode,
        }],
        as_user: None,
    }
}

fn read(lease: &LeaseId, path: &str) -> ReadFileRequest {
    ReadFileRequest {
        lease_id: lease.clone(),
        path: path.into(),
        as_user: None,
    }
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 251) as u8).collect()
}

fn other(failure: ReadFileError<FileTransferError>) -> Result<FileTransferError, Box<dyn Error>> {
    match failure {
        ReadFileError::Other(error) => Ok(error),
        ReadFileError::NotFound(missing) => Err(format!("not found: {missing:?}").into()),
    }
}

#[tokio::test]
async fn writes_a_file_in_chunks_and_installs_it() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let content = pattern(60 * 1024);
    rig.provider
        .write_files(&write(&lease, "/srv/app/data.bin", content.clone(), Some(0o600)))
        .await?;
    let state = rig.space.state();
    assert_eq!(state.fs.get("/srv/app/data.bin"), Some(&content));
    assert_eq!(state.modes.get("/srv/app/data.bin").map(String::as_str), Some("600"));
    assert!(!state.fs.contains_key("/srv/app/data.bin.sandbox.tmp"));
    Ok(())
}

#[tokio::test]
async fn writes_with_the_default_mode_and_an_empty_file() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.provider
        .write_files(&write(&lease, "/srv/empty", Vec::new(), None))
        .await?;
    let state = rig.space.state();
    assert_eq!(state.fs.get("/srv/empty"), Some(&Vec::new()));
    assert_eq!(state.modes.get("/srv/empty").map(String::as_str), Some("644"));
    Ok(())
}

#[tokio::test]
async fn reads_back_what_was_written_across_chunks() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let content = pattern(1_200_000);
    rig.space.state().fs.insert("/srv/big".into(), content.clone());
    let data = rig
        .provider
        .read_file(&read(&lease, "/srv/big"))
        .await
        .map_err(|_| "read failed")?;
    assert_eq!(data, content);
    Ok(())
}

#[tokio::test]
async fn reads_a_file_that_ends_on_a_chunk_boundary() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let content = pattern(524_288);
    rig.space.state().fs.insert("/srv/edge".into(), content.clone());
    let data = rig
        .provider
        .read_file(&read(&lease, "/srv/edge"))
        .await
        .map_err(|_| "read failed")?;
    assert_eq!(data, content);
    Ok(())
}

#[tokio::test]
async fn resolves_relative_paths_against_the_lease_directory() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let error = rig
        .provider
        .write_files(&write(&lease, "notes.txt", vec![1], None))
        .await
        .err()
        .ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::RelativePath));
    let error = other(
        rig.provider
            .read_file(&read(&lease, "notes.txt"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::RelativePath));
    Ok(())
}

#[tokio::test]
async fn reports_a_missing_file() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let failure = rig
        .provider
        .read_file(&read(&lease, "/srv/none"))
        .await
        .err()
        .ok_or("read")?;
    assert!(matches!(failure, ReadFileError::NotFound(_)));
    Ok(())
}

#[tokio::test]
async fn reports_a_guest_that_fails_the_read() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().fs_exit.insert("/srv/dir".into(), 1);
    let error = other(
        rig.provider
            .read_file(&read(&lease, "/srv/dir"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::ReadFailed));
    Ok(())
}

#[tokio::test]
async fn reports_a_reply_that_is_not_base64() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    {
        let mut state = rig.space.state();
        state.fs.insert("/srv/x".into(), vec![1]);
        state.garbage_read = true;
    }
    let error = other(
        rig.provider
            .read_file(&read(&lease, "/srv/x"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::NotBase64));
    Ok(())
}

#[tokio::test]
async fn refuses_a_file_over_the_limit() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space
        .state()
        .fs
        .insert("/srv/huge".into(), vec![7; 16 * 1024 * 1024 + 1]);
    let error = other(
        rig.provider
            .read_file(&read(&lease, "/srv/huge"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::TooLarge));
    Ok(())
}

#[tokio::test]
async fn gives_up_on_a_slow_read() -> TestResult {
    let rig = Rig::reading_for(Duration::from_millis(200)).await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().next_plans.push_back(Plan::Events(vec![Reply::Stall]));
    let error = other(
        rig.provider
            .read_file(&read(&lease, "/srv/slow"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::ReadTimedOut));
    Ok(())
}

#[tokio::test]
async fn reports_a_guest_command_that_timed_out() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let reply = Reply::Body(exit_body(b"", b"", 0, "timed_out"));
    rig.space.state().next_plans.push_back(Plan::Events(vec![reply]));
    let error = other(
        rig.provider
            .read_file(&read(&lease, "/srv/slow"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::GuestTimedOut));
    Ok(())
}

#[tokio::test]
async fn reports_a_guest_command_that_cannot_run() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().next_plans.push_back(Plan::CreateStatus(400));
    let error = other(
        rig.provider
            .read_file(&read(&lease, "/srv/x"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::Exec(_)));
    Ok(())
}

#[tokio::test]
async fn cleans_up_when_a_chunk_fails() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().fail_script = Some("base64 -d");
    let error = rig
        .provider
        .write_files(&write(&lease, "/srv/f", vec![1, 2, 3], None))
        .await
        .err()
        .ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::GuestFailed));
    assert!(rig.space.state().fs.is_empty());
    Ok(())
}

#[tokio::test]
async fn reports_a_guest_that_names_no_temporary_file() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space.state().empty_mktemp = true;
    let error = rig
        .provider
        .write_files(&write(&lease, "/srv/f", vec![1], None))
        .await
        .err()
        .ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::NoTemporaryFile));
    Ok(())
}

#[tokio::test]
async fn refuses_bad_write_requests() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let provider = &rig.provider;
    let error = provider
        .write_files(&write(&lease, "", vec![1], None))
        .await
        .err()
        .ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::InvalidPath));
    let error = provider
        .write_files(&write(&lease, "/a\u{0}b", vec![1], None))
        .await
        .err()
        .ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::InvalidPath));
    let error = provider
        .write_files(&write(&lease, "/a", vec![1], Some(0o10000)))
        .await
        .err()
        .ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::InvalidMode));
    let error = provider
        .write_files(&write(&lease, "/a", vec![0; 16 * 1024 * 1024 + 1], None))
        .await
        .err()
        .ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::TooMuchData));
    let mut many = write(&lease, "/a", vec![1], None);
    many.files = (0..257)
        .map(|index| SandboxFileWrite {
            path: format!("/f{index}"),
            content: Vec::new(),
            mode: None,
        })
        .collect();
    let error = provider.write_files(&many).await.err().ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::TooMuchData));
    let missing = write(&LeaseId::new("nobody"), "/a", vec![1], None);
    let error = provider.write_files(&missing).await.err().ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::Lease(_)));
    assert!(rig.space.state().commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn refuses_acting_as_another_user() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let user = serde_json::from_value(serde_json::json!("root"))?;
    let mut request = write(&lease, "/a", vec![1], None);
    request.as_user = Some(user);
    let error = rig.provider.write_files(&request).await.err().ok_or("wrote")?;
    assert!(matches!(error, FileTransferError::AsUser));
    let user = serde_json::from_value(serde_json::json!("root"))?;
    let mut request = read(&lease, "/a");
    request.as_user = Some(user);
    let error = other(rig.provider.read_file(&request).await.err().ok_or("read")?)?;
    assert!(matches!(error, FileTransferError::AsUser));
    Ok(())
}

#[tokio::test]
async fn reads_from_a_lease_it_does_not_hold() -> TestResult {
    let rig = Rig::start().await?;
    let error = other(
        rig.provider
            .read_file(&read(&LeaseId::new("nobody"), "/a"))
            .await
            .err()
            .ok_or("read")?,
    )?;
    assert!(matches!(error, FileTransferError::Lease(_)));
    Ok(())
}
