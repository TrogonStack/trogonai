use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ProviderKind, WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerHeartbeat};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, Provider, subjects};

use super::{ORIGINAL_UID, REPLACEMENT_UID, object_of, owned_entry, owner_of, worker};
use crate::actor_index::ActorIndex;
use crate::actor_index::memory::MemoryActorIndex;
use crate::adapter::tests_support::{FakeControl, TestResult};
use crate::ate_client::ResourceRevision;
use crate::ate_client::{AteError, Control};
use crate::{proto, recovery, worker as worker_task};

#[tokio::test]
async fn authority_reaper_never_deletes_an_actor_from_its_name_without_fleet_uid_proof() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let entry = owned_entry()?;
    let control = Arc::new(
        FakeControl::with_statuses(vec![proto::ActorState::Suspended])
            .with_uid(ORIGINAL_UID)
            .with_transitions(),
    );
    control.set_listed(vec![entry.actor.clone()]);
    let actor = object_of(&entry);
    let observed = control.get_actor(actor.clone()).await?;
    assert_eq!(observed.metadata.ok_or("no metadata")?.uid, ORIGINAL_UID);
    assert!(index.all().await?.is_empty());
    let provider = worker(control.clone(), index, "actual-reaper-worker")?;

    let reaped = recovery::reap(&provider, std::slice::from_ref(&entry.atespace)).await?;

    assert_eq!(reaped, 0);
    assert_eq!(control.delete_count(), 0);
    let preserved = control.get_actor(actor.clone()).await?;
    control
        .delete_actor(ResourceRevision::from_metadata(preserved.metadata.as_ref())?)
        .await?;
    assert!(matches!(control.get_actor(actor).await, Err(AteError::NotFound { .. })));
    Ok(())
}

/// Runs a real worker whose authority oracle always answers "not assigned", and reports whether
/// the worker reclaimed its actor, or left it alone because the actor's UID changed.
async fn actual_guard(replace_uid: bool) -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let context = server.jetstream().await;
    let index = Arc::new(MemoryActorIndex::default());
    let entry = owned_entry()?;
    index.put(&entry).await?;
    let owner = owner_of(&entry)?.clone();
    let control = Arc::new(FakeControl::running().with_uid(ORIGINAL_UID).with_transitions());
    let provider = Arc::new(worker(control.clone(), index.clone(), owner.as_str())?);
    assert_eq!(recovery::recover(&provider).await?.adopted, 1);
    let mut heartbeats = client
        .subscribe(subjects::worker_heartbeat(ProviderKind::Substrate, &owner))
        .await?;
    let mut queries = client.subscribe(subjects::worker_authority_check(&owner)).await?;
    client.flush().await?;
    let checks = Arc::new(AtomicUsize::new(0));
    let (confirm_heartbeat, heartbeat_confirmed) = tokio::sync::oneshot::channel::<()>();
    let oracle = tokio::spawn({
        let client = client.clone();
        let checks = checks.clone();
        let control = control.clone();
        let owner = owner.clone();
        async move {
            if heartbeat_confirmed.await.is_err() {
                return;
            }
            while let Some(message) = queries.next().await {
                let Ok(query) = serde_json::from_slice::<WorkerAuthorityRequest>(&message.payload) else {
                    continue;
                };
                if query.worker_id != owner {
                    continue;
                }
                if replace_uid {
                    control.set_uid(REPLACEMENT_UID);
                }
                checks.fetch_add(1, Ordering::SeqCst);
                let response = WorkerAuthorityResponse {
                    owned: Vec::new(),
                    not_assigned: query.lease_ids,
                };
                let (Some(reply), Ok(payload)) = (message.reply, serde_json::to_vec(&response)) else {
                    continue;
                };
                let _ = client.publish(reply, payload.into()).await;
            }
        }
    });
    let mut authority = AuthorityConfig::new(AuthorityMode::Enforce);
    authority.interval = Duration::from_millis(10);
    authority.min_lease_age = Duration::ZERO;
    authority.confirmations = 1;
    authority.grace = Duration::ZERO;
    let cancel = CancellationToken::new();
    worker_task::spawn_heartbeat_task(client.clone(), provider.clone(), cancel.clone());
    let mut running = tokio::spawn(worker_task::run_with_authority(
        client,
        context,
        provider.clone(),
        owner.clone(),
        cancel.clone(),
        authority,
    ));

    let observation: TestResult = async {
        let heartbeat = tokio::time::timeout(Duration::from_secs(5), heartbeats.next())
            .await?
            .ok_or("heartbeat stream closed")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
        assert_eq!(heartbeat.worker_id, owner);
        assert_eq!(heartbeat.capacity.current_leases, 1);
        assert!(!running.is_finished());
        let _ = confirm_heartbeat.send(());
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let done = if replace_uid {
                    checks.load(Ordering::SeqCst) >= 4
                } else {
                    provider.active_lease(&entry.lease_id).await.is_none()
                        && index.get(&entry.atespace, &entry.actor).await?.is_none()
                        && control.delete_count() == 1
                };
                if done {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<(), Box<dyn std::error::Error>>(())
        })
        .await??;
        assert!(checks.load(Ordering::SeqCst) > 0);
        if replace_uid {
            assert_eq!(control.suspend_count(), 0);
            assert_eq!(control.delete_count(), 0);
            assert!(provider.active_lease(&entry.lease_id).await.is_some());
            assert_eq!(index.get(&entry.atespace, &entry.actor).await?.as_ref(), Some(&entry));
        }
        Ok(())
    }
    .await;

    cancel.cancel();
    if tokio::time::timeout(Duration::from_secs(5), &mut running)
        .await
        .is_err()
    {
        running.abort();
        let _ = running.await;
    }
    oracle.abort();
    let _ = oracle.await;
    control.set_uid(ORIGINAL_UID);
    Provider::purge(provider.as_ref(), &entry.lease_id).await?;
    assert!(provider.active_lease(&entry.lease_id).await.is_none());
    assert!(index.get(&entry.atespace, &entry.actor).await?.is_none());
    assert!(matches!(
        control.get_actor(object_of(&entry)).await,
        Err(AteError::NotFound { .. })
    ));
    observation
}

#[tokio::test]
async fn authority_actual_worker_reclaims_only_its_exact_actor_uid() -> TestResult {
    actual_guard(false).await
}

#[tokio::test]
async fn authority_actual_worker_never_mutates_a_replacement_actor_uid() -> TestResult {
    actual_guard(true).await
}
