mod support;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use support::{FakeRegistry, TestResult, artifact_ref, digest_of, layer, limits, sha256};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::{ArtifactPullLimits, OciError, build_push_target};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn manifest(artifact_type: Option<&str>, layers: Vec<Value>) -> Result<Vec<u8>, serde_json::Error> {
    let mut value = json!({
        "schemaVersion": 2,
        "mediaType": support::OCI_MANIFEST,
        "config": {"mediaType": "application/vnd.oci.empty.v1+json", "digest": sha256(b"{}"), "size": 2},
        "layers": layers,
    });
    if let Some(artifact_type) = artifact_type {
        value["artifactType"] = json!(artifact_type);
    }
    serde_json::to_vec(&value)
}

fn entry(size: i64) -> Value {
    json!({"mediaType": "application/octet-stream", "digest": sha256(b"layer"), "size": size})
}

#[tokio::test]
async fn pushing_layers_that_do_not_match_the_target_digest_fails() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let layers = vec![layer(b"one")];
    let annotations = BTreeMap::new();
    let target = build_push_target(&registry.address, "repo", ArtifactType::SbomV1, &layers, &annotations)?;
    let error = registry
        .client()?
        .push_artifact(&target, vec![layer(b"two")], annotations)
        .await
        .err();
    assert!(matches!(error, Some(OciError::DigestMismatch { .. })), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn bounded_pulls_enforce_manifest_budgets() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let sbom = ArtifactType::SbomV1.media_type();
    let unlimited = ArtifactPullLimits {
        max_layers: 10,
        max_layer_bytes: u64::MAX,
        max_total_bytes: u64::MAX,
        ..limits()
    };
    let cases: Vec<(&str, Vec<u8>, ArtifactPullLimits)> = vec![
        (
            "count",
            manifest(Some(sbom), vec![entry(1), entry(1)])?,
            ArtifactPullLimits {
                max_layers: 1,
                ..unlimited
            },
        ),
        (
            "layer",
            manifest(Some(sbom), vec![entry(100)])?,
            ArtifactPullLimits {
                max_layer_bytes: 50,
                ..unlimited
            },
        ),
        (
            "total",
            manifest(Some(sbom), vec![entry(40), entry(40)])?,
            ArtifactPullLimits {
                max_total_bytes: 50,
                ..unlimited
            },
        ),
        ("negative", manifest(Some(sbom), vec![entry(-1)])?, unlimited),
        (
            "overflow",
            manifest(Some(sbom), vec![entry(i64::MAX), entry(i64::MAX), entry(2)])?,
            unlimited,
        ),
        (
            "foreign",
            manifest(
                Some(sbom),
                vec![
                    json!({"mediaType": "x/y", "digest": sha256(b"layer"), "size": 1, "urls": ["https://fixture.invalid/x"]}),
                ],
            )?,
            unlimited,
        ),
        (
            "digest",
            manifest(
                Some(sbom),
                vec![json!({"mediaType": "x/y", "digest": "sha256:zz", "size": 1})],
            )?,
            unlimited,
        ),
        ("untyped", manifest(None, vec![])?, unlimited),
        (
            "other",
            manifest(Some(ArtifactType::ScanReportV1.media_type()), vec![])?,
            unlimited,
        ),
    ];
    for (name, raw, limits) in cases {
        registry.put_manifest("repo", &sha256(&raw), support::OCI_MANIFEST, &raw);
        let target = artifact_ref(&registry.address, "repo", digest_of(&raw)?, ArtifactType::SbomV1);
        assert!(client.pull_artifact_bounded(&target, limits).await.is_err(), "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn head_manifest_refuses_invalid_manifests() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let raw = b"not a manifest".to_vec();
    registry.put_manifest("repo", &sha256(&raw), support::OCI_MANIFEST, &raw);
    let target = artifact_ref(&registry.address, "repo", digest_of(&raw)?, ArtifactType::SbomV1);
    let error = registry.client()?.head_manifest(&target).await.err();
    assert!(matches!(error, Some(OciError::Layout(_))), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn pulls_reject_a_manifest_served_under_a_different_digest() -> TestResult {
    let server = MockServer::start().await;
    let registry = server.address().to_string();
    let (target, raw) = support::artifact(&registry, b"blob")?;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", support::OCI_MANIFEST)
                .insert_header("docker-content-digest", sha256(b"other").as_str())
                .set_body_bytes(raw),
        )
        .mount(&server)
        .await;
    let error = support::client_for(&registry)?.pull_artifact(&target).await.err();
    assert!(error.is_some(), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn pulls_and_heads_refuse_a_manifest_of_another_or_unknown_type() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let other = ArtifactType::SbomV1.media_type();
    for declared in [None, Some("application/vnd.example.unknown.v1")] {
        let raw = manifest(declared, vec![])?;
        registry.put_manifest("repo", &sha256(&raw), support::OCI_MANIFEST, &raw);
        let target = artifact_ref(&registry.address, "repo", digest_of(&raw)?, ArtifactType::SbomV1);
        let head = client.head_manifest(&target).await.err();
        assert!(
            matches!(head, Some(OciError::UnsupportedMediaType(_))),
            "{declared:?} {head:?}"
        );
        let pull = client.pull_artifact(&target).await.err();
        assert!(pull.is_some(), "{declared:?}");
    }
    let raw = manifest(Some(other), vec![])?;
    registry.put_manifest("repo", &sha256(&raw), support::OCI_MANIFEST, &raw);
    let mismatched = artifact_ref(
        &registry.address,
        "repo",
        digest_of(&raw)?,
        ArtifactType::FirecrackerSnapshotV1,
    );
    let error = client.head_manifest(&mismatched).await.err();
    assert!(matches!(error, Some(OciError::UnsupportedMediaType(_))), "{error:?}");
    let matched = artifact_ref(&registry.address, "repo", digest_of(&raw)?, ArtifactType::SbomV1);
    client.head_manifest(&matched).await?;
    Ok(())
}

#[tokio::test]
async fn head_refuses_a_negative_config_size() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let mut value: Value = serde_json::from_slice(&manifest(Some(ArtifactType::SbomV1.media_type()), vec![])?)?;
    value["config"]["size"] = json!(-1);
    let raw = serde_json::to_vec(&value)?;
    registry.put_manifest("repo", &sha256(&raw), support::OCI_MANIFEST, &raw);
    let target = artifact_ref(&registry.address, "repo", digest_of(&raw)?, ArtifactType::SbomV1);
    let error = registry.client()?.head_manifest(&target).await.err();
    assert!(matches!(error, Some(OciError::Layout(_))), "{error:?}");
    Ok(())
}
