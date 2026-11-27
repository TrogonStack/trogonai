mod support;

use support::*;
use trogon_sandbox_contract::{ArtifactType, SnapshotPushTarget};
use trogon_sandbox_k8s_worker::SnapshotTransferError;
use trogon_sandbox_k8s_worker::snapshot::{export_and_push_rootfs, pull_rootfs, tar_directory};
use trogon_sandbox_protocol::Provider;

fn entry_names(archive: &[u8]) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    for entry in tar::Archive::new(archive).entries()? {
        names.push(entry?.path()?.to_string_lossy().into_owned());
    }
    Ok(names)
}

#[test]
fn a_directory_is_archived_with_its_files() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("hello.txt"), b"hello")?;
    let names = entry_names(&tar_directory(dir.path())?)?;
    assert!(names.iter().any(|name| name.ends_with("hello.txt")), "{names:?}");
    Ok(())
}

#[test]
fn a_path_that_is_not_a_directory_gives_an_empty_archive() -> TestResult {
    let dir = tempfile::tempdir()?;
    assert!(entry_names(&tar_directory(&dir.path().join("absent"))?)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_jobs_workspace_is_exported_to_the_registry_and_pulled_back() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-export");
    world.provider.lease(&lease_request()?, &lease).await?;
    let [job] = world.cluster.names("jobs").try_into().map_err(|_| "one job expected")?;
    world.cluster.script(ExecScript::new("tar").stdout(b"archive-bytes"));
    let target = SnapshotPushTarget {
        registry: world.registry.address.clone(),
        repository: "snapshots/export".into(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    let oci = world.registry.client()?;
    let receipt = export_and_push_rootfs(&world.provider.inner, &oci, &job, &target).await?;
    assert_eq!(receipt.bytes, 13);

    let reference = trogon_sandbox_contract::OciArtifactRef {
        registry: target.registry.clone(),
        repository: target.repository.clone(),
        digest: receipt.digest,
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    assert_eq!(pull_rootfs(&oci, &reference).await?, b"archive-bytes");
    Ok(())
}

#[tokio::test]
async fn only_rootfs_snapshots_can_be_pulled() -> TestResult {
    let world = World::start().await?;
    let artifact = seed_snapshot(&world.registry, "snapshots/other", b"disk").await?;
    let other = trogon_sandbox_contract::OciArtifactRef {
        artifact_type: ArtifactType::KubevirtDiskV1,
        ..artifact
    };
    let refused = pull_rootfs(&world.registry.client()?, &other).await;
    assert!(matches!(
        refused,
        Err(SnapshotTransferError::UnsupportedArtifactType {
            found: ArtifactType::KubevirtDiskV1
        })
    ));
    Ok(())
}
