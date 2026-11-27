//! Live CRIU checkpoint of a running Docker container, pushed to and pulled from a local OCI registry, then
//! restored into a second running container.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker --test live_checkpoint -- --ignored --nocapture
//! ```

mod docker_support;
mod zot_support;

use std::sync::Arc;
use std::time::Duration;

use docker_support::{TestResult, fresh_id};
use trogon_sandbox_contract::ArtifactType;
use trogon_sandbox_docker_worker::snapshot::{layer_is_checkpoint, pull_snapshot_layer, push_checkpoint_artifact};
use trogon_sandbox_docker_worker::{
    PROFILE, checkpoint_name_from_annotations, checkpoint_storage_dir, gzip_checkpoint, tar_checkpoint_dir,
    untar_checkpoint,
};
use trogon_sandbox_providers::{BollardEngine, DockerEngine, InspectStatus, LocalDockerProvider};
use zot_support::{Zot, sleeper_spec};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Docker experimental mode, CRIU, and zot; run with --ignored"]
async fn live_checkpoint_keeps_container_running() -> TestResult {
    let engine = Arc::new(BollardEngine::connect()?);
    if !engine.experimental_enabled().await? {
        return Err("enable the Docker daemon experimental mode to checkpoint a running container".into());
    }
    assert!(PROFILE.snapshot.can_checkpoint_running.is_supported());

    let zot = Zot::start().await?;
    let provider = LocalDockerProvider::new(
        Arc::clone(&engine) as Arc<dyn DockerEngine>,
        "sandbox-egress-proxyd:dev".to_string(),
    );

    let container_id = engine
        .create_and_start(sleeper_spec("live-chk", vec![]))
        .await?
        .container_id;
    let checkpoint_name = format!("trogon-chk-{}", fresh_id());
    let checkpoint_dir = checkpoint_storage_dir(&container_id, &checkpoint_name);
    let mut restored_id = None;

    let result = async {
        provider
            .checkpoint_running(&container_id, &checkpoint_name, &checkpoint_dir.to_string_lossy())
            .await?;
        assert_eq!(
            engine.inspect(&container_id).await?,
            InspectStatus::Running,
            "container must stay running after leave-running checkpoint"
        );

        let gz_tar = gzip_checkpoint(&tar_checkpoint_dir(&checkpoint_dir)?)?;
        let oci = zot.oci_client()?;
        let (artifact_ref, bytes) = push_checkpoint_artifact(
            &oci,
            &zot.registry,
            "snapshots/test/docker-live-checkpoint",
            gz_tar,
            &checkpoint_name,
        )
        .await?;
        assert!(bytes > 0);
        assert_eq!(artifact_ref.artifact_type, ArtifactType::SandboxRootfsV1);

        let pulled = pull_snapshot_layer(&oci, &artifact_ref).await?;
        assert!(layer_is_checkpoint(&pulled.annotations));
        assert_eq!(
            checkpoint_name_from_annotations(&pulled.annotations).as_deref(),
            Some(checkpoint_name.as_str())
        );

        let restore_dir = tempfile::tempdir()?;
        untar_checkpoint(&pulled.data, restore_dir.path())?;
        let restored = provider
            .restore_from_checkpoint(
                sleeper_spec("live-chk-restored", vec![]),
                &checkpoint_name,
                &restore_dir.path().to_string_lossy(),
            )
            .await?;
        restored_id = Some(restored.clone());

        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(engine.inspect(&restored).await?, InspectStatus::Running);
        TestResult::Ok(())
    }
    .await;

    let _ = engine.kill(&container_id).await;
    if let Some(restored) = &restored_id {
        let _ = engine.kill(restored).await;
    }
    let _ = tokio::fs::remove_dir_all(&checkpoint_dir).await;
    result
}
