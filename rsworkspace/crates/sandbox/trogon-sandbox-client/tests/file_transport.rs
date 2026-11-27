mod common;
mod file_support;

use common::{MockBus, TestResult, client_replying, lease_id};
use file_support::client_for;
use trogon_sandbox_client::{ClientError, TransportError};
use trogon_sandbox_contract::{
    FileOp, FileOpErrorKind, FileOpResponse, LeaseError, ReadFileRequest, ReadFileResponse, SandboxFileWrite,
    SandboxUserName, WriteFilesRequest, WriteFilesResponse,
};

fn one_file(content: Vec<u8>) -> Vec<SandboxFileWrite> {
    vec![SandboxFileWrite {
        path: "/tmp/foo".into(),
        content,
        mode: None,
    }]
}

#[tokio::test]
async fn write_files_succeeds_when_broker_writes() -> TestResult {
    let (client, log) = client_replying(&WriteFilesResponse::Written)?;
    client.write_files(&lease_id(), one_file(b"bar".to_vec()), None).await?;
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.write_files.lease-1");
    assert_eq!(sent.timeout, std::time::Duration::from_secs(30));
    Ok(())
}

#[tokio::test]
async fn write_files_returns_denied_when_broker_denies() -> TestResult {
    let error = LeaseError::LeaseNotFound { lease_id: lease_id() };
    let (client, _) = client_replying(&WriteFilesResponse::Denied(error))?;
    let result = client.write_files(&lease_id(), one_file(b"bar".to_vec()), None).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn write_files_sends_the_contents_and_the_user() -> TestResult {
    let (client, log) = client_replying(&WriteFilesResponse::Written)?;
    let user = SandboxUserName::new("builder")?;
    client
        .write_files(&lease_id(), one_file(vec![b'a'; 2048]), Some(user.clone()))
        .await?;
    let request: WriteFilesRequest = serde_json::from_slice(&log.only()?.payload)?;
    assert_eq!(request.files[0].content, vec![b'a'; 2048]);
    assert_eq!(request.as_user, Some(user));
    Ok(())
}

#[tokio::test]
async fn write_files_refuses_a_request_over_the_transfer_cap() -> TestResult {
    let (client, log) = client_replying(&WriteFilesResponse::Written)?;
    // Every byte of 255 encodes as `255,` in JSON, so this request is just over 128 MiB.
    let result = client
        .write_files(&lease_id(), one_file(vec![255; 33_554_433]), None)
        .await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::FilesTooLargeForTransport {
            content_bytes: 33_554_433,
            ..
        }))
    ));
    assert!(log.is_empty());
    Ok(())
}

#[tokio::test]
async fn write_files_reports_a_missing_reply_as_a_timeout() -> TestResult {
    let client = MockBus::timing_out().into_client();
    let result = client.write_files(&lease_id(), one_file(Vec::new()), None).await;
    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}

#[tokio::test]
async fn read_file_returns_content_when_broker_replies() -> TestResult {
    let response = ReadFileResponse::Content {
        data: b"file-contents".to_vec(),
    };
    let (client, log) = client_replying(&response)?;
    let data = client.read_file(&lease_id(), "/tmp/foo".into(), None).await?;
    assert_eq!(data, b"file-contents".to_vec());
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.read_file.lease-1");
    let request: ReadFileRequest = serde_json::from_slice(&sent.payload)?;
    assert_eq!(request.path, "/tmp/foo");
    Ok(())
}

#[tokio::test]
async fn read_file_returns_denied_when_broker_denies() -> TestResult {
    let error = LeaseError::LeaseNotRunning { lease_id: lease_id() };
    let (client, _) = client_replying(&ReadFileResponse::Denied(error))?;
    let result = client.read_file(&lease_id(), "/tmp/foo".into(), None).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotRunning { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn read_file_reports_a_missing_file_as_file_not_found() -> TestResult {
    let (client, _) = client_replying(&ReadFileResponse::NotFound)?;
    let result = client.read_file(&lease_id(), "/tmp/foo".into(), None).await;
    assert!(matches!(result, Err(ClientError::FileNotFound(ref path)) if path == "/tmp/foo"));
    Ok(())
}

#[tokio::test]
async fn an_undecodable_reply_is_a_transport_error() -> TestResult {
    let client = MockBus::replying("not json".into()).into_client();
    let result = client.read_file(&lease_id(), "/tmp/foo".into(), None).await;
    assert!(matches!(result, Err(ClientError::Transport(TransportError::Decode(_)))));
    Ok(())
}

#[tokio::test]
async fn file_op_sends_the_request_on_the_file_op_subject() -> TestResult {
    let (client, log) = client_for(&FileOpResponse::Renamed)?;
    let op = FileOp::Stat { path: "/x".into() };
    let reply = client.file_op(&lease_id(), op, None).await?;
    assert_eq!(reply, FileOpResponse::Renamed);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.file_op.lease-1");
    assert_eq!(sent.timeout, std::time::Duration::from_secs(60));
    Ok(())
}

#[tokio::test]
async fn file_op_maps_typed_replies_to_typed_errors() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::Unsupported { message: "no".into() })?;
    let error = client.stat(&lease_id(), "/x", None).await.err();
    assert!(matches!(error, Some(ClientError::FileOpUnsupported(ref message)) if message == "no"));

    let (client, _) = client_for(&FileOpResponse::NotFound { path: "/x".into() })?;
    let error = client.stat(&lease_id(), "/x", None).await.err();
    assert!(matches!(error, Some(ClientError::FileNotFound(ref path)) if path == "/x"));

    let (client, _) = client_for(&FileOpResponse::Failed {
        kind: FileOpErrorKind::TooLarge,
        message: "big".into(),
    })?;
    let error = client.stat(&lease_id(), "/x", None).await.err();
    assert!(matches!(
        error,
        Some(ClientError::FileOpFailed {
            kind: FileOpErrorKind::TooLarge,
            ..
        })
    ));

    let (client, _) = client_for(&FileOpResponse::Denied(LeaseError::LeaseNotRunning {
        lease_id: lease_id(),
    }))?;
    let error = client.stat(&lease_id(), "/x", None).await.err();
    assert!(matches!(
        error,
        Some(ClientError::Denied(LeaseError::LeaseNotRunning { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn a_reply_of_the_wrong_kind_is_an_unexpected_reply() -> TestResult {
    let (client, _) = client_for(&FileOpResponse::Renamed)?;
    let error = client.stat(&lease_id(), "/x", None).await.err();
    assert!(matches!(
        error,
        Some(ClientError::Transport(TransportError::UnexpectedReply {
            operation: "stat",
            ..
        }))
    ));
    Ok(())
}
