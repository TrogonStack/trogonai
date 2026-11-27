//! Startup sweep against a live Docker daemon: it removes only the containers of its own owner.

mod docker_support;

use std::sync::Arc;

use docker_support::{ALPINE_IMAGE, TestResult, fresh_id};
use trogon_sandbox_providers::{BollardEngine, ContainerRunSpec, DockerEngine, InspectStatus, LocalDockerProvider};

fn spec(provider: &LocalDockerProvider, role: &str) -> ContainerRunSpec {
    ContainerRunSpec {
        name: format!("orphan-sweep-{}", fresh_id()),
        image: ALPINE_IMAGE.to_string(),
        memory_mib: 64,
        cpu: 0.5,
        disk_mib: None,
        env: vec![],
        binds: vec![],
        network_mode: "none".to_string(),
        entrypoint_override: None,
        cmd_override: Some(vec!["sleep".to_string(), "600".to_string()]),
        working_dir: None,
        user: None,
        cap_add: vec![],
        cap_drop: vec![],
        exposed_ports: vec![],
        hardening: Default::default(),
        labels: provider.owner_labels(role, Some("lease-live")),
    }
}

#[tokio::test]
async fn sweep_removes_only_this_owners_containers() -> TestResult {
    let engine = Arc::new(BollardEngine::connect()?);
    let run = fresh_id();
    let mine = LocalDockerProvider::new(Arc::clone(&engine) as Arc<dyn DockerEngine>, "unused".to_string())
        .with_owner(format!("orphan-sweep-mine-{run}"));
    let other = LocalDockerProvider::new(Arc::clone(&engine) as Arc<dyn DockerEngine>, "unused".to_string())
        .with_owner(format!("orphan-sweep-other-{run}"));
    let unlabelled = LocalDockerProvider::new(Arc::clone(&engine) as Arc<dyn DockerEngine>, "unused".to_string());

    let started_mine = engine.create_and_start(spec(&mine, "workload")).await?.container_id;
    let started_other = engine.create_and_start(spec(&other, "workload")).await?.container_id;
    let started_plain = engine
        .create_and_start(spec(&unlabelled, "workload"))
        .await?
        .container_id;

    let removed = mine.sweep_owned().await;
    let mine_after = engine.inspect(&started_mine).await;
    let other_after = engine.inspect(&started_other).await;
    let plain_after = engine.inspect(&started_plain).await;

    // Remove only the ids recorded above, whatever the assertions say.
    for id in [&started_mine, &started_other, &started_plain] {
        let _ = engine.remove(id).await;
    }

    assert_eq!(removed?, 1);
    assert!(mine_after.is_err(), "owned container should be gone");
    assert_eq!(other_after?, InspectStatus::Running);
    assert_eq!(plain_after?, InspectStatus::Running);
    Ok(())
}
