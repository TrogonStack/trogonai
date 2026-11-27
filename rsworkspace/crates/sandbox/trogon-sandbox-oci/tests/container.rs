mod support;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use support::{FakeRegistry, OCI_INDEX, OCI_MANIFEST, TestResult, amd64_config, image_ref, put_image, sha256};
use trogon_sandbox_contract::{Digest, OciImageRef};
use trogon_sandbox_oci::{ContainerPullLimits, OciError, ResolvedContainerImage};

const GZIP_LAYER: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
const REPOSITORY: &str = "acme/app";

fn is_config_error<T: std::fmt::Debug>(result: Result<T, OciError>) -> bool {
    matches!(result, Err(OciError::Config(_)))
}

fn raw_manifest(
    config_digest: &str,
    config_size: i64,
    layers: Vec<Value>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST,
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config_digest, "size": config_size},
        "layers": layers,
    }))?)
}

fn layer_entry(digest: &str, size: i64) -> Value {
    json!({"mediaType": GZIP_LAYER, "digest": digest, "size": size})
}

fn platform_entry(digest: &str, size: usize, os: &str, architecture: &str) -> Value {
    json!({"mediaType": OCI_MANIFEST, "digest": digest, "size": size,
        "platform": {"os": os, "architecture": architecture}})
}

fn index_bytes(entries: Vec<Value>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(serde_json::to_vec(
        &json!({"schemaVersion": 2, "mediaType": OCI_INDEX, "manifests": entries}),
    )?)
}

async fn resolve(registry: &FakeRegistry, tag: &str) -> Result<ResolvedContainerImage, OciError> {
    registry
        .client()
        .map_err(|e| OciError::Config(e.to_string()))?
        .resolve_container_image(&image_ref(&registry.address, tag))
        .await
}

#[tokio::test]
async fn resolves_the_image_configuration() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let layer = b"layer-bytes";
    let (digest, raw) = put_image(&registry, REPOSITORY, "v1", &amd64_config(), &[(GZIP_LAYER, layer)])?;
    let resolved = resolve(&registry, "v1").await?;
    assert_eq!(resolved.digest().as_str(), digest);
    assert_eq!(resolved.reference().tag, None);
    assert_eq!(
        resolved.reference().digest.as_ref().map(Digest::as_str),
        Some(digest.as_str())
    );
    assert_eq!(resolved.env().get("PATH").map(String::as_str), Some("/bin"));
    assert_eq!(resolved.env().get("EMPTY").map(String::as_str), Some(""));
    assert_eq!(resolved.working_dir(), Some("/work"));
    assert_eq!(resolved.user(), Some("app"));
    let config_len = serde_json::to_vec(&amd64_config())?.len();
    assert_eq!(resolved.compressed_bytes(), (config_len + layer.len()) as u64);
    assert!(!raw.is_empty());

    let by_digest = OciImageRef {
        digest: Some(Digest::new(&digest)?),
        tag: None,
        ..image_ref(&registry.address, "v1")
    };
    let again = registry.client()?.resolve_container_image(&by_digest).await?;
    assert_eq!(again.digest(), resolved.digest());
    let untagged = OciImageRef {
        tag: None,
        ..image_ref(&registry.address, "v1")
    };
    assert!(registry.client()?.resolve_container_image(&untagged).await.is_err());
    Ok(())
}

#[tokio::test]
async fn blank_working_dir_and_user_are_dropped() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let config = json!({"architecture": "amd64", "os": "linux", "config": {"WorkingDir": "", "User": ""},
        "rootfs": {"type": "layers", "diff_ids": []}});
    put_image(&registry, REPOSITORY, "blank", &config, &[])?;
    let resolved = resolve(&registry, "blank").await?;
    assert_eq!((resolved.working_dir(), resolved.user()), (None, None));
    assert!(resolved.env().is_empty());

    let bare = json!({"architecture": "amd64", "os": "linux", "rootfs": {"type": "layers", "diff_ids": []}});
    put_image(&registry, REPOSITORY, "bare", &bare, &[])?;
    assert!(resolve(&registry, "bare").await?.env().is_empty());
    Ok(())
}

