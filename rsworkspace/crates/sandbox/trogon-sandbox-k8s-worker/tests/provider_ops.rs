mod support;

use support::*;
use trogon_sandbox_contract::{ArtifactType, NetworkPolicy, SnapshotPushTarget};
use trogon_sandbox_k8s_worker::K8sWorkerError;
use trogon_sandbox_protocol::Provider;

#[tokio::test]
async fn ssh_keys_are_installed_and_removed_per_session() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-ssh");
    assert!(
        world
            .provider
            .ssh_attach_for_lease(&lease, "session-1", "ssh-ed25519 AAAA key")
            .await
            .is_none()
    );
    assert!(world.provider.ssh_detach_for_lease(&lease, "session-1").await.is_none());
    world.provider.lease(&lease_request()?, &lease).await?;

    world
        .provider
        .ssh_attach_for_lease(&lease, "session-1", "ssh-ed25519 AAAA it's")
        .await
        .ok_or("no lease")??;
    assert!(
        world
            .cluster
            .state()
            .execs
            .iter()
            .any(|command| command.contains("authorized_keys"))
    );
    world
        .provider
        .ssh_detach_for_lease(&lease, "session-1")
        .await
        .ok_or("no lease")??;
    // A second detach finds nothing to remove and still succeeds.
    world
        .provider
        .ssh_detach_for_lease(&lease, "session-1")
        .await
        .ok_or("no lease")??;

    world
        .cluster
        .script(ExecScript::new("authorized_keys").exit(1).stderr(b"read only"));
    let failed = world
        .provider
        .ssh_attach_for_lease(&lease, "session-2", "key")
        .await
        .ok_or("no lease")?;
    assert!(matches!(
        failed,
        Err(K8sWorkerError::CommandFailed {
            action: "ssh attach",
            ..
        })
    ));

    world.cluster.state().scripts.clear();
    world
        .provider
        .ssh_attach_for_lease(&lease, "session-3", "key")
        .await
        .ok_or("no lease")??;
    world.cluster.script(ExecScript::new("grep -vF").exit(1));
    let failed = world
        .provider
        .ssh_detach_for_lease(&lease, "session-3")
        .await
        .ok_or("no lease")?;
    assert!(matches!(
        failed,
        Err(K8sWorkerError::CommandFailed {
            action: "ssh detach",
            ..
        })
    ));

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("").refuse(503));
    assert!(
        world
            .provider
            .ssh_attach_for_lease(&lease, "session-4", "key")
            .await
            .ok_or("no lease")?
            .is_err()
    );
    world.provider.ssh_attach_for_lease(&lease, "session-4", "key").await;
    world.cluster.state().scripts.clear();
    world
        .provider
        .ssh_attach_for_lease(&lease, "session-5", "key")
        .await
        .ok_or("no lease")??;
    world.cluster.script(ExecScript::new("").refuse(503));
    assert!(
        world
            .provider
            .ssh_detach_for_lease(&lease, "session-5")
            .await
            .ok_or("no lease")?
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn the_network_policy_follows_pushed_updates() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-policy");
    let missing = world
        .provider
        .push_network_policy_for_lease(&lease, &NetworkPolicy::AllowAll)
        .await;
    assert!(matches!(missing, Err(K8sWorkerError::NoActiveLease(_))));
    world.provider.lease(&lease_request()?, &lease).await?;
    assert!(world.cluster.names("networkpolicies").is_empty());

    world
        .provider
        .push_network_policy_for_lease(&lease, &NetworkPolicy::DenyAll)
        .await?;
    assert_eq!(world.cluster.names("networkpolicies").len(), 1);
    world
        .provider
        .push_network_policy_for_lease(&lease, &NetworkPolicy::DenyAll)
        .await?;
    world
        .provider
        .push_network_policy_for_lease(&lease, &NetworkPolicy::AllowAll)
        .await?;
    assert!(world.cluster.names("networkpolicies").is_empty());
    world.provider.cancel(&lease).await?;
    Ok(())
}

#[tokio::test]
async fn checkpointing_an_unknown_lease_is_refused() -> TestResult {
    let world = World::start().await?;
    let target = SnapshotPushTarget {
        registry: "registry.example".into(),
        repository: "snapshots/one".into(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    };
    let refused = world.provider.checkpoint_for_lease(&lease_id("absent"), &target).await;
    assert!(matches!(refused, Err(K8sWorkerError::NoActiveLease(_))));
    Ok(())
}
