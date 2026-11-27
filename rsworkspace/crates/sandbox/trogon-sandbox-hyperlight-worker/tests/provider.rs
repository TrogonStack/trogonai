mod support;

use serde_json::{Value, json};
use support::{Rig, TestResult, component_image, empty_workspace, lease, request, request_with};
use trogon_sandbox_contract::{LeaseRequest, ProviderKind};
use trogon_sandbox_protocol::Provider;

async fn refusal(rig: &Rig, request: LeaseRequest) -> Result<String, Box<dyn std::error::Error>> {
    let refused = rig.provider.lease(&request, &lease("l1")).await.err();
    Ok(refused.ok_or("expected a refusal")?.to_string())
}

async fn assert_refused(rig: &Rig, request: LeaseRequest, expected: &str) -> TestResult {
    let message = refusal(rig, request).await?;
    assert!(message.contains(expected), "{message} lacks {expected}");
    Ok(())
}

async fn assert_spec_refused(rig: &Rig, image: Value, workspace: Value, spec: Value, expected: &str) -> TestResult {
    let request = request_with(image, workspace, spec)?;
    assert_refused(rig, request, expected).await
}

#[tokio::test]
async fn grants_a_lease_for_a_component() -> TestResult {
    let rig = Rig::start().await?;
    let granted = rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    assert_eq!(granted.sandbox_id.as_str(), "hl-l1");
    assert_eq!(
        granted.evidence_subject,
        trogon_sandbox_protocol::subjects::evidence_subject(&lease("l1"))
    );
    assert_eq!(granted.worker_id, None);
    assert_eq!(rig.provider.kind(), ProviderKind::Hyperlight);
    assert_eq!(rig.provider.active_lease_count(), 1);
    assert_eq!(rig.hypervisor.loads(), 1);
    Ok(())
}

#[tokio::test]
async fn refuses_a_lease_it_already_holds() -> TestResult {
    let rig = Rig::start().await?;
    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    assert_refused(&rig, request("echo")?, "retains a native handle").await?;
    Ok(())
}

#[tokio::test]
async fn refuses_what_a_sealed_guest_cannot_take() -> TestResult {
    let rig = Rig::start().await?;
    let image = || component_image("echo");
    assert_spec_refused(
        &rig,
        image(),
        empty_workspace(),
        json!({"privileged": true}),
        "privileged: true is not supported",
    )
    .await?;
    assert_spec_refused(
        &rig,
        image(),
        empty_workspace(),
        json!({"env": {"A": "b"}}),
        "spec.env is not supported",
    )
    .await?;
    let clone =
        json!({"clone": {"url": "https://example.test/r.git", "sha": "abc", "base_sha": null, "auth": {"none": {}}}});
    assert_spec_refused(&rig, image(), clone, json!({}), "clone workspaces").await?;
    let mount = json!({"host_mount": {"path": "/tmp"}});
    assert_spec_refused(&rig, image(), mount, json!({}), "host_mount").await?;
    let volume = json!({"s3_volume": {"bucket": "b", "prefix": "p", "mode": "read_only"}});
    assert_spec_refused(&rig, image(), volume, json!({}), "s3_volume").await?;
    assert_eq!(rig.provider.active_lease_count(), 0);
    Ok(())
}

#[tokio::test]
async fn refuses_images_it_cannot_boot() -> TestResult {
    let rig = Rig::start().await?;
    let native = json!({"kind": "native_snapshot", "reference": {"provider": "hyperlight", "id": "snap-1"}});
    assert_spec_refused(&rig, native, empty_workspace(), json!({}), "native provider snapshots").await?;
    let tag = json!({"kind": "snapshot_tag", "tag": {"tenant": "acme", "name": "t"}, "artifact_type": "hyperlight_snapshot_v1"});
    assert_spec_refused(&rig, tag, empty_workspace(), json!({}), "must resolve SnapshotTag").await?;
    let wrong = json!({"kind": "snapshot", "reference": {"registry": "r", "repository": "p", "digest": support::sha256(b"x"), "artifact_type": "sandbox_rootfs_v1"}});
    assert_spec_refused(
        &rig,
        wrong,
        empty_workspace(),
        json!({}),
        "only restores HyperlightSnapshotV1",
    )
    .await?;
    assert_refused(&rig, request("missing")?, "not found under component_root").await?;
    assert_refused(&rig, request("../escape")?, "without `..`").await?;
    Ok(())
}

#[tokio::test]
async fn reports_a_hypervisor_that_fails_to_load() -> TestResult {
    let rig = Rig::start().await?;
    rig.hypervisor.fail_load(true);
    assert_refused(&rig, request("echo")?, "acquisition failed").await?;
    rig.hypervisor.fail_load(false);
    rig.hypervisor.fail_build(true);
    assert_refused(&rig, request("echo")?, "acquisition failed").await?;
    rig.hypervisor.fail_build(false);
    rig.hypervisor.panic_on_load(true);
    assert_refused(&rig, request("echo")?, "blocking task failed").await?;
    assert_eq!(rig.provider.active_lease_count(), 0);
    Ok(())
}

#[tokio::test]
async fn refuses_a_lease_id_that_cannot_name_a_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    let granted = rig.provider.lease(&request("echo")?, &lease("bad\nid")).await;
    assert!(granted.is_err());
    Ok(())
}

#[tokio::test]
async fn recycles_a_cancelled_guest_into_the_warm_pool() -> TestResult {
    let rig = Rig::start().await?;
    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    rig.provider.cancel(&lease("l1")).await?;
    assert_eq!(rig.provider.active_lease_count(), 0);
    assert_eq!((rig.hypervisor.builds(), rig.hypervisor.unloads()), (1, 1));
    rig.provider.lease(&request("echo")?, &lease("l2")).await?;
    assert_eq!(rig.hypervisor.builds(), 1);
    rig.provider.cancel(&lease("nobody")).await?;
    Ok(())
}

#[tokio::test]
async fn reports_a_guest_that_will_not_unload() -> TestResult {
    let rig = Rig::start().await?;
    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    rig.hypervisor.fail_unload(true);
    assert!(rig.provider.cancel(&lease("l1")).await.is_err());
    assert_eq!(rig.provider.active_lease_count(), 0);
    Ok(())
}

#[tokio::test]
async fn advertises_what_it_was_configured_with() -> TestResult {
    let rig = Rig::with(2, 3).await?;
    let worker = trogon_sandbox_contract::WorkerId::new("w1")?;
    let beat = rig.provider.heartbeat(&worker);
    assert_eq!(beat.capacity.max_concurrent_leases, 3);
    assert_eq!(beat.capacity.current_leases, 0);
    assert_eq!(beat.provider, ProviderKind::Hyperlight);
    rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    assert_eq!(rig.provider.heartbeat(&worker).capacity.current_leases, 1);
    assert_eq!(rig.provider.profile().provider, ProviderKind::Hyperlight);
    Ok(())
}

#[tokio::test]
async fn binds_to_one_worker_only() -> TestResult {
    let rig = Rig::start().await?;
    let first = trogon_sandbox_contract::WorkerId::new("w1")?;
    let second = trogon_sandbox_contract::WorkerId::new("w2")?;
    rig.provider.bind_worker(&first)?;
    rig.provider.bind_worker(&first)?;
    assert!(rig.provider.bind_worker(&second).is_err());
    let granted = rig.provider.lease(&request("echo")?, &lease("l1")).await?;
    assert_eq!(granted.worker_id, Some(first));
    Ok(())
}