#[tokio::test]
async fn rejects_unsuitable_image_configurations() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let rootfs = json!({"type": "layers", "diff_ids": []});
    let cases = [
        ("arm", json!({"architecture": "arm64", "os": "linux", "rootfs": rootfs})),
        (
            "noeq",
            json!({"architecture": "amd64", "os": "linux", "config": {"Env": ["NOEQUALS"]}, "rootfs": rootfs}),
        ),
        (
            "noname",
            json!({"architecture": "amd64", "os": "linux", "config": {"Env": ["=value"]}, "rootfs": rootfs}),
        ),
        (
            "relative",
            json!({"architecture": "amd64", "os": "linux", "config": {"WorkingDir": "rel"}, "rootfs": rootfs}),
        ),
        ("junk", json!({"architecture": 5})),
    ];
    for (tag, config) in cases {
        put_image(&registry, REPOSITORY, tag, &config, &[])?;
        assert!(is_config_error(resolve(&registry, tag).await), "{tag}");
    }
    Ok(())
}

#[tokio::test]
async fn rejects_a_corrupt_config_blob() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let config_bytes = serde_json::to_vec(&amd64_config())?;
    let config_digest = registry.put_blob(&config_bytes);
    let raw = raw_manifest(&config_digest, config_bytes.len() as i64, vec![])?;
    registry.put_manifest(REPOSITORY, "corrupt", OCI_MANIFEST, &raw);
    registry.corrupt_blob(&config_digest, &vec![b'x'; config_bytes.len()]);
    assert!(resolve(&registry, "corrupt").await.is_err());
    Ok(())
}

#[tokio::test]
async fn enforces_manifest_limits() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let config_bytes = serde_json::to_vec(&amd64_config())?;
    let config_digest = registry.put_blob(&config_bytes);
    let config_size = config_bytes.len() as i64;
    let layer_digest = sha256(b"layer");
    let many: Vec<Value> = (0..513).map(|_| layer_entry(&layer_digest, 1)).collect();
    let foreign =
        json!({"mediaType": GZIP_LAYER, "digest": layer_digest, "size": 1, "urls": ["https://fixture.invalid/x"]});
    let cases: Vec<(&str, i64, Vec<Value>)> = vec![
        ("bigconfig", 2 * 1024 * 1024, vec![]),
        ("many", config_size, many),
        ("negative", config_size, vec![layer_entry(&layer_digest, -1)]),
        ("foreign", config_size, vec![foreign]),
        (
            "overflow",
            2,
            vec![
                layer_entry(&layer_digest, i64::MAX),
                layer_entry(&layer_digest, i64::MAX),
            ],
        ),
    ];
    let unlimited = ContainerPullLimits {
        max_blob_bytes: u64::MAX,
        max_total_bytes: u64::MAX,
    };
    for (tag, size, layers) in cases {
        let raw = raw_manifest(&config_digest, size, layers)?;
        registry.put_manifest(REPOSITORY, tag, OCI_MANIFEST, &raw);
        let result = registry
            .client()?
            .resolve_container_image_with_limits(&image_ref(&registry.address, tag), unlimited)
            .await;
        assert!(result.is_err(), "{tag}");
    }

    let raw = raw_manifest(&config_digest, config_size, vec![layer_entry(&layer_digest, 100)])?;
    registry.put_manifest(REPOSITORY, "sized", OCI_MANIFEST, &raw);
    let client = registry.client()?;
    let image = image_ref(&registry.address, "sized");
    let blob_limit = ContainerPullLimits {
        max_blob_bytes: 50,
        max_total_bytes: u64::MAX,
    };
    assert!(is_config_error(
        client.resolve_container_image_with_limits(&image, blob_limit).await
    ));
    let total_limit = ContainerPullLimits {
        max_blob_bytes: u64::MAX,
        max_total_bytes: 50,
    };
    assert!(is_config_error(
        client.resolve_container_image_with_limits(&image, total_limit).await
    ));
    Ok(())
}

#[tokio::test]
async fn rejects_an_unsupported_manifest_schema() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let raw = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "mediaType": OCI_MANIFEST,
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": sha256(b"c"), "size": 1},
        "layers": [],
    }))?;
    registry.put_manifest(REPOSITORY, "v0", OCI_MANIFEST, &raw);
    assert!(is_config_error(resolve(&registry, "v0").await));
    registry.put_manifest(REPOSITORY, "garbage", OCI_MANIFEST, b"not json");
    assert!(is_config_error(resolve(&registry, "garbage").await));
    Ok(())
}

