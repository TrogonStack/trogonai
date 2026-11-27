mod support;

use support::*;
use trogon_sandbox_contract::{
    ArtifactType, LeaseId, ProviderKind, SnapshotExportError, SnapshotExportOutcome, SnapshotPushTarget,
};
use trogon_sandbox_protocol::subjects;

fn export_subject(running: &Running) -> String {
    subjects::worker_snapshot_export(&running.worker)
}

fn target(running: &Running) -> SnapshotPushTarget {
    SnapshotPushTarget {
        registry: running.world.registry.address.clone(),
        repository: "snapshots/exported".into(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    }
}

#[tokio::test]
async fn a_dispatch_is_granted_once_and_a_cancel_releases_it() -> TestResult {
    let mut running = Running::start().await?;
    running.grant("lease-1").await?;
    assert_eq!(running.world.cluster.state().jobs_created, 1);
    // A redelivery gets the same grant instead of a second sandbox.
    running.grant("lease-1").await?;
    assert_eq!(running.world.cluster.state().jobs_created, 1);
    assert!(running.world.provider.holds_lease(&lease_id("lease-1")).await);

    let cancel = subjects::provider_cancel(ProviderKind::K8sMicroVm, &LeaseId::new("lease-1"));
    running.publish(cancel, &serde_json::json!({})).await?;
    let cluster = &running.world.cluster;
    until(|| cluster.names("jobs").is_empty()).await?;
    assert!(!running.world.provider.holds_lease(&lease_id("lease-1")).await);

    // The released id launches afresh.
    running.grant("lease-1").await?;
    assert_eq!(running.world.cluster.state().jobs_created, 2);
    running.stop().await
}

/// Pins today's behaviour, pending a decision on which one every worker should have: the cancel handler forgets the
/// grant only once a cancel succeeds, so a redispatch after a failed cancel is answered with the original grant.
/// The aca, modal, daytona and vercel workers forget the grant first and refuse the redispatch instead.
#[tokio::test]
async fn a_redispatch_after_a_failed_cancel_is_answered_with_the_original_grant() -> TestResult {
    let mut running = Running::start().await?;
    let first = running
        .dispatch("lease-stuck", serde_json::to_value(lease_request()?)?)
        .await?;
    running.world.cluster.state().fail_deletes = 100;

    let cancel = subjects::provider_cancel(ProviderKind::K8sMicroVm, &LeaseId::new("lease-stuck"));
    running.publish(cancel, &serde_json::json!({})).await?;
    let cluster = &running.world.cluster;
    until(|| cluster.state().fail_deletes < 100).await?;
    let second = running
        .dispatch("lease-stuck", serde_json::to_value(lease_request()?)?)
        .await?;

    assert_eq!(first, second);
    assert_eq!(running.world.cluster.state().jobs_created, 1);
    assert!(running.world.provider.holds_lease(&lease_id("lease-stuck")).await);
    running.world.cluster.state().fail_deletes = 0;
    running.stop().await
}

#[tokio::test]
async fn a_cancel_of_a_lease_nobody_holds_is_harmless() -> TestResult {
    let mut running = Running::start().await?;
    let cancel = subjects::provider_cancel(ProviderKind::K8sMicroVm, &LeaseId::new("lease-absent"));
    running.publish(cancel, &serde_json::json!({})).await?;
    // A cancel whose subject names no lease is ignored.
    running
        .publish(
            "sandbox.provider.k8s_micro_vm.cancel.".to_owned(),
            &serde_json::json!({}),
        )
        .await?;
    running.grant("lease-after").await?;
    running.stop().await
}

#[tokio::test]
async fn failed_dispatches_say_why() -> TestResult {
    let mut running = Running::start().await?;
    let subject = subjects::provider_dispatch(ProviderKind::K8sMicroVm);
    let undecodable = running.ask(subject, &serde_json::json!({"unexpected": true})).await?;
    assert!(undecodable["decode_error"].is_string(), "{undecodable}");

    running.world.cluster.state().fail_creates.push("jobs".into());
    let refused = running
        .dispatch("lease-1", serde_json::to_value(lease_request()?)?)
        .await?;
    assert!(refused["provider_error"].is_string(), "{refused}");
    running.stop().await
}

#[tokio::test]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    let subject = subjects::provider_dispatch(ProviderKind::K8sMicroVm);
    let payload = serde_json::json!({"lease_id": "lease-1", "request": serde_json::to_value(lease_request()?)?});
    running.publish(subject, &payload).await?;
    let cluster = &running.world.cluster;
    until(|| cluster.state().jobs_created == 1).await?;
    running.stop().await
}

#[tokio::test]
async fn snapshot_exports_report_receipts_and_failures() -> TestResult {
    let mut running = Running::start().await?;
    // An undecodable request is dropped.
    running.silent(export_subject(&running), b"not json".to_vec()).await?;

    let absent = running
        .ask(
            export_subject(&running),
            &serde_json::json!({"lease_id": "lease-absent", "target": target(&running)}),
        )
        .await?;
    let absent: SnapshotExportOutcome = serde_json::from_value(absent)?;
    assert!(
        matches!(
            absent,
            SnapshotExportOutcome::Err(SnapshotExportError::Permanent { .. })
        ),
        "{absent:?}"
    );

    running.grant("lease-export").await?;
    running
        .world
        .cluster
        .script(ExecScript::new("tar").stdout(b"archive-bytes"));
    let exported = running
        .ask(
            export_subject(&running),
            &serde_json::json!({"lease_id": "lease-export", "target": target(&running)}),
        )
        .await?;
    let exported: SnapshotExportOutcome = serde_json::from_value(exported)?;
    assert!(matches!(exported, SnapshotExportOutcome::Ok(receipt) if receipt.bytes == 13));

    running.world.registry.shutdown();
    let failed = running
        .ask(
            export_subject(&running),
            &serde_json::json!({"lease_id": "lease-export", "target": target(&running)}),
        )
        .await?;
    let failed: SnapshotExportOutcome = serde_json::from_value(failed)?;
    assert!(
        matches!(
            failed,
            SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
        ),
        "{failed:?}"
    );
    running.stop().await
}
