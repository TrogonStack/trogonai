mod support;

use std::time::Duration;

use support::{FakeRegistry, TestResult, push_artifact};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::{DockerConfigSource, OciClient, OciClientConfig};

#[tokio::test]
async fn deleting_from_an_insecure_registry_sends_a_plain_http_delete_to_the_manifest_path() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let artifact = push_artifact(&registry, "snapshots/acme", ArtifactType::SbomV1, b"payload").await?;

    client.delete_artifact(&artifact).await?;

    let expected = format!("DELETE /v2/snapshots/acme/manifests/{}", artifact.digest);
    assert!(registry.requests().contains(&expected), "{:?}", registry.requests());
    Ok(())
}

#[tokio::test]
async fn deleting_from_a_registry_that_is_not_insecure_uses_https() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let artifact = push_artifact(&registry, "snapshots", ArtifactType::SbomV1, b"payload").await?;
    let secure = OciClient::new(OciClientConfig {
        docker_config: DockerConfigSource::Disabled,
        ..Default::default()
    });

    let result = tokio::time::timeout(Duration::from_secs(30), secure.delete_artifact(&artifact)).await?;

    assert!(result.is_err(), "a plain-http registry must not accept an https delete");
    assert!(
        !registry.requests().iter().any(|request| request.starts_with("DELETE ")),
        "{:?}",
        registry.requests()
    );
    assert!(registry.manifest("snapshots", artifact.digest.as_str()).is_some());
    Ok(())
}
