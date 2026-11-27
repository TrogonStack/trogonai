mod support;

use std::sync::Arc;

use support::*;
use trogon_sandbox_contract::{TerminalRequest, TerminalResponse};
use trogon_sandbox_k8s_worker::{K8sAuthorityHost, K8sTerminals};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, FreezeOutcome, HeldLease, Provider, PurgeOutcome, RedispatchGuard, ThawOutcome,
};
use trogon_sandbox_providers::K8sMicroVmConfig;

fn identified() -> K8sMicroVmConfig {
    K8sMicroVmConfig::new(NAMESPACE).with_worker_identity("worker-0")
}

#[tokio::test]
async fn the_authority_inspects_and_purges_the_leases_the_worker_holds() -> TestResult {
    let world = World::start().await?;
    let provider = world.restarted(identified(), false).await?;
    let lease = lease_id("lease-hosted");
    provider.lease(&lease_request()?, &lease).await?;

    let terminals = Arc::new(K8sTerminals::new(provider.clone(), world.nats.client().await, 4096));
    for (terminal, op) in [("term-1", "open"), ("term-1", "close"), ("term-2", "open")] {
        let reply = terminals
            .handle_request(&serde_json::from_value::<TerminalRequest>(serde_json::json!({
                "lease_id": "lease-hosted", "terminal_id": terminal, "op": op, "cols": 80, "rows": 24,
            }))?)
            .await;
        assert!(!matches!(reply, TerminalResponse::Failed(_)), "{op}: {reply:?}");
    }
    assert!(
        world
            .cluster
            .state()
            .execs
            .iter()
            .any(|command| command.contains("kill -HUP")),
        "closing a terminal hangs up its shell in the pod"
    );

    let host = K8sAuthorityHost::new(provider.clone(), Arc::new(RedispatchGuard::new()), terminals.clone());
    let [held]: [HeldLease; 1] = host.held_leases().await?.try_into().map_err(|_| "one lease expected")?;
    assert_eq!(host.freeze(&held).await?, FreezeOutcome::Unsupported);
    assert_eq!(host.thaw(&held).await?, ThawOutcome::NoLongerOwned);

    assert_eq!(host.purge(&held).await?, PurgeOutcome::Purged);
    assert!(!provider.holds_lease(&lease).await);
    assert_eq!(terminals.open_count(&lease), 0);
    Ok(())
}

#[tokio::test]
async fn a_cluster_that_cannot_be_changed_fails_the_authority_calls() -> TestResult {
    let world = World::start().await?;
    let provider = world.restarted(identified(), false).await?;
    let lease = lease_id("lease-stuck");
    provider.lease(&lease_request()?, &lease).await?;
    let terminals = Arc::new(K8sTerminals::new(provider.clone(), world.nats.client().await, 4096));
    let host = K8sAuthorityHost::new(provider.clone(), Arc::new(RedispatchGuard::new()), terminals);
    let [held]: [HeldLease; 1] = host.held_leases().await?.try_into().map_err(|_| "one lease expected")?;

    world.cluster.state().fail_deletes = 100;
    let failed = host.purge(&held).await;
    assert!(matches!(failed, Err(AuthorityHostError::Failed(_))), "{failed:?}");
    assert!(provider.holds_lease(&lease).await);
    Ok(())
}
