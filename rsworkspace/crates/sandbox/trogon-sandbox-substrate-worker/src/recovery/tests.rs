use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};

use super::atespace_of::atespace_of;
use super::*;
use crate::actor_index::memory::MemoryActorIndex;
use crate::actor_index::{
    ActorClaimId, ActorEntry, ActorIndex, ActorIndexError, ActorIndexResult, ActorOwner, ActorUid,
};
use crate::adapter::SubstrateProvider;
use crate::adapter::tests_support::{FakeControl, provider_with as unconfigured_provider};
use crate::ate_client::AteError;
use crate::naming::{ActorName, Atespace};
use crate::proto;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const UID: &str = "00000000-0000-4000-8000-000000000001";
const OTHER_UID: &str = "00000000-0000-4000-8000-000000000002";

/// An index that can be told to fail reads or to refuse every compare-and-exchange.
#[derive(Default)]
struct FlakyIndex {
    inner: MemoryActorIndex,
    fail_reads: AtomicBool,
    fail_every_other_read: AtomicBool,
    reads: AtomicUsize,
    exchange_fails: AtomicBool,
    exchange_loses: AtomicBool,
}

#[async_trait::async_trait]
impl ActorIndex for FlakyIndex {
    async fn put(&self, entry: &ActorEntry) -> ActorIndexResult<()> {
        self.inner.put(entry).await
    }

    async fn get(&self, atespace: &str, actor: &str) -> ActorIndexResult<Option<ActorEntry>> {
        self.inner.get(atespace, actor).await
    }

    async fn delete(&self, atespace: &str, actor: &str) -> ActorIndexResult<()> {
        self.inner.delete(atespace, actor).await
    }

    async fn all(&self) -> ActorIndexResult<Vec<ActorEntry>> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail_reads.load(Ordering::SeqCst)
            || (self.fail_every_other_read.load(Ordering::SeqCst) && read % 2 == 1)
        {
            return Err(ActorIndexError::storage(std::io::Error::other("unreadable")));
        }
        self.inner.all().await
    }

    async fn compare_exchange(
        &self,
        expected: Option<&ActorEntry>,
        replacement: Option<&ActorEntry>,
    ) -> ActorIndexResult<bool> {
        if self.exchange_fails.load(Ordering::SeqCst) {
            return Err(ActorIndexError::storage(std::io::Error::other("unwritable")));
        }
        if self.exchange_loses.load(Ordering::SeqCst) {
            return Ok(false);
        }
        self.inner.compare_exchange(expected, replacement).await
    }
}

fn provider_with(
    control: Arc<FakeControl>,
    index: Arc<dyn ActorIndex>,
) -> Result<SubstrateProvider, Box<dyn std::error::Error>> {
    Ok(unconfigured_provider(control, index)?.with_worker_id(WorkerId::new("recovery-test-worker")?))
}

fn atespace() -> Result<Atespace, Box<dyn std::error::Error>> {
    Ok(Atespace::for_tenant(&TenantId::new("tenant-a")?)?)
}

fn entry(actor: &str, lease: &str, named: bool, retained: bool) -> Result<ActorEntry, Box<dyn std::error::Error>> {
    Ok(ActorEntry {
        atespace: "tenant-a".to_string(),
        actor: actor.to_string(),
        lease_id: LeaseId::new(lease),
        named,
        retained,
        owner: Some(ActorOwner {
            worker_id: WorkerId::new("recovery-test-worker")?,
            claim_id: ActorClaimId::generate(),
            actor_uid: Some(ActorUid::try_from(UID.to_string())?),
        }),
    })
}

fn running_control() -> Arc<FakeControl> {
    Arc::new(FakeControl::running().with_uid(UID))
}

fn suspended_control() -> Arc<FakeControl> {
    Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]).with_uid(UID))
}

#[tokio::test]
async fn a_restart_puts_live_leases_back_in_the_active_map() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let actor = ActorName::for_lease(&LeaseId::new("01ARZ"))?;
    index.put(&entry(actor.as_str(), "01ARZ", false, false)?).await?;
    let provider = provider_with(running_control(), index)?;

    let report = recover(&provider).await?;
    assert_eq!(report.adopted, 1);
    assert_eq!(report.pruned, 0);
    assert!(provider.active_lease(&LeaseId::new("01ARZ")).await.is_some());
    Ok(())
}

