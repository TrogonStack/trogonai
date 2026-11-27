use super::*;
use trogon_sandbox_volumes::VolumeBackend;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn build_tar(files: &[(&str, &[u8])]) -> Result<Vec<u8>, std::io::Error> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, *content)?;
    }
    builder.into_inner()
}

fn local_config(root: &Path) -> VolumeClientConfig {
    VolumeClientConfig {
        backend: VolumeBackend::LocalFs {
            root: root.to_path_buf(),
        },
        kms_key_id: None,
    }
}

#[tokio::test]
async fn unpacks_tar_and_pushes_files_under_prefix() -> TestResult {
    let tar_bytes = build_tar(&[("a.txt", b"hello"), ("nested/b.txt", b"world")])?;
    let backend_root = tempfile::tempdir()?;
    let scratch = tempfile::tempdir()?;

    let summary = unpack_and_push_tar(
        &tar_bytes,
        scratch.path(),
        local_config(backend_root.path()),
        &VolumePrefix::new("lease-123")?,
    )
    .await?;

    assert_eq!(summary.objects, 2);
    assert_eq!(summary.bytes, 10);
    assert_eq!(std::fs::read(backend_root.path().join("lease-123/a.txt"))?, b"hello");
    assert_eq!(
        std::fs::read(backend_root.path().join("lease-123/nested/b.txt"))?,
        b"world"
    );
    Ok(())
}

#[tokio::test]
async fn empty_tar_pushes_zero_objects() -> TestResult {
    let tar_bytes = build_tar(&[])?;
    let backend_root = tempfile::tempdir()?;
    let scratch = tempfile::tempdir()?;

    let summary = unpack_and_push_tar(
        &tar_bytes,
        scratch.path(),
        local_config(backend_root.path()),
        &VolumePrefix::new("lease-empty")?,
    )
    .await?;

    assert_eq!(summary.objects, 0);
    assert_eq!(summary.bytes, 0);
    Ok(())
}

#[tokio::test]
async fn malformed_tar_bytes_surface_as_an_unpack_error() -> TestResult {
    let backend_root = tempfile::tempdir()?;
    let scratch = tempfile::tempdir()?;

    let result = unpack_and_push_tar(
        b"this is not a tar archive",
        scratch.path(),
        local_config(backend_root.path()),
        &VolumePrefix::new("lease-bad")?,
    )
    .await;

    assert!(matches!(result, Err(S3ExportError::Unpack(_))));
    Ok(())
}

#[test]
fn target_without_a_key_has_no_kms_key() -> TestResult {
    let target = S3ExportTarget::new(
        trogon_sandbox_contract::S3BucketName::new("exports-bucket")?,
        VolumePrefix::new("lease-1")?,
        trogon_sandbox_volumes::VolumeRegion::new("us-east-1")?,
        None,
    );
    assert!(target.client_config().kms_key_id.is_none());
    Ok(())
}
