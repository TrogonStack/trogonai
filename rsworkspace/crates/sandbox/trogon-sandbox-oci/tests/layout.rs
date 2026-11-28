mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::{FakeRegistry, TestResult, push_artifact, sha256};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_oci::layout::{export_to_directory, export_to_tarball, import_from_directory};
use trogon_sandbox_oci::{OciClient, OciError};

fn blob_file(dir: &Path, digest: &str) -> std::path::PathBuf {
    dir.join("blobs/sha256").join(digest.trim_start_matches("sha256:"))
}

fn read_index(dir: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&std::fs::read(dir.join("index.json"))?)?)
}

fn write_index(dir: &Path, index: &Value) -> TestResult {
    std::fs::write(dir.join("index.json"), serde_json::to_vec(index)?)?;
    Ok(())
}

async fn exported(registry: &FakeRegistry) -> Result<(tempfile::TempDir, OciClient), Box<dyn std::error::Error>> {
    let client = registry.client()?;
    let subject = push_artifact(registry, "snapshots", ArtifactType::FirecrackerSnapshotV1, b"shared").await?;
    let sbom = push_artifact(registry, "snapshots", ArtifactType::SbomV1, b"shared").await?;
    client.attach_referrer(&subject, &sbom).await?;
    let dir = tempfile::tempdir()?;
    export_to_directory(&client, &subject, dir.path()).await?;
    Ok((dir, client))
}

#[tokio::test]
async fn directory_export_includes_referrers_and_shares_blobs() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let (dir, _) = exported(&registry).await?;
    let index = read_index(dir.path())?;
    assert_eq!(index["manifests"].as_array().map(Vec::len), Some(1));
    assert_eq!(index["annotations"]["org.opencontainers.image.ref.name"], "snapshots");
    assert_eq!(
        std::fs::read(dir.path().join("oci-layout"))?,
        br#"{"imageLayoutVersion":"1.0.0"}"#
    );
    assert_eq!(std::fs::read(blob_file(dir.path(), &sha256(b"shared")))?, b"shared");
    // subject manifest, referrer manifest, shared layer, shared config
    assert_eq!(std::fs::read_dir(dir.path().join("blobs/sha256"))?.count(), 4);
    Ok(())
}

#[tokio::test]
async fn tarball_export_contains_the_layout() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let subject = push_artifact(&registry, "snapshots", ArtifactType::SbomV1, b"payload").await?;
    let out = tempfile::tempdir()?;
    let tarball = out.path().join("layout.tar");
    export_to_tarball(&client, &subject, &tarball).await?;
    let mut archive = tar::Archive::new(std::fs::File::open(&tarball)?);
    let mut names = Vec::new();
    for entry in archive.entries()? {
        names.push(entry?.path()?.to_string_lossy().into_owned());
    }
    assert!(names.iter().any(|name| name == "index.json"));
    assert!(names.iter().any(|name| name == "oci-layout"));
    assert!(names.iter().any(|name| name.starts_with("blobs/sha256/")));
    assert!(
        export_to_tarball(&client, &subject, &out.path().join("missing/layout.tar"))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn an_exported_layout_imports_into_another_registry() -> TestResult {
    let source = FakeRegistry::start().await?;
    let (dir, _) = exported(&source).await?;
    let target = FakeRegistry::start().await?;
    let pushed = import_from_directory(&target.client()?, &target.host()?, dir.path()).await?;
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].repository, "snapshots");
    assert!(target.manifest("snapshots", pushed[0].digest.as_str()).is_some());

    let mut index = read_index(dir.path())?;
    index["annotations"] = Value::Null;
    write_index(dir.path(), &index)?;
    let unnamed = FakeRegistry::start().await?;
    let error = import_from_directory(&unnamed.client()?, &unnamed.host()?, dir.path())
        .await
        .err()
        .ok_or("accepted a layout with no repository")?;
    assert!(matches!(error, OciError::Layout(_)), "{error}");
    Ok(())
}

#[tokio::test]
async fn importing_a_damaged_layout_fails() -> TestResult {
    let source = FakeRegistry::start().await?;
    let (dir, _) = exported(&source).await?;
    let target = FakeRegistry::start().await?;
    let client = target.client()?;
    let host = target.host()?;
    let pristine = read_index(dir.path())?;
    let digest = pristine["manifests"][0]["digest"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut unknown = pristine.clone();
    unknown["manifests"][0]["artifactType"] = json!("fixture/unknown");
    write_index(dir.path(), &unknown)?;
    assert!(matches!(
        import_from_directory(&client, &host, dir.path()).await,
        Err(OciError::UnsupportedMediaType(_))
    ));

    let mut bad_digest = pristine.clone();
    bad_digest["manifests"][0]["digest"] = json!("bad");
    write_index(dir.path(), &bad_digest)?;
    assert!(import_from_directory(&client, &host, dir.path()).await.is_err());

    std::fs::write(dir.path().join("index.json"), b"not json")?;
    assert!(matches!(
        import_from_directory(&client, &host, dir.path()).await,
        Err(OciError::Layout(_))
    ));
    std::fs::remove_file(dir.path().join("index.json"))?;
    assert!(matches!(
        import_from_directory(&client, &host, dir.path()).await,
        Err(OciError::Io(_))
    ));
    write_index(dir.path(), &pristine)?;

    let manifest_path = blob_file(dir.path(), &digest);
    let manifest_raw = std::fs::read(&manifest_path)?;
    std::fs::write(&manifest_path, b"not json")?;
    assert!(matches!(
        import_from_directory(&client, &host, dir.path()).await,
        Err(OciError::Layout(_))
    ));

    let mut tampered: Value = serde_json::from_slice(&manifest_raw)?;
    tampered["annotations"] = json!({"extra": "value"});
    std::fs::write(&manifest_path, serde_json::to_vec(&tampered)?)?;
    assert!(matches!(
        import_from_directory(&client, &host, dir.path()).await,
        Err(OciError::DigestMismatch { .. })
    ));

    std::fs::write(&manifest_path, &manifest_raw)?;
    std::fs::write(blob_file(dir.path(), &sha256(b"shared")), b"tampered")?;
    assert!(import_from_directory(&client, &host, dir.path()).await.is_err());
    Ok(())
}

#[tokio::test]
async fn exporting_a_missing_artifact_fails() -> TestResult {
    let registry = FakeRegistry::start().await?;
    let client = registry.client()?;
    let (subject, _) = support::artifact(&registry.address, b"never pushed")?;
    let dir = tempfile::tempdir()?;
    assert!(export_to_directory(&client, &subject, dir.path()).await.is_err());
    Ok(())
}
