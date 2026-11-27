//! What the provider does when the cluster changes under it: objects replaced behind its back, deletions that
//! wait on finalizers, replies without identity, and claims whose reply was lost.

mod fake_cluster;
mod provider_support;
mod support;

use fake_cluster::{Effect, Fault, Verb};
use provider_support::{Harness, lease, refused, registry_request, snapshot_request};
use serde_json::json;
use support::{TestResult, digest, reference, seed_warm_vm};
use trogon_sandbox_kubevirt_worker::KubeVirtProviderError;
use trogon_sandbox_protocol::{AuthorityHost, AuthorityHostError, Provider, PurgeOutcome};

#[tokio::test(start_paused = true)]
async fn cancel_never_deletes_objects_that_replaced_the_originals() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let before = harness.cluster.objects("virtualmachines");
    let disks = harness.cluster.names("datavolumes");
    harness.cluster.with_store(|store| {
        for name in store.names("virtualmachines") {
            store.recreate("virtualmachines", &name);
        }
        for name in &disks {
            store.recreate("datavolumes", name);
        }
    });
    let replaced = harness.cluster.objects("virtualmachines");
    assert_ne!(replaced, before);

    harness.provider.cancel(&lease()).await?;
    assert!(harness.provider.held_leases().await?.is_empty());
    assert_eq!(harness.cluster.objects("virtualmachines"), replaced);
    assert_eq!(harness.cluster.names("datavolumes"), disks);
    assert_eq!(harness.cluster.count_requests(Verb::Delete, "virtualmachines"), 0);
    assert_eq!(harness.cluster.count_requests(Verb::Delete, "datavolumes"), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_deletion_that_waits_on_finalizers_keeps_the_lease_held() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let held = harness.provider.held_leases().await?.pop().ok_or("not held")?;
    let vms = harness.cluster.names("virtualmachines");
    for name in &vms {
        harness.cluster.update("virtualmachines", name, |vm| {
            vm["metadata"]["finalizers"] = json!(["test/hold"])
        });
    }

    let error = AuthorityHost::purge(&harness.provider, &held)
        .await
        .err()
        .ok_or("purged")?;
    assert!(matches!(error, AuthorityHostError::Failed(_)));
    assert_eq!(harness.cluster.names("virtualmachines"), vms);
    assert_eq!(harness.provider.held_leases().await?, vec![held.clone()]);

    for name in &vms {
        harness
            .cluster
            .update("virtualmachines", name, |vm| vm["metadata"]["finalizers"] = json!([]));
    }
    assert_eq!(
        AuthorityHost::purge(&harness.provider, &held).await?,
        PurgeOutcome::Purged
    );
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.cluster.names("datavolumes").is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_created_vm_without_a_uid_cannot_produce_a_grant() -> TestResult {
    let harness = Harness::new()?;
    harness.cluster.inject(Fault::new(
        Verb::Create,
        "virtualmachines",
        Effect::rewrite(|reply| {
            if let Some(metadata) = reply.pointer_mut("/metadata").and_then(|value| value.as_object_mut()) {
                metadata.remove("uid");
            }
        }),
    ));
    let error = harness
        .provider
        .lease(&registry_request()?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::Ops(_)))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_warm_claim_whose_reply_was_lost_is_resolved_from_the_applied_marker() -> TestResult {
    let harness = Harness::new()?;
    let digest = digest('a')?;
    seed_warm_vm(&harness.cluster, "warm-1", &digest, Some(512))?;
    harness
        .cluster
        .inject(Fault::new(Verb::Patch, "virtualmachines", Effect::ApplyThenReject(500)).named("warm-1"));
    let granted = harness
        .provider
        .lease(&snapshot_request(reference(&digest))?, &lease())
        .await?;
    assert_eq!(granted.sandbox_id.as_str(), "warm-1");
    assert_eq!(harness.cluster.count_requests(Verb::Patch, "virtualmachines"), 1);
    assert_eq!(harness.cluster.count_requests(Verb::Create, "virtualmachines"), 0);
    assert_eq!(harness.provider.held_leases().await?.len(), 1);

    harness.provider.cancel(&lease()).await?;
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.cluster.names("datavolumes").is_empty());
    Ok(())
}