#[tokio::test]
async fn an_entry_whose_actor_is_gone_is_pruned_rather_than_adopted() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    index.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    let control = running_control();
    control.fail_get_with(AteError::NotFound {
        message: "no such actor".to_string(),
    });
    let provider = provider_with(control, index.clone())?;

    let report = recover(&provider).await?;
    assert_eq!(report.pruned, 1);
    assert_eq!(report.adopted, 0);
    assert!(index.all().await?.is_empty());
    assert!(provider.active_lease(&LeaseId::new("01ARZ")).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_failed_prune_keeps_the_entry_and_counts_nothing() -> TestResult {
    let index = Arc::new(FlakyIndex::default());
    index.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    index.exchange_fails.store(true, Ordering::SeqCst);
    let control = running_control();
    control.fail_get_with(AteError::NotFound {
        message: "no such actor".to_string(),
    });
    let provider = provider_with(control, index.clone())?;

    assert_eq!(recover(&provider).await?.pruned, 0);
    assert_eq!(index.all().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_retained_named_actor_is_left_parked_and_not_leased() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let actor = ActorName::for_named_lease(&atespace()?, "build");
    index.put(&entry(actor.as_str(), "01OLD", true, true)?).await?;
    let provider = provider_with(suspended_control(), index.clone())?;

    let report = recover(&provider).await?;
    assert_eq!(report.retained, 1);
    assert_eq!(report.adopted, 0);
    assert_eq!(
        index.all().await?.len(),
        1,
        "the tenant's parked state must survive a restart"
    );
    Ok(())
}

#[tokio::test]
async fn a_suspended_actor_with_a_live_entry_is_not_adopted_as_a_working_lease() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    index.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    let provider = provider_with(suspended_control(), index)?;

    let report = recover(&provider).await?;
    assert_eq!(report.adopted, 0);
    assert_eq!(report.pruned, 1);
    Ok(())
}

#[tokio::test]
async fn an_unreachable_control_plane_does_not_erase_the_index() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    index.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    let control = running_control();
    control.fail_get_with(AteError::Unavailable {
        message: "connection refused".to_string(),
    });
    let provider = provider_with(control, index.clone())?;

    let report = recover(&provider).await?;
    assert_eq!(report.pruned, 0);
    assert_eq!(report.adopted, 0);
    assert_eq!(index.all().await?.len(), 1, "an outage must not be read as dead actors");
    Ok(())
}

#[tokio::test]
async fn entries_without_confirmed_ownership_are_skipped() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let mut unbound = entry("sbx-01aaa", "01AAA", false, false)?;
    unbound.owner = None;
    let mut foreign = entry("sbx-01bbb", "01BBB", false, false)?;
    if let Some(owner) = foreign.owner.as_mut() {
        owner.worker_id = WorkerId::new("another-worker")?;
    }
    let mut without_uid = entry("sbx-01ccc", "01CCC", false, false)?;
    if let Some(owner) = without_uid.owner.as_mut() {
        owner.actor_uid = None;
    }
    for item in [&unbound, &foreign, &without_uid] {
        index.put(item).await?;
    }
    let provider = provider_with(running_control(), index.clone())?;

    assert_eq!(recover(&provider).await?, RecoveryReport::default());
    assert_eq!(index.all().await?.len(), 3);
    Ok(())
}

#[tokio::test]
async fn an_actor_with_a_different_resource_uid_keeps_its_claim() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    index.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    let control = Arc::new(FakeControl::running().with_uid(OTHER_UID));
    let provider = provider_with(control, index.clone())?;

    assert_eq!(recover(&provider).await?, RecoveryReport::default());
    assert_eq!(index.all().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_unconfirmable_claim_is_not_adopted() -> TestResult {
    let lost = Arc::new(FlakyIndex::default());
    lost.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    lost.exchange_loses.store(true, Ordering::SeqCst);
    let provider = provider_with(running_control(), lost)?;
    assert_eq!(recover(&provider).await?.adopted, 0);

    let failing = Arc::new(FlakyIndex::default());
    failing.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    failing.exchange_fails.store(true, Ordering::SeqCst);
    let provider = provider_with(running_control(), failing)?;
    assert_eq!(recover(&provider).await?.adopted, 0);
    Ok(())
}

#[tokio::test]
async fn entries_outside_the_naming_scheme_are_not_adopted() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    index.put(&entry("operators-own-actor", "01ARZ", false, false)?).await?;
    let provider = provider_with(running_control(), index)?;
    assert_eq!(recover(&provider).await?.adopted, 0);

    let index = Arc::new(MemoryActorIndex::default());
    let mut odd_space = entry("sbx-01arz", "01ARZ", false, false)?;
    odd_space.atespace = "Not A Tenant".to_string();
    index.put(&odd_space).await?;
    let provider = provider_with(running_control(), index)?;
    assert_eq!(recover(&provider).await?.adopted, 0);
    Ok(())
}

#[tokio::test]
async fn recovery_reports_an_unreadable_index() -> TestResult {
    let index = Arc::new(FlakyIndex::default());
    index.fail_reads.store(true, Ordering::SeqCst);
    let provider = provider_with(running_control(), index)?;
    assert!(matches!(recover(&provider).await, Err(RecoveryError::Index(_))));
    assert!(matches!(
        reap(&provider, &["tenant-a".to_string()]).await,
        Err(RecoveryError::Index(_))
    ));
    Ok(())
}

