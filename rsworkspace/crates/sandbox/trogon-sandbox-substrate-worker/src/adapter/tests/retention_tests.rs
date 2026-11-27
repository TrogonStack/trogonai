use std::time::Duration;

use super::super::tests_support::{FakeControl, TestResult, complete_snapshot};
use super::*;

async fn held_named(control: Arc<FakeControl>) -> Result<(SubstrateProvider, LeaseId, ActorEntry), Box<dyn Error>> {
    let provider = provider(control)?;
    let lease_id = LeaseId::new("01ARZ");
    let entry = adopt_test(&provider, &lease_id, named("durable-work", true)?).await?;
    Ok((provider, lease_id, entry))
}

fn pending(mut snapshot: proto::Snapshot) -> Result<proto::Snapshot, Box<dyn Error>> {
    snapshot.durable_snapshot.as_mut().ok_or("no durable snapshot")?.state =
        proto::SnapshotStorageState::Pending as i32;
    Ok(snapshot)
}

fn with_uri(mut snapshot: proto::Snapshot, uri: &str) -> Result<proto::Snapshot, Box<dyn Error>> {
    snapshot
        .durable_snapshot
        .as_mut()
        .and_then(|storage| storage.object.as_mut())
        .ok_or("no snapshot object")?
        .snapshot_uri = uri.to_string();
    Ok(snapshot)
}

async fn assert_retention_refused(generation: i32, snapshots: Vec<proto::Snapshot>) -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    control.set_snapshot_state(generation, snapshots);
    let (provider, lease_id, original) = held_named(control.clone()).await?;

    let result = Provider::cancel(&provider, &lease_id).await;

    assert!(result.is_err(), "unproved retained state must not be acknowledged");
    assert_eq!(control.delete_count(), 0);
    assert!(provider.active_lease(&lease_id).await.is_some());
    assert_eq!(
        provider.index().get(&original.atespace, &original.actor).await?,
        Some(original)
    );
    Ok(())
}

#[tokio::test]
async fn completed_latest_memory_snapshot_can_be_retained() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    control.set_snapshot_state(2, vec![complete_snapshot(1), complete_snapshot(2)]);
    let (provider, lease_id, original) = held_named(control.clone()).await?;

    Provider::cancel(&provider, &lease_id).await?;

    assert_eq!(control.delete_count(), 0);
    assert!(provider.active_lease(&lease_id).await.is_none());
    let retained = provider
        .index()
        .get(&original.atespace, &original.actor)
        .await?
        .ok_or("the claim was dropped")?;
    assert!(retained.retained);
    assert_eq!(retained.owner, original.owner);
    Ok(())
}

#[tokio::test]
async fn a_pending_upload_must_complete_before_retained_release_is_acknowledged() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    control.set_snapshot_state(1, vec![pending(complete_snapshot(1))?]);
    let (provider, lease_id, original) = held_named(control.clone()).await?;
    let provider = Arc::new(provider);
    let task = tokio::spawn({
        let provider = provider.clone();
        let lease_id = lease_id.clone();
        async move { Provider::cancel(provider.as_ref(), &lease_id).await }
    });

    tokio::time::timeout(Duration::from_secs(5), control.snapshot_observed.notified()).await?;
    control.set_snapshot_state(1, vec![complete_snapshot(1)]);
    task.await??;

    assert_eq!(control.delete_count(), 0);
    assert!(provider.active_lease(&lease_id).await.is_none());
    let retained = provider
        .index()
        .get(&original.atespace, &original.actor)
        .await?
        .ok_or("the claim was dropped")?;
    assert!(retained.retained);
    Ok(())
}

#[tokio::test]
async fn snapshot_fidelity_downgrades_preserve_cleanup_custody() -> TestResult {
    for fidelity in [proto::SnapshotFidelity::Rootfs, proto::SnapshotFidelity::Volumes] {
        let mut snapshot = complete_snapshot(1);
        snapshot.fidelity = fidelity as i32;
        // Rootfs and volumes are durable but do not preserve memory.
        assert_retention_refused(1, vec![snapshot]).await?;
    }
    Ok(())
}

#[tokio::test]
async fn an_older_complete_snapshot_cannot_hide_a_pending_latest_generation() -> TestResult {
    let latest = pending(complete_snapshot(2))?;
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    control.set_snapshot_state(2, vec![complete_snapshot(1), latest]);
    let (provider, lease_id, _) = held_named(control.clone()).await?;
    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("acknowledged")?;
    assert!(matches!(error, SubstrateError::ResumeTimeout { .. }));
    Ok(())
}

