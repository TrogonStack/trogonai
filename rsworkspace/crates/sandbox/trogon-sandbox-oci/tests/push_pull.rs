mod support;

use std::collections::BTreeMap;

use support::{FakeRegistry, TestResult};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::{Layer, OciMetrics, build_push_target};

fn layer(data: &[u8]) -> Layer {
    Layer {
        data: data.to_vec(),
        media_type: "application/octet-stream".into(),
        annotations: BTreeMap::new(),
    }
}

#[tokio::test]
async fn pushed_artifact_pulls_back_with_its_layers() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let layers = vec![layer(b"first"), layer(b"second")];
    let mut annotations = BTreeMap::new();
    annotations.insert("fixture".to_string(), "value".to_string());
    let target = build_push_target(
        &registry.address,
        "snapshots/acme",
        ArtifactType::FirecrackerSnapshotV1,
        &layers,
        &annotations,
    )?;

    let receipt = client
        .push_artifact(&target, layers.clone(), annotations.clone())
        .await?;
    assert_eq!(receipt.manifest_digest, target.digest);
    assert_eq!(receipt.reference, target);

    let pulled = client.pull_artifact(&target).await?;
    assert_eq!(pulled.layers.len(), 2);
    assert_eq!(pulled.layers[0].data, b"first");
    assert_eq!(pulled.layers[1].data, b"second");
    assert_eq!(pulled.annotations, annotations);
    assert_eq!(pulled.manifest.digest, target.digest);
    assert_eq!(pulled.manifest.artifact_type, ArtifactType::FirecrackerSnapshotV1);
    Ok(())
}

#[tokio::test]
async fn push_and_pull_record_metrics() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let metrics = OciMetrics::new()?;
    let client = trogon_sandbox_oci::OciClient::with_metrics(
        trogon_sandbox_oci::OciClientConfig {
            insecure_registries: vec![registry.host()?],
            ..Default::default()
        },
        Some(metrics.clone()),
    );
    let layers = vec![layer(b"payload")];
    let target = build_push_target(
        &registry.address,
        "metrics",
        ArtifactType::SandboxRootfsV1,
        &layers,
        &BTreeMap::new(),
    )?;
    client.push_artifact(&target, layers, BTreeMap::new()).await?;
    client.pull_artifact(&target).await?;
    let text = metrics.gather_text()?;
    assert!(text.contains("push_bytes_total"), "{text}");
    assert!(text.contains("pull_bytes_total"), "{text}");
    Ok(())
}
