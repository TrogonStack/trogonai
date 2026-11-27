use std::collections::BTreeMap;

use trogon_sandbox_contract::{ArtifactType, Digest, OciImageRef};
use trogon_sandbox_oci::{
    ContainerPullLimits, OciMetrics, RegistryHost, RegistryHostError, ResolvedContainerImage,
    ResolvedContainerImageError, artifact_type_label, digest_from_staging_tag,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn digest(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", fill.to_string().repeat(64)))?)
}

fn pinned(fill: char) -> Result<OciImageRef, Box<dyn std::error::Error>> {
    Ok(OciImageRef {
        registry: "registry.example".into(),
        repository: "acme/app".into(),
        tag: None,
        digest: Some(digest(fill)?),
    })
}

#[test]
fn registry_hosts_are_parsed_from_host_and_port() -> TestResult {
    let host = RegistryHost::new("registry.example:5000")?;
    assert_eq!(host.as_str(), "registry.example:5000");
    assert_eq!(host.to_string(), "registry.example:5000");
    assert_eq!("localhost".parse::<RegistryHost>()?.as_str(), "localhost");
    let borrowed: &str = std::borrow::Borrow::borrow(&host);
    assert_eq!(borrowed, "registry.example:5000");
    assert_eq!(
        RegistryHost::new("user:pass@registry.example"),
        Err(RegistryHostError::Credentials)
    );
    for value in ["registry.example/path", "registry.example?x=1", "registry.example#frag"] {
        assert_eq!(
            RegistryHost::new(value),
            Err(RegistryHostError::ExtraComponents),
            "{value}"
        );
    }
    for value in ["", "bad host", "[::"] {
        assert_eq!(RegistryHost::new(value), Err(RegistryHostError::Malformed), "{value:?}");
    }
    assert!("bad host".parse::<RegistryHost>().is_err());
    Ok(())
}

#[test]
fn resolved_images_must_be_pinned_and_well_formed() -> TestResult {
    let ok = ResolvedContainerImage::new(
        pinned('a')?,
        digest('a')?,
        BTreeMap::from([("PATH".to_string(), "/bin".to_string())]),
        Some("/work".into()),
        Some("app".into()),
        42,
    )?;
    assert_eq!(ok.compressed_bytes(), 42);

    let tagged = OciImageRef {
        tag: Some("v1".into()),
        ..pinned('a')?
    };
    let new = |reference: OciImageRef, digest: Digest, env: BTreeMap<String, String>, dir: Option<String>| {
        ResolvedContainerImage::new(reference, digest, env, dir, None, 0)
    };
    let env = |name: &str, value: &str| BTreeMap::from([(name.to_string(), value.to_string())]);
    assert_eq!(
        new(tagged, digest('a')?, BTreeMap::new(), None),
        Err(ResolvedContainerImageError::NotPinned)
    );
    assert_eq!(
        new(pinned('a')?, digest('b')?, BTreeMap::new(), None),
        Err(ResolvedContainerImageError::NotPinned)
    );
    assert_eq!(
        new(pinned('a')?, digest('a')?, env("", "x"), None),
        Err(ResolvedContainerImageError::InvalidEnvName)
    );
    assert_eq!(
        new(pinned('a')?, digest('a')?, env("A\0", "x"), None),
        Err(ResolvedContainerImageError::InvalidEnvName)
    );
    assert_eq!(
        new(pinned('a')?, digest('a')?, env("A", "x\0"), None),
        Err(ResolvedContainerImageError::InvalidEnvValue)
    );
    assert_eq!(
        new(pinned('a')?, digest('a')?, BTreeMap::new(), Some("rel".into())),
        Err(ResolvedContainerImageError::InvalidWorkingDir)
    );
    assert_eq!(
        new(pinned('a')?, digest('a')?, BTreeMap::new(), Some("/a\0".into())),
        Err(ResolvedContainerImageError::InvalidWorkingDir)
    );
    for error in [
        ResolvedContainerImageError::NotPinned,
        ResolvedContainerImageError::InvalidEnvName,
        ResolvedContainerImageError::InvalidEnvValue,
        ResolvedContainerImageError::InvalidWorkingDir,
    ] {
        assert!(!error.to_string().is_empty());
    }
    Ok(())
}

#[test]
fn resolved_images_roundtrip_through_json_and_reject_invalid_input() -> TestResult {
    let image = ResolvedContainerImage::new(pinned('a')?, digest('a')?, BTreeMap::new(), None, None, 7)?;
    let json = serde_json::to_string(&image)?;
    let back: ResolvedContainerImage = serde_json::from_str(&json)?;
    assert_eq!(back, image);

    let mut value: serde_json::Value = serde_json::from_str(&json)?;
    value["digest"] = serde_json::json!(digest('b')?.as_str());
    assert!(serde_json::from_value::<ResolvedContainerImage>(value).is_err());
    Ok(())
}

#[test]
fn metrics_are_exposed_in_their_own_registry() -> TestResult {
    let metrics = OciMetrics::new()?;
    metrics
        .push_bytes_total
        .with_label_values(&["registry.example", "sbom_v1"])
        .inc_by(3);
    assert!(metrics.gather_text()?.contains("trogon_sandbox_oci_push_bytes_total"));
    assert!(!metrics.registry().gather().is_empty());
    Ok(())
}

#[test]
fn artifact_types_have_stable_labels() {
    let labels: Vec<&str> = [
        ArtifactType::SandboxRootfsV1,
        ArtifactType::HyperlightSnapshotV1,
        ArtifactType::FirecrackerSnapshotV1,
        ArtifactType::KubevirtDiskV1,
        ArtifactType::SbomV1,
        ArtifactType::ScanReportV1,
    ]
    .into_iter()
    .map(artifact_type_label)
    .collect();
    assert_eq!(
        labels,
        [
            "sandbox_rootfs_v1",
            "hyperlight_snapshot_v1",
            "firecracker_snapshot_v1",
            "kubevirt_disk_v1",
            "sbom_v1",
            "scan_report_v1"
        ]
    );
}

#[test]
fn staging_tags_map_back_to_their_digest() -> TestResult {
    let hex = "a".repeat(64);
    let parsed = digest_from_staging_tag(&format!("sandbox-artifact-sha256-{hex}"));
    assert_eq!(
        parsed.as_ref().map(Digest::as_str),
        Some(format!("sha256:{hex}").as_str())
    );
    assert!(digest_from_staging_tag("latest").is_none());
    assert!(digest_from_staging_tag("sandbox-artifact-md5-abc").is_none());
    assert!(digest_from_staging_tag("sandbox-artifact-sha256-short").is_none());
    Ok(())
}

#[test]
fn container_limits_default_to_one_gibibyte() {
    let limits = ContainerPullLimits::default();
    assert_eq!(limits.max_blob_bytes, 1024 * 1024 * 1024);
    assert_eq!(limits.max_total_bytes, 1024 * 1024 * 1024);
}