#[tokio::test]
async fn an_absent_latest_generation_is_not_replaced_by_older_memory() -> TestResult {
    assert_retention_refused(2, vec![complete_snapshot(1)]).await
}

#[tokio::test]
async fn duplicate_latest_generations_are_ambiguous() -> TestResult {
    assert_retention_refused(2, vec![complete_snapshot(2), complete_snapshot(2)]).await
}

#[tokio::test]
async fn a_golden_tag_snapshot_cannot_substitute_for_the_actors_captured_state() -> TestResult {
    let mut snapshot = complete_snapshot(1);
    snapshot.owner = proto::SnapshotOwner::Tag as i32;
    assert_retention_refused(1, vec![snapshot]).await
}

#[tokio::test]
async fn completed_storage_requires_an_actual_snapshot_object() -> TestResult {
    let mut missing = complete_snapshot(1);
    missing.durable_snapshot.as_mut().ok_or("no storage")?.object = None;
    assert_retention_refused(1, vec![missing]).await?;
    assert_retention_refused(1, vec![with_uri(complete_snapshot(1), "")?]).await
}

#[tokio::test]
async fn a_snapshot_uri_must_be_absolute_and_well_formed() -> TestResult {
    assert_retention_refused(1, vec![with_uri(complete_snapshot(1), "/snapshots/relative")?]).await?;
    assert_retention_refused(1, vec![with_uri(complete_snapshot(1), "not a uri")?]).await
}

#[tokio::test]
async fn unknown_or_missing_snapshot_evidence_never_confirms_retention() -> TestResult {
    let mut unknown = complete_snapshot(1);
    unknown.fidelity = i32::MAX;
    assert_retention_refused(1, vec![unknown]).await?;
    let mut no_id = complete_snapshot(1);
    no_id.uuid.clear();
    assert_retention_refused(1, vec![no_id]).await?;
    assert_retention_refused(0, vec![]).await?;
    assert_retention_refused(2, vec![complete_snapshot(2), complete_snapshot(3)]).await?;
    let mut unknown_owner = complete_snapshot(1);
    unknown_owner.owner = i32::MAX;
    assert_retention_refused(1, vec![unknown_owner]).await?;
    let mut unknown_storage = complete_snapshot(1);
    unknown_storage.durable_snapshot.as_mut().ok_or("no storage")?.state = i32::MAX;
    assert_retention_refused(1, vec![unknown_storage]).await
}

#[tokio::test]
async fn an_actor_that_resumed_again_is_not_a_retained_actor() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![
        proto::ActorState::Suspended,
        proto::ActorState::Running,
    ]));
    let (provider, lease_id, _) = held_named(control.clone()).await?;
    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("acknowledged")?;
    assert!(matches!(error, SubstrateError::Ownership(ref message) if message.contains("no longer suspended")));
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn an_actor_that_vanished_before_its_state_was_confirmed_is_refused() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    let (provider, _, original) = held_named(control.clone()).await?;
    control.fail_get_with(crate::ate_client::AteError::NotFound { message: "gone".into() });
    let error = provider
        .confirm_retained_snapshot(&original)
        .await
        .err()
        .ok_or("confirmed")?;
    assert!(matches!(error, SubstrateError::Ownership(ref message) if message.contains("disappeared")));
    Ok(())
}

#[tokio::test]
async fn a_crashed_persistent_name_keeps_its_snapshot_until_explicit_purge() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Crashed]));
    control.set_snapshot_state(1, vec![complete_snapshot(1)]);
    let (provider, lease_id, original) = held_named(control.clone()).await?;

    assert!(Provider::cancel(&provider, &lease_id).await.is_err());
    assert_eq!(control.delete_count(), 0);
    assert_eq!(
        provider.index().get(&original.atespace, &original.actor).await?,
        Some(original.clone())
    );

    Provider::purge(&provider, &lease_id).await?;
    assert_eq!(control.delete_count(), 1);
    assert!(
        provider
            .index()
            .get(&original.atespace, &original.actor)
            .await?
            .is_none()
    );
    Ok(())
}

#[test]
fn a_snapshot_without_storage_evidence_is_not_yet_durable() -> TestResult {
    let mut snapshot = complete_snapshot(1);
    snapshot.durable_snapshot = None;
    let actor = proto::Actor {
        status: Some(proto::ActorStatus {
            state: proto::ActorState::Suspended as i32,
            last_assigned_generation: 1,
            snapshots: vec![snapshot],
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(super::super::retained_snapshot::durable_memory_snapshot(&actor)?.is_none());
    Ok(())
}