#[tokio::test]
async fn selects_the_amd64_entry_of_an_index() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let (digest, raw) = put_image(&registry, REPOSITORY, "child", &amd64_config(), &[])?;
    let arm = platform_entry(&sha256(b"arm"), 3, "linux", "arm64");
    let amd = platform_entry(&digest, raw.len(), "linux", "amd64");
    registry.put_manifest(REPOSITORY, "multi", OCI_INDEX, &index_bytes(vec![arm.clone(), amd])?);
    assert_eq!(resolve(&registry, "multi").await?.digest().as_str(), digest);

    let bare = json!({"mediaType": OCI_MANIFEST, "digest": digest, "size": raw.len()});
    registry.put_manifest(REPOSITORY, "single", OCI_INDEX, &index_bytes(vec![bare])?);
    assert_eq!(resolve(&registry, "single").await?.digest().as_str(), digest);

    registry.put_manifest(REPOSITORY, "noamd", OCI_INDEX, &index_bytes(vec![arm])?);
    assert!(is_config_error(resolve(&registry, "noamd").await));
    registry.put_manifest(REPOSITORY, "empty", OCI_INDEX, &index_bytes(vec![])?);
    assert!(is_config_error(resolve(&registry, "empty").await));
    Ok(())
}

#[tokio::test]
async fn refuses_malformed_indexes() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let (digest, raw) = put_image(&registry, REPOSITORY, "child", &amd64_config(), &[])?;
    let good = platform_entry(&digest, raw.len(), "linux", "amd64");

    let wrong_size = platform_entry(&digest, raw.len() + 1, "linux", "amd64");
    registry.put_manifest(REPOSITORY, "wrongsize", OCI_INDEX, &index_bytes(vec![wrong_size])?);
    let too_big = platform_entry(&digest, 2 * 1024 * 1024, "linux", "amd64");
    registry.put_manifest(REPOSITORY, "toobig", OCI_INDEX, &index_bytes(vec![too_big])?);
    let negative = platform_entry(&digest, 0, "linux", "amd64");
    let negative = Value::Object(
        negative
            .as_object()
            .cloned()
            .map(|mut o| {
                o.insert("size".into(), json!(-1));
                o
            })
            .unwrap_or_default(),
    );
    registry.put_manifest(REPOSITORY, "negative", OCI_INDEX, &index_bytes(vec![negative])?);
    let mut with_urls = good.clone();
    with_urls["urls"] = json!(["https://fixture.invalid/x"]);
    registry.put_manifest(REPOSITORY, "urls", OCI_INDEX, &index_bytes(vec![with_urls])?);
    let mut null_urls = good.clone();
    null_urls["urls"] = Value::Null;
    registry.put_manifest(REPOSITORY, "nullurls", OCI_INDEX, &index_bytes(vec![null_urls])?);
    let entries: Vec<Value> = (0..513).map(|_| good.clone()).collect();
    registry.put_manifest(REPOSITORY, "entries", OCI_INDEX, &index_bytes(entries)?);
    let bad_digest = platform_entry("bad", 1, "linux", "amd64");
    registry.put_manifest(REPOSITORY, "baddigest", OCI_INDEX, &index_bytes(vec![bad_digest])?);

    for tag in ["wrongsize", "toobig", "negative", "urls", "entries", "baddigest"] {
        assert!(resolve(&registry, tag).await.is_err(), "{tag}");
    }
    // A null urls field is not a foreign URL.
    assert!(resolve(&registry, "nullurls").await.is_ok());

    let mut next = platform_entry(&sha256(b"leaf"), 1, "linux", "amd64");
    let mut last_digest = String::new();
    for level in 0..4 {
        let raw = index_bytes(vec![next.clone()])?;
        last_digest = sha256(&raw);
        registry.put_manifest(REPOSITORY, &format!("chain{level}"), OCI_INDEX, &raw);
        next = platform_entry(&last_digest, raw.len(), "linux", "amd64");
    }
    assert!(!last_digest.is_empty());
    assert!(is_config_error(resolve(&registry, "chain3").await));
    Ok(())
}

#[tokio::test]
async fn rejects_index_schema_versions_other_than_two() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let raw = serde_json::to_vec(&json!({"schemaVersion": 1, "mediaType": OCI_INDEX, "manifests": []}))?;
    registry.put_manifest(REPOSITORY, "old", OCI_INDEX, &raw);
    assert!(is_config_error(resolve(&registry, "old").await));
    Ok(())
}

async fn resolved_for(registry: &FakeRegistry, tag: &str) -> Result<ResolvedContainerImage, OciError> {
    resolve(registry, tag).await
}

