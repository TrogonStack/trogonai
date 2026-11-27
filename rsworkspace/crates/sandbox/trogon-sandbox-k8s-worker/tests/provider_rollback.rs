mod support;

use support::*;
use trogon_sandbox_contract::{ArtifactType, LeaseError, SandboxRollbackResponse, SnapshotPushTarget};
use trogon_sandbox_k8s_worker::K8sWorkerError;
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_providers::K8sMicroVmConfig;

fn identified() -> K8sMicroVmConfig {
    K8sMicroVmConfig::new(NAMESPACE).with_worker_identity("worker-0")
}

fn denied_with(response: &SandboxRollbackResponse, text: &str) -> bool {
    matches!(response, SandboxRollbackResponse::Denied(LeaseError::Backend { message }) if message.contains(text))
}

#[tokio::test]
async fn rollback_rebuilds_the_sandbox_from_a_snapshot() -> TestResult {
    let world = World::persisting(identified()).await?;
    let lease = lease_id("lease-rollback");
    let artifact = seed_snapshot(&world.registry, "snapshots/r", b"rootfs").await?;

    let unknown = world.provider.rollback_for_lease(&lease, &artifact).await;
    assert!(matches!(
        unknown,
        SandboxRollbackResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));

    world.provider.lease(&lease_request()?, &lease).await?;
    let missing = trogon_sandbox_contract::OciArtifactRef {
        digest: trogon_sandbox_contract::Digest::new(format!("sha256:{}", "0".repeat(64)))?,
        ..artifact.clone()
    };
    let refused = world.provider.rollback_for_lease(&lease, &missing).await;
    assert!(denied_with(&refused, "not restorable"), "{refused:?}");
    assert_eq!(world.cluster.state().jobs_created, 1);

    let done = world.provider.rollback_for_lease(&lease, &artifact).await;
    assert_eq!(done, SandboxRollbackResponse::Done);
    assert_eq!(world.cluster.state().jobs_created, 2);
    assert!(world.provider.holds_lease(&lease).await);

    world.cluster.state().fail_deletes = 100;
    let stuck = world.provider.rollback_for_lease(&lease, &artifact).await;
    assert!(denied_with(&stuck, "could not remove"), "{stuck:?}");

    {
        let mut state = world.cluster.state();
        state.fail_deletes = 0;
        state.fail_creates.push("jobs".into());
    }
    let failed = world.provider.rollback_for_lease(&lease, &artifact).await;
    assert!(denied_with(&failed, "after the old sandbox was removed"), "{failed:?}");
    assert!(world.provider.holds_lease(&lease).await);

    world.cluster.state().fail_creates.clear();
    let retried = world.provider.rollback_for_lease(&lease, &artifact).await;
    assert_eq!(retried, SandboxRollbackResponse::Done);
    Ok(())
}

#[tokio::test]
async fn rollback_needs_the_stored_request_after_a_restart() -> TestResult {
    let world = World::persisting(identified()).await?;
    let lease = lease_id("lease-restart");
    world.provider.lease(&lease_request()?, &lease).await?;
    let artifact = seed_snapshot(&world.registry, "snapshots/r", b"rootfs").await?;

    let remembering = world.restarted(identified(), true).await?;
    let recovery = remembering.recover_leases().await?;
    assert_eq!((recovery.cold, recovery.warm, recovery.swept), (1, 0, 0));
    assert_eq!(
        remembering.rollback_for_lease(&lease, &artifact).await,
        SandboxRollbackResponse::Done
    );

    let plain = World::with(identified(), |_| None).await?;
    plain.provider.lease(&lease_request()?, &lease).await?;
    let forgetful = plain.restarted(identified(), false).await?;
    assert_eq!(forgetful.recover_leases().await?.cold, 1);
    let response = forgetful.rollback_for_lease(&lease, &artifact).await;
    assert!(denied_with(&response, "no copy of the request"), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn checkpoint_pushes_the_workspace_and_remembers_it() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-checkpoint");
    let target = SnapshotPushTarget {
        registry: world.registry.address.clone(),
        repository: "snapshots/c".into(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    let unknown = world.provider.checkpoint_for_lease(&lease, &target).await;
    assert!(matches!(unknown, Err(K8sWorkerError::NoActiveLease(_))));
    world.provider.lease(&lease_request()?, &lease).await?;
    world.cluster.script(ExecScript::new("tar").stdout(b"archive-bytes"));
    let receipt = world.provider.checkpoint_for_lease(&lease, &target).await?;
    assert_eq!(receipt.bytes, 13);
    assert!(world.provider.digest_cache.contains(&receipt.digest));
    assert!(world.registry.manifest_count() > 0);

    let reference = trogon_sandbox_contract::OciArtifactRef {
        registry: target.registry.clone(),
        repository: target.repository.clone(),
        digest: receipt.digest.clone(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    let through_trait = world.provider.snapshot(&lease, &reference).await?;
    assert_eq!(through_trait.digest, receipt.digest);

    world.registry.shutdown();
    assert!(world.provider.checkpoint_for_lease(&lease, &target).await.is_err());
    Ok(())
}