#[tokio::test]
async fn the_reaper_deletes_an_unclaimed_actor_this_fleet_made() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let control = suspended_control();
    control.set_listed(vec!["sbx-01orphan".to_string()]);
    index.put(&entry("sbx-01orphan", "01ORPHAN", false, false)?).await?;
    let provider = provider_with(control.clone(), index)?;

    assert_eq!(reap(&provider, &["tenant-a".to_string()]).await?, 1);
    assert_eq!(control.delete_count(), 1);
    Ok(())
}

#[tokio::test]
async fn the_reaper_skips_other_atespaces_and_reports_cleanup_failures() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let control = suspended_control();
    index.put(&entry("operators-own-actor", "01ARZ", false, false)?).await?;
    let provider = provider_with(control.clone(), index)?;

    assert_eq!(reap(&provider, &["other".to_string()]).await?, 0);
    assert_eq!(reap(&provider, &["tenant-a".to_string()]).await?, 0);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn the_reaper_leaves_a_claimed_actor_alone() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    index.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    let control = running_control();
    control.set_listed(vec!["sbx-01arz".to_string()]);
    let provider = provider_with(control.clone(), index)?;

    assert_eq!(recover(&provider).await?.adopted, 1);
    assert_eq!(reap(&provider, &["tenant-a".to_string()]).await?, 0);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn the_reaper_never_touches_an_actor_it_did_not_create() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let control = suspended_control();
    control.set_listed(vec!["operators-own-actor".to_string(), "jupyter".to_string()]);
    let provider = provider_with(control.clone(), index)?;

    assert_eq!(reap(&provider, &["tenant-a".to_string()]).await?, 0);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_retained_named_actor_is_not_reaped() -> TestResult {
    let index = Arc::new(MemoryActorIndex::default());
    let actor = ActorName::for_named_lease(&atespace()?, "build");
    index.put(&entry(actor.as_str(), "01OLD", true, true)?).await?;
    let control = suspended_control();
    control.set_listed(vec![actor.as_str().to_string()]);
    let provider = provider_with(control.clone(), index)?;

    assert_eq!(reap(&provider, &["tenant-a".to_string()]).await?, 0);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn the_reap_loop_sweeps_on_each_tick_and_survives_failures() -> TestResult {
    let index = Arc::new(FlakyIndex::default());
    let control = suspended_control();
    control.set_listed(vec!["sbx-01orphan".to_string()]);
    index.put(&entry("sbx-01orphan", "01ORPHAN", false, false)?).await?;
    let provider = Arc::new(provider_with(control.clone(), index.clone())?);

    let handle = tokio::spawn(reap_loop(provider.clone(), Duration::from_millis(5)));
    tokio::time::timeout(Duration::from_secs(5), async {
        while control.delete_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;

    index.fail_reads.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!handle.is_finished(), "an unreadable index must not stop the loop");
    handle.abort();
    Ok(())
}

#[tokio::test]
async fn the_reap_loop_logs_a_failed_sweep_and_keeps_going() -> TestResult {
    let index = Arc::new(FlakyIndex::default());
    index.fail_every_other_read.store(true, Ordering::SeqCst);
    index.put(&entry("sbx-01arz", "01ARZ", false, false)?).await?;
    let provider = Arc::new(provider_with(suspended_control(), index.clone())?);
    let handle = tokio::spawn(reap_loop(provider, Duration::from_millis(5)));
    // Every sweep reads the index once to list atespaces and once more inside `reap`, which fails.
    tokio::time::timeout(Duration::from_secs(5), async {
        while index.reads.load(Ordering::SeqCst) < 6 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    assert!(!handle.is_finished());
    handle.abort();
    Ok(())
}

#[tokio::test]
async fn an_entry_without_an_owner_cannot_be_adopted() -> TestResult {
    let provider = provider_with(running_control(), Arc::new(MemoryActorIndex::default()))?;
    let actor = ActorName::for_lease(&LeaseId::new("01ARZ"))?;
    let mut unowned = entry(actor.as_str(), "01ARZ", false, false)?;
    unowned.owner = None;
    let observed = proto::Actor {
        status: Some(proto::ActorStatus {
            state: proto::ActorState::Running as i32,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(!adopt::adopt(&provider, &unowned, &observed).await);
    Ok(())
}

#[test]
fn an_anonymous_actor_name_still_identifies_its_lease_for_a_reap_log() {
    assert_eq!(lease_id_of("sbx-01arz"), Some(LeaseId::new("01ARZ")));
    assert_eq!(lease_id_of("sbxn-abcdef"), None);
    assert_eq!(lease_id_of("not-ours"), None);
}

#[test]
fn a_tenant_value_becomes_an_atespace_only_when_valid() -> TestResult {
    assert_eq!(atespace_of("tenant-a"), Some(atespace()?));
    assert_eq!(atespace_of("Not A Tenant"), None);
    assert_eq!(REAP_INTERVAL, Duration::from_secs(300));
    Ok(())
}
