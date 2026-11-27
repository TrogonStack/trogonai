//! Snapshot push/restore round-trip via a local OCI registry and bollard.

mod docker_support;
mod zot_support;

use std::sync::Arc;
use std::time::Duration;

use docker_support::{TestResult, fresh_id};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_docker_worker::snapshot::{gunzip_tar, gzip_tar, pull_rootfs_layer, push_rootfs_artifact};
use trogon_sandbox_providers::{BollardEngine, DockerEngine, InspectStatus, LocalDockerProvider};
use zot_support::{Zot, sleeper_spec};

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_roundtrip() -> TestResult {
    let zot = Zot::start().await?;
    let oci = zot.oci_client()?;

    let source_engine = Arc::new(BollardEngine::connect()?);
    let provider = LocalDockerProvider::new(
        Arc::clone(&source_engine) as Arc<dyn DockerEngine>,
        "sandbox-egress-proxyd:dev".to_string(),
    );

    // The containers are auto-removed, so they need a command that keeps
    // running: alpine's default shell exits at once, and an imported rootfs
    // has no command at all.
    let container_id = source_engine
        .create_and_start(sleeper_spec(
            "snap-roundtrip",
            vec!["SNAP_MARKER=roundtrip".to_string()],
        ))
        .await?
        .container_id;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let raw_tar = provider.commit_and_export_rootfs(&container_id).await;
    let _ = source_engine.kill(&container_id).await;
    let raw_tar = raw_tar?;
    assert!(!raw_tar.is_empty(), "exported rootfs should not be empty");

    let gz_tar = gzip_tar(&raw_tar)?;
    let (artifact_ref, bytes) =
        push_rootfs_artifact(&oci, &zot.registry, "snapshots/test/docker-roundtrip", gz_tar).await?;
    assert!(bytes > 0);

    let pulled_gz = pull_rootfs_layer(&oci, &artifact_ref).await?;
    let restored_tar = gunzip_tar(&pulled_gz)?;
    assert_eq!(restored_tar.len(), raw_tar.len());

    let restore_engine = Arc::new(BollardEngine::connect()?);
    let restore_provider = LocalDockerProvider::new(
        Arc::clone(&restore_engine) as Arc<dyn DockerEngine>,
        "sandbox-egress-proxyd:dev".to_string(),
    );
    let tag = format!("restored-{}-{}", fresh_id().to_lowercase(), artifact_ref.digest.hex());
    let restored_image = restore_provider
        .import_rootfs_image(&restored_tar, &format!("trogon-sandbox-restore:{tag}"))
        .await?;

    let mut restored_spec = sleeper_spec("snap-restored", vec![]);
    restored_spec.image = restored_image;
    let roundtrip_container = restore_engine.create_and_start(restored_spec).await?.container_id;

    tokio::time::sleep(Duration::from_secs(1)).await;
    let status = restore_engine.inspect(&roundtrip_container).await;
    let _ = restore_engine.kill(&roundtrip_container).await;
    assert_eq!(status?, InspectStatus::Running);
    assert_eq!(artifact_ref.artifact_type, ArtifactType::SandboxRootfsV1);
    Ok(())
}