#[tokio::test]
async fn exports_an_oci_layout_with_normalized_media_types() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let config_bytes = serde_json::to_vec(&amd64_config())?;
    let config_digest = registry.put_blob(&config_bytes);
    let gzip = b"gzip-layer";
    let plain = b"plain-layer";
    let other = b"other-layer";
    let layers = vec![
        json!({"mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip", "digest": registry.put_blob(gzip), "size": gzip.len()}),
        json!({"mediaType": "application/vnd.docker.image.rootfs.diff.tar", "digest": registry.put_blob(plain), "size": plain.len()}),
        json!({"mediaType": "application/x-custom", "digest": registry.put_blob(other), "size": other.len()}),
    ];
    let raw = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "config": {"mediaType": "application/vnd.docker.container.image.v1+json", "digest": config_digest, "size": config_bytes.len()},
        "layers": layers,
    }))?;
    registry.put_manifest(
        REPOSITORY,
        "docker",
        "application/vnd.docker.distribution.manifest.v2+json",
        &raw,
    );
    let resolved = resolved_for(&registry, "docker").await?;

    let dir = tempfile::tempdir()?;
    registry
        .client()?
        .export_container_layout(&resolved, dir.path())
        .await?;
    assert_eq!(
        std::fs::read(dir.path().join("oci-layout"))?,
        br#"{"imageLayoutVersion":"1.0.0"}"#
    );
    let index: Value = serde_json::from_slice(&std::fs::read(dir.path().join("index.json"))?)?;
    let digest = index["manifests"][0]["digest"].as_str().unwrap_or_default().to_string();
    let hex = digest.trim_start_matches("sha256:");
    let manifest: Value = serde_json::from_slice(&std::fs::read(dir.path().join("blobs/sha256").join(hex))?)?;
    assert_eq!(manifest["mediaType"], "application/vnd.oci.image.manifest.v1+json");
    assert_eq!(
        manifest["config"]["mediaType"],
        "application/vnd.oci.image.config.v1+json"
    );
    assert_eq!(manifest["layers"][0]["mediaType"], GZIP_LAYER);
    assert_eq!(
        manifest["layers"][1]["mediaType"],
        "application/vnd.oci.image.layer.v1.tar"
    );
    assert_eq!(manifest["layers"][2]["mediaType"], "application/x-custom");
    assert_eq!(
        std::fs::read(dir.path().join("blobs/sha256").join(Digest::new(sha256(gzip))?.hex()))?,
        gzip
    );

    let tiny = ContainerPullLimits {
        max_blob_bytes: 2,
        max_total_bytes: 2,
    };
    let other_dir = tempfile::tempdir()?;
    assert!(is_config_error(
        registry
            .client()?
            .export_container_layout_with_limits(&resolved, other_dir.path(), tiny)
            .await
    ));
    Ok(())
}

#[tokio::test]
async fn export_refuses_an_index_digest_and_invalid_blob_digests() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let (digest, raw) = put_image(&registry, REPOSITORY, "child", &amd64_config(), &[])?;
    let entry = platform_entry(&digest, raw.len(), "linux", "amd64");
    let index = index_bytes(vec![entry])?;
    registry.put_manifest(REPOSITORY, "idx", OCI_INDEX, &index);
    let index_digest = Digest::new(sha256(&index))?;
    let pinned = OciImageRef {
        digest: Some(index_digest.clone()),
        tag: None,
        ..image_ref(&registry.address, "idx")
    };
    let resolved = ResolvedContainerImage::new(pinned, index_digest, BTreeMap::new(), None, None, 0)?;
    let dir = tempfile::tempdir()?;
    let result = registry.client()?.export_container_layout(&resolved, dir.path()).await;
    assert!(matches!(result, Err(OciError::DigestMismatch { .. })), "{result:?}");

    let config_bytes = serde_json::to_vec(&amd64_config())?;
    let config_digest = registry.put_blob(&config_bytes);
    let broken = raw_manifest(
        &config_digest,
        config_bytes.len() as i64,
        vec![layer_entry("sha256:zz", 1)],
    )?;
    registry.put_manifest(REPOSITORY, "broken", OCI_MANIFEST, &broken);
    let broken_digest = Digest::new(sha256(&broken))?;
    let pinned = OciImageRef {
        digest: Some(broken_digest.clone()),
        tag: None,
        ..image_ref(&registry.address, "broken")
    };
    let resolved = ResolvedContainerImage::new(pinned, broken_digest, BTreeMap::new(), None, None, 0)?;
    assert!(
        registry
            .client()?
            .export_container_layout(&resolved, dir.path())
            .await
            .is_err()
    );
    Ok(())
}
