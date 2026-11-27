mod support;

use support::{FakeRegistry, TestResult, push_artifact};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::OciError;

#[tokio::test]
async fn deleting_removes_the_manifest_and_reports_later_failures() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let artifact = push_artifact(&registry, "snapshots", ArtifactType::SbomV1, b"payload").await?;
    client.delete_artifact(&artifact).await?;
    assert!(registry.manifest("snapshots", artifact.digest.as_str()).is_none());

    let error = client.delete_artifact(&artifact).await.err();
    assert!(matches!(error, Some(OciError::RegistryRejected(message)) if message.contains("HTTP 404")));
    Ok(())
}

#[tokio::test]
async fn deleting_reports_connection_failures_as_network_errors() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    drop(listener);
    let client = support::client_for(&address)?;
    let (artifact, _) = support::artifact(&address, b"x")?;
    let error = client.delete_artifact(&artifact).await.err();
    assert!(matches!(error, Some(OciError::Network(_))), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn deleting_reports_malformed_responses_as_rejections() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let _ = socket.write_all(b"garbage\r\n\r\n").await;
        }
    });
    let client = support::client_for(&address)?;
    let (artifact, _) = support::artifact(&address, b"x")?;
    let error = client.delete_artifact(&artifact).await.err();
    assert!(matches!(error, Some(OciError::RegistryRejected(_))), "{error:?}");
    Ok(())
}
