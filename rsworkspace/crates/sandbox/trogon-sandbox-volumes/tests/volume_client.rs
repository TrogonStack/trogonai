use std::path::Path;

use bytes::Bytes;
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStoreExt, PutPayload};
use tempfile::tempdir;
use trogon_sandbox_contract::S3BucketName;
use trogon_sandbox_volumes::{
    KmsKeyId, StoreResourceName, VolumeBackend, VolumeClient, VolumeClientConfig, VolumeEndpoint, VolumeError,
    VolumeMetrics, VolumeMode, VolumePrefix, VolumeRegion, build_changed_manifest, compute_tree_digest,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn local_config(root: &Path) -> VolumeClientConfig {
    VolumeClientConfig {
        backend: VolumeBackend::LocalFs {
            root: root.to_path_buf(),
        },
        kms_key_id: None,
    }
}

async fn seed(root: &Path, prefix: &str) -> TestResult {
    let store = LocalFileSystem::new_with_prefix(root)?;
    let files: [(&str, &[u8]); 3] = [
        ("README.md", b"# demo"),
        ("src/main.rs", b"fn main() {}"),
        ("docs/guide.md", b"guide"),
    ];
    for (rel, body) in files {
        store
            .put(
                &ObjectPath::from(format!("{prefix}/{rel}")),
                PutPayload::from_bytes(Bytes::copy_from_slice(body)),
            )
            .await?;
    }
    Ok(())
}

#[test]
fn config_labels_name_the_backend() -> TestResult {
    let s3 = VolumeClientConfig {
        backend: VolumeBackend::S3 {
            bucket: S3BucketName::new("tenant-bucket")?,
            region: VolumeRegion::new("us-east-1")?,
            endpoint: None,
        },
        kms_key_id: None,
    };
    assert_eq!((s3.bucket_label().as_str(), s3.backend_name()), ("tenant-bucket", "s3"));

    let gcs = VolumeClientConfig {
        backend: VolumeBackend::Gcs {
            bucket: StoreResourceName::new("gcs-bucket")?,
        },
        kms_key_id: None,
    };
    assert_eq!((gcs.bucket_label().as_str(), gcs.backend_name()), ("gcs-bucket", "gcs"));

    let azure = VolumeClientConfig {
        backend: VolumeBackend::AzureBlob {
            container: StoreResourceName::new("box")?,
            account: StoreResourceName::new("acct")?,
        },
        kms_key_id: None,
    };
    assert_eq!((azure.bucket_label().as_str(), azure.backend_name()), ("box", "azure"));

    let local = local_config(Path::new("/tmp/vol"));
    assert_eq!(
        (local.bucket_label().as_str(), local.backend_name()),
        ("/tmp/vol", "local_fs")
    );
    Ok(())
}

#[test]
fn remote_backends_build_or_report_a_typed_error() -> TestResult {
    let configs = [
        VolumeBackend::S3 {
            bucket: S3BucketName::new("tenant-bucket")?,
            region: VolumeRegion::new("us-east-1")?,
            endpoint: Some(VolumeEndpoint::new("http://127.0.0.1:9000")?),
        },
        VolumeBackend::S3 {
            bucket: S3BucketName::new("tenant-bucket")?,
            region: VolumeRegion::new("us-east-1")?,
            endpoint: None,
        },
        VolumeBackend::Gcs {
            bucket: StoreResourceName::new("gcs-bucket")?,
        },
        VolumeBackend::AzureBlob {
            container: StoreResourceName::new("box")?,
            account: StoreResourceName::new("acct")?,
        },
    ];
    for backend in configs {
        let result = VolumeClient::new(VolumeClientConfig {
            backend,
            kms_key_id: Some(KmsKeyId::new("key-1")?),
        });
        match result {
            Ok(client) => assert_eq!(client.kms_key_id().map(KmsKeyId::as_str), Some("key-1")),
            Err(error) => assert!(matches!(error, VolumeError::StoreBuild { .. })),
        }
    }
    Ok(())
}

#[test]
fn local_root_that_is_a_file_is_an_io_error() -> TestResult {
    let dir = tempdir()?;
    let file = dir.path().join("file");
    std::fs::write(&file, b"x")?;
    let result = VolumeClient::new(local_config(&file.join("nested")));
    assert!(matches!(result, Err(VolumeError::Io(_))));
    Ok(())
}

#[tokio::test]
async fn accessors_report_construction_options() -> TestResult {
    let root = tempdir()?;
    let client = VolumeClient::with_options(
        local_config(root.path()),
        VolumeMode::ReadWrite,
        VolumeMetrics::new()?,
        Some(10),
    )?;
    assert_eq!(client.mode(), VolumeMode::ReadWrite);
    assert_eq!(client.bucket(), root.path().to_string_lossy());
    assert!(client.kms_key_id().is_none());
    assert!(client.metrics().gather_text()?.is_empty());
    assert_eq!(
        VolumeClient::new(local_config(root.path()))?.mode(),
        VolumeMode::ReadOnly
    );
    Ok(())
}

#[tokio::test]
async fn pull_downloads_nested_objects_and_records_metrics() -> TestResult {
    let root = tempdir()?;
    seed(root.path(), "base").await?;
    let client = VolumeClient::with_mode(local_config(root.path()), VolumeMode::ReadOnly)?;

    let dest = tempdir()?;
    let summary = client.pull_to_dir(&VolumePrefix::new("base")?, dest.path()).await?;
    assert_eq!((summary.objects, summary.bytes), (3, 23));
    assert_eq!(std::fs::read(dest.path().join("src/main.rs"))?, b"fn main() {}");

    let text = client.metrics().gather_text()?;
    assert!(text.contains("trogon_sandbox_volumes_bytes_in_total"));
    assert!(text.contains("trogon_sandbox_volumes_pull_duration_seconds"));
    Ok(())
}

#[tokio::test]
async fn pull_from_the_root_prefix_lists_everything() -> TestResult {
    let root = tempdir()?;
    seed(root.path(), "base").await?;
    let client = VolumeClient::new(local_config(root.path()))?;
    let dest = tempdir()?;
    let summary = client.pull_to_dir(&VolumePrefix::root(), dest.path()).await?;
    assert_eq!(summary.objects, 3);
    assert!(dest.path().join("base/README.md").exists());
    Ok(())
}

#[tokio::test]
async fn pull_of_an_empty_prefix_downloads_nothing() -> TestResult {
    let root = tempdir()?;
    let client = VolumeClient::new(local_config(root.path()))?;
    let dest = tempdir()?;
    let summary = client.pull_to_dir(&VolumePrefix::new("absent")?, dest.path()).await?;
    assert_eq!((summary.objects, summary.bytes), (0, 0));
    Ok(())
}

#[tokio::test]
async fn push_uploads_only_manifest_entries() -> TestResult {
    let root = tempdir()?;
    let client = VolumeClient::with_mode(local_config(root.path()), VolumeMode::ReadWrite)?;
    let src = tempdir()?;
    std::fs::create_dir_all(src.path().join("src"))?;
    std::fs::write(src.path().join("README.md"), b"# changed")?;
    std::fs::write(src.path().join("src/main.rs"), b"fn main() {}")?;

    let mut manifest = build_changed_manifest(src.path(), None)?;
    manifest.entries.retain(|e| e.key.as_str() == "README.md");
    let summary = client
        .push_changed(&VolumePrefix::new("uploads/demo")?, src.path(), manifest)
        .await?;
    assert_eq!((summary.objects, summary.bytes), (1, 9));
    assert!(root.path().join("uploads/demo/README.md").exists());
    assert!(!root.path().join("uploads/demo/src/main.rs").exists());
    assert!(client.metrics().gather_text()?.contains("bytes_out_total"));
    Ok(())
}

#[tokio::test]
async fn push_to_the_root_prefix_uses_bare_keys() -> TestResult {
    let root = tempdir()?;
    let client = VolumeClient::with_mode(local_config(root.path()), VolumeMode::ReadWrite)?;
    let src = tempdir()?;
    std::fs::write(src.path().join("a.txt"), b"a")?;
    let manifest = build_changed_manifest(src.path(), None)?;
    client.push_changed(&VolumePrefix::root(), src.path(), manifest).await?;
    assert!(root.path().join("a.txt").exists());
    Ok(())
}

#[tokio::test]
async fn push_enforces_the_quota() -> TestResult {
    let root = tempdir()?;
    let client = VolumeClient::with_options(
        local_config(root.path()),
        VolumeMode::ReadWrite,
        VolumeMetrics::new()?,
        Some(4),
    )?;
    let src = tempdir()?;
    std::fs::write(src.path().join("big.txt"), b"too large")?;
    let manifest = build_changed_manifest(src.path(), None)?;
    let result = client
        .push_changed(&VolumePrefix::new("p")?, src.path(), manifest)
        .await;
    assert!(matches!(result, Err(VolumeError::QuotaExceeded { limit_bytes: 4, .. })));
    Ok(())
}

#[tokio::test]
async fn push_of_a_missing_file_is_an_io_error() -> TestResult {
    let root = tempdir()?;
    let client = VolumeClient::with_mode(local_config(root.path()), VolumeMode::ReadWrite)?;
    let src = tempdir()?;
    std::fs::write(src.path().join("a.txt"), b"a")?;
    let manifest = build_changed_manifest(src.path(), None)?;
    std::fs::remove_file(src.path().join("a.txt"))?;
    let result = client
        .push_changed(&VolumePrefix::new("p")?, src.path(), manifest)
        .await;
    assert!(matches!(result, Err(VolumeError::Io(_))));
    Ok(())
}

#[tokio::test]
async fn head_exists_reflects_listed_objects() -> TestResult {
    let root = tempdir()?;
    seed(root.path(), "base").await?;
    let client = VolumeClient::new(local_config(root.path()))?;
    assert!(client.head_exists(&VolumePrefix::new("base")?).await?);
    assert!(!client.head_exists(&VolumePrefix::new("other")?).await?);
    Ok(())
}

#[tokio::test]
async fn cache_hits_are_counted_per_bucket() -> TestResult {
    let root = tempdir()?;
    let client = VolumeClient::new(local_config(root.path()))?;
    client.record_cache_hit();
    assert!(
        client
            .metrics()
            .gather_text()?
            .contains("trogon_sandbox_volumes_cache_hit_total")
    );
    Ok(())
}

#[tokio::test]
async fn copy_on_write_uploads_the_delta_and_returns_its_digest() -> TestResult {
    let root = tempdir()?;
    seed(root.path(), "base").await?;
    let client = VolumeClient::with_mode(local_config(root.path()), VolumeMode::CopyOnWrite)?;
    let dest = tempdir()?;

    let handle = client
        .cow_setup(
            &VolumePrefix::new("base")?,
            &VolumePrefix::new("leases/one")?,
            dest.path(),
        )
        .await?;
    assert_eq!(handle.base_prefix.as_str(), "base");
    assert_eq!(handle.lease_prefix.as_str(), "leases/one");
    assert!(handle.delta_dir.is_dir());
    assert!(dest.path().join("README.md").exists());

    std::fs::write(handle.delta_dir.join("delta.txt"), b"lease delta")?;
    let expected = build_changed_manifest(&handle.delta_dir, None)?.digest()?;
    let digest = client.cow_finalize(handle).await?;
    assert_eq!(digest, expected);
    assert!(root.path().join("leases/one/delta.txt").exists());
    Ok(())
}

#[tokio::test]
async fn copy_on_write_without_changes_digests_the_materialized_tree() -> TestResult {
    let root = tempdir()?;
    seed(root.path(), "base").await?;
    let client = VolumeClient::with_mode(local_config(root.path()), VolumeMode::CopyOnWrite)?;
    let dest = tempdir()?;
    let handle = client
        .cow_setup(
            &VolumePrefix::new("base")?,
            &VolumePrefix::new("leases/two")?,
            dest.path(),
        )
        .await?;
    let digest = client.cow_finalize(handle).await?;
    assert_eq!(digest, compute_tree_digest(dest.path())?);
    assert!(!root.path().join("leases/two").exists());
    Ok(())
}

#[test]
fn errors_render_their_context() -> TestResult {
    let quota = VolumeError::QuotaExceeded {
        bucket: "b".to_owned(),
        limit_bytes: 1,
    };
    assert_eq!(quota.to_string(), "quota exceeded for bucket b: limit 1 bytes");
    assert_eq!(
        VolumeError::FuseUnsupported.to_string(),
        "FUSE mounts are not supported on this platform"
    );
    let source = object_store::Error::NotImplemented {
        operation: "x".to_owned(),
        implementer: "y".to_owned(),
    };
    assert!(VolumeError::Backend(source).to_string().starts_with("object store:"));
    Ok(())
}
