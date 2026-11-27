mod support;

use serde_json::json;
use support::{FakeRegistry, TestResult, push_artifact, sha256};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::{OciClient, OciClientConfig, OciError, OciMetrics};

#[tokio::test]
async fn attached_referrers_are_listed_through_the_tag_index() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let metrics = OciMetrics::new()?;
    let client = OciClient::with_metrics(support::config_for(&registry.address)?, Some(metrics.clone()));
    let subject = push_artifact(&registry, "snapshots", ArtifactType::FirecrackerSnapshotV1, b"disk").await?;
    let sbom = push_artifact(&registry, "snapshots", ArtifactType::SbomV1, b"{}").await?;
    let scan = push_artifact(&registry, "snapshots", ArtifactType::ScanReportV1, b"report").await?;

    assert!(client.list_referrers(&subject).await?.is_empty());
    client.attach_referrer(&subject, &sbom).await?;
    client.attach_referrer(&subject, &sbom).await?;
    client.attach_referrer(&subject, &scan).await?;

    let listed = client.list_referrers(&subject).await?;
    assert_eq!(listed, vec![sbom, scan]);
    assert!(metrics.gather_text()?.contains("referrers_fallback_total"));
    Ok(())
}

#[tokio::test]
async fn native_referrers_skip_entries_with_unknown_artifact_types() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let subject = push_artifact(&registry, "snapshots", ArtifactType::SandboxRootfsV1, b"disk").await?;
    let sbom = push_artifact(&registry, "snapshots", ArtifactType::SbomV1, b"{}").await?;
    let index = json!({
        "schemaVersion": 2,
        "mediaType": support::OCI_INDEX,
        "manifests": [
            {"mediaType": support::OCI_MANIFEST, "digest": sbom.digest.as_str(), "size": 10,
             "artifactType": ArtifactType::SbomV1.media_type()},
            {"mediaType": support::OCI_MANIFEST, "digest": subject.digest.as_str(), "size": 10,
             "artifactType": "fixture/unknown"},
        ],
    });
    registry.set_native_referrers(&serde_json::to_vec(&index)?);
    assert_eq!(client.list_referrers(&subject).await?, vec![sbom]);
    Ok(())
}

#[tokio::test]
async fn malformed_referrer_indexes_are_refused() -> TestResult {
    let bodies = [
        b"not json".to_vec(),
        serde_json::to_vec(&json!({"schemaVersion": 2, "mediaType": support::OCI_INDEX}))?,
        serde_json::to_vec(&json!({"schemaVersion": 1, "mediaType": support::OCI_INDEX, "manifests": []}))?,
        serde_json::to_vec(&json!({"schemaVersion": 2, "mediaType": "text/plain", "manifests": []}))?,
    ];
    let invalid_digest = serde_json::to_vec(&json!({"schemaVersion": 2, "mediaType": support::OCI_INDEX,
        "manifests": [{"mediaType": support::OCI_MANIFEST, "digest": "bad", "size": 1,
            "artifactType": ArtifactType::SbomV1.media_type()}]}))?;
    let registry = FakeRegistry::start().await?;
    let subject = push_artifact(&registry, "snapshots", ArtifactType::SandboxRootfsV1, b"disk").await?;
    registry.set_native_referrers(&invalid_digest);
    assert!(registry.client()?.list_referrers(&subject).await.is_err());
    for body in bodies {
        let registry = FakeRegistry::start().await?;
        let subject = push_artifact(&registry, "snapshots", ArtifactType::SandboxRootfsV1, b"disk").await?;
        registry.set_native_referrers(&body);
        let error = registry.client()?.list_referrers(&subject).await.err();
        assert!(matches!(error, Some(OciError::Layout(_))), "{error:?}");
    }
    Ok(())
}

#[tokio::test]
async fn referrer_entries_with_foreign_urls_are_refused() -> TestResult {
    for urls in [json!(["https://fixture.invalid/x"]), json!("https://fixture.invalid/x")] {
        let registry = FakeRegistry::start().await?;
        let subject = push_artifact(&registry, "snapshots", ArtifactType::SandboxRootfsV1, b"disk").await?;
        let index = json!({"schemaVersion": 2, "mediaType": support::OCI_INDEX,
            "manifests": [{"mediaType": support::OCI_MANIFEST, "digest": subject.digest.as_str(), "size": 1, "urls": urls}]});
        registry.set_native_referrers(&serde_json::to_vec(&index)?);
        let error = registry.client()?.list_referrers(&subject).await.err();
        assert!(matches!(error, Some(OciError::RegistryRejected(_))), "{error:?}");
    }
    Ok(())
}

#[tokio::test]
async fn attaching_a_referrer_with_an_invalid_manifest_fails() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let subject = push_artifact(&registry, "snapshots", ArtifactType::SandboxRootfsV1, b"disk").await?;
    let junk = b"{\"junk\":true}".to_vec();
    registry.put_manifest("snapshots", &sha256(&junk), support::OCI_MANIFEST, &junk);
    let referrer = support::artifact_ref(
        &registry.address,
        "snapshots",
        support::digest_of(&junk)?,
        ArtifactType::SbomV1,
    );
    let error = registry.client()?.attach_referrer(&subject, &referrer).await.err();
    assert!(matches!(error, Some(OciError::Layout(_))), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn referrer_operations_reject_malformed_registries() -> TestResult {
    let client = OciClient::new(OciClientConfig {
        docker_config: trogon_sandbox_oci::DockerConfigSource::Disabled,
        ..Default::default()
    });
    let (mut subject, _) = support::artifact("localhost:1", b"x")?;
    subject.registry = "bad host/with path".into();
    assert!(matches!(
        client.list_referrers(&subject).await.err(),
        Some(OciError::InvalidRegistry(_))
    ));
    Ok(())
}
