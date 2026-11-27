fn test_usage() -> Arc<dyn crate::usage_store::LeaseUsageStore> {
    Arc::new(crate::usage_store::InMemoryLeaseUsageStore::new())
}

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{
    LeaseId, LeaseRecord, LeaseReleased, LeaseReleasedReason, LeaseStatus, ProviderKind, SandboxTags, TenantId,
};

use super::sweep_once;
use crate::active_lease::ActiveLeaseStore;
use crate::active_lease::InMemoryActiveLeaseStore;
use crate::bus::BrokerBus;
use crate::idle::tests::spec;
use crate::idle::{IdleAction, IdlePauser};
use crate::metrics::BrokerMetrics;
use crate::quota::{InMemoryQuotaStore, QuotaStore};

type Published = Arc<Mutex<Vec<(String, Bytes)>>>;

struct CaptureBus {
    published: Published,
}

impl CaptureBus {
    fn new() -> (Self, Published) {
        let published = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                published: Arc::clone(&published),
            },
            published,
        )
    }
}

#[async_trait]
impl BrokerBus for CaptureBus {
    async fn request(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        Ok(Bytes::new())
    }

    async fn publish(&self, subject: &str, payload: Bytes) -> anyhow::Result<()> {
        self.published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((subject.to_string(), payload));
        Ok(())
    }
}

fn sample_record(
    id: &str,
    tenant: &str,
    deadline: SystemTime,
    cpu: u32,
    wall: u32,
) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: LeaseId::new(id),
        tenant: TenantId::new(tenant)?,
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: SystemTime::now(),
        deadline,
        spec: spec(cpu, None, None)?,
        sandbox_id: format!("sandbox-{id}"),
        cpu_seconds_consumed: cpu,
        wall_seconds_consumed: wall,
        correlation_id: None,
        tags: SandboxTags::default(),
        name: None,
        worker_id: None,
        dispatch_attempt: None,
        quota_reservation: None,
        last_activity_at: None,
        paused_remaining_seconds: None,
        freeze_in_flight: None,
        auto_paused_at: None,
        idle_pause_retry: None,
        dormancy: None,
        restored_from: None,
    })
}

#[tokio::test]
async fn grant_stores_record_with_consumption() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryActiveLeaseStore::new();
    let record = sample_record(
        "lease-sweep-1",
        "tenant-a",
        SystemTime::now() + Duration::from_secs(300),
        60,
        300,
    )?;
    store.put(&record).await?;

    let got = store.get(&LeaseId::new("lease-sweep-1")).await?.ok_or("not found")?;
    assert_eq!(got.cpu_seconds_consumed, 60);
    assert_eq!(got.wall_seconds_consumed, 300);
    Ok(())
}
#[tokio::test]
async fn normal_cancel_deletes_record_and_refunds() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let quota = Arc::new(InMemoryQuotaStore::new());

    // Pre-charge quota
    quota
        .check_and_reserve(&TenantId::new("t")?, 60, 300, 3600, 7200)
        .await?;

    let record = sample_record(
        "lease-cancel",
        "t",
        SystemTime::now() + Duration::from_secs(300),
        60,
        300,
    )?;
    store.put(&record).await?;

    // Release
    let deleted = store.delete(&LeaseId::new("lease-cancel")).await?;
    assert!(deleted);
    quota.release(&TenantId::new("t")?, 60, 300).await?;

    // Second cancel is a no-op
    let deleted2 = store.delete(&LeaseId::new("lease-cancel")).await?;
    assert!(!deleted2);
    Ok(())
}
#[tokio::test]
async fn sweeper_reclaims_expired_lease_and_publishes_event() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let quota: Arc<dyn QuotaStore> = Arc::new(InMemoryQuotaStore::new());
    let (bus, published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);

    // Pre-charge quota
    quota
        .check_and_reserve(&TenantId::new("t")?, 60, 300, 3600, 7200)
        .await?;

    // Insert an already-expired record
    let record = sample_record(
        "lease-expired",
        "t",
        SystemTime::now() - Duration::from_secs(1),
        60,
        300,
    )?;
    store.put(&record).await?;

    // Run one sweep tick
    sweep_once(
        &(store.clone() as Arc<dyn ActiveLeaseStore>),
        &quota,
        &bus,
        &BrokerMetrics::new()?,
        &test_usage(),
        &(Arc::new(crate::idle::NoIdlePause) as Arc<dyn IdlePauser>),
    )
    .await;

    // Record must be gone
    let got = store.get(&LeaseId::new("lease-expired")).await?;
    assert!(got.is_none());

    // A lease_released event with reason=expired must have been published
    let msgs = published.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let released_msg = msgs
        .iter()
        .find(|(subj, _)| subj.contains("sandbox.lease.released.lease-expired"));
    assert!(released_msg.is_some(), "lease_released event not published");

    let (_, payload) = released_msg.ok_or("released message missing")?;
    let event: LeaseReleased = serde_json::from_slice(payload)?;
    assert_eq!(event.reason, LeaseReleasedReason::Expired);
    assert_eq!(event.lease_id.as_str(), "lease-expired");
    Ok(())
}
#[tokio::test]
async fn sweeper_ignores_non_expired_lease() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let quota: Arc<dyn QuotaStore> = Arc::new(InMemoryQuotaStore::new());
    let (bus, published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);

    let record = sample_record(
        "lease-active",
        "t",
        SystemTime::now() + Duration::from_secs(999),
        60,
        300,
    )?;
    store.put(&record).await?;

    sweep_once(
        &(store.clone() as Arc<dyn ActiveLeaseStore>),
        &quota,
        &bus,
        &BrokerMetrics::new()?,
        &test_usage(),
        &(Arc::new(crate::idle::NoIdlePause) as Arc<dyn IdlePauser>),
    )
    .await;

    // Record should still be present
    let got = store.get(&LeaseId::new("lease-active")).await?;
    assert!(got.is_some());

    // No publish
    let msgs = published.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(msgs.is_empty());
    Ok(())
}
#[tokio::test]
async fn concurrent_sweepers_refund_an_expired_lease_once() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let quota: Arc<dyn QuotaStore> = Arc::new(InMemoryQuotaStore::new());
    let (bus, _published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);
    let tenant = TenantId::new("t")?;

    // Two leases hold a slot; only one expires, so a double decrement
    // would drive the counter to zero.
    for _ in 0..2 {
        quota.check_and_reserve(&tenant, 60, 300, 3600, 7200).await?;
        quota.increment_active_leases(&tenant).await?;
    }
    store
        .put(&sample_record(
            "lease-expired",
            "t",
            SystemTime::now() - Duration::from_secs(1),
            60,
            300,
        )?)
        .await?;

    let store: Arc<dyn ActiveLeaseStore> = store;
    let usage = test_usage();
    let metrics = BrokerMetrics::new()?;
    let no_pause: Arc<dyn IdlePauser> = Arc::new(crate::idle::NoIdlePause);
    tokio::join!(
        sweep_once(&store, &quota, &bus, &metrics, &usage, &no_pause),
        sweep_once(&store, &quota, &bus, &metrics, &usage, &no_pause),
        sweep_once(&store, &quota, &bus, &metrics, &usage, &no_pause),
    );

    assert_eq!(quota.active_lease_count(&tenant).await?, 1);
    Ok(())
}
fn idle_record(id: &str, idle_seconds: u32, quiet_for: Duration) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    let mut record = sample_record(id, "t", SystemTime::now() + Duration::from_secs(999), 60, 300)?;
    record.spec = spec(60, Some(idle_seconds), None)?;
    record.created_at = SystemTime::now() - quiet_for;
    Ok(record)
}

async fn sweep(store: &Arc<InMemoryActiveLeaseStore>) -> Result<Published, Box<dyn std::error::Error>> {
    let quota: Arc<dyn QuotaStore> = Arc::new(InMemoryQuotaStore::new());
    let (bus, published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);
    sweep_once(
        &(store.clone() as Arc<dyn ActiveLeaseStore>),
        &quota,
        &bus,
        &BrokerMetrics::new()?,
        &test_usage(),
        &(Arc::new(crate::idle::NoIdlePause) as Arc<dyn IdlePauser>),
    )
    .await;
    Ok(published)
}

/// Records which leases it was asked to idle-pause.
struct RecordingPauser {
    can_pause: bool,
    paused: Mutex<Vec<String>>,
}

#[async_trait]
impl IdlePauser for RecordingPauser {
    async fn can_pause(&self, _provider: ProviderKind) -> bool {
        self.can_pause
    }
    async fn pause_idle(&self, record: &LeaseRecord) -> bool {
        self.paused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(record.lease_id.as_str().to_owned());
        true
    }
}

fn pause_chain_record(id: &str, quiet_for: Duration) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    let mut record = idle_record(id, 30, quiet_for)?;
    record.spec = spec(60, None, Some(&[(60, IdleAction::Pause), (3600, IdleAction::Release)]))?;
    Ok(record)
}

async fn sweep_with(
    store: &Arc<InMemoryActiveLeaseStore>,
    pauser: Arc<dyn IdlePauser>,
) -> Result<Published, Box<dyn std::error::Error>> {
    let quota: Arc<dyn QuotaStore> = Arc::new(InMemoryQuotaStore::new());
    let (bus, published) = CaptureBus::new();
    let bus: Arc<dyn BrokerBus> = Arc::new(bus);
    sweep_once(
        &(store.clone() as Arc<dyn ActiveLeaseStore>),
        &quota,
        &bus,
        &BrokerMetrics::new()?,
        &test_usage(),
        &pauser,
    )
    .await;
    Ok(published)
}

#[tokio::test]
async fn sweeper_pauses_a_legacy_lease_past_its_pause_stage_and_keeps_it() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let record = pause_chain_record("lease-pause", Duration::from_secs(120))?;
    store.put(&record).await?;
    let pauser = Arc::new(RecordingPauser {
        can_pause: true,
        paused: Mutex::new(Vec::new()),
    });

    let published = sweep_with(&store, pauser.clone()).await?;

    assert_eq!(
        *pauser.paused.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        vec!["lease-pause"]
    );
    assert!(store.get(&record.lease_id).await?.is_some());
    assert!(
        published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty(),
        "a pause is not a release"
    );
    Ok(())
}
#[tokio::test]
async fn sweeper_skips_the_pause_stage_when_the_provider_cannot_pause() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let record = pause_chain_record("lease-no-pause", Duration::from_secs(120))?;
    store.put(&record).await?;
    let pauser = Arc::new(RecordingPauser {
        can_pause: false,
        paused: Mutex::new(Vec::new()),
    });

    sweep_with(&store, pauser.clone()).await?;

    assert!(
        pauser
            .paused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    assert!(store.get(&record.lease_id).await?.is_some());
    Ok(())
}
#[tokio::test]
async fn sweeper_leaves_a_legacy_lease_before_its_pause_stage() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let record = pause_chain_record("lease-fresh", Duration::from_secs(10))?;
    store.put(&record).await?;
    let pauser = Arc::new(RecordingPauser {
        can_pause: true,
        paused: Mutex::new(Vec::new()),
    });

    sweep_with(&store, pauser.clone()).await?;

    assert!(
        pauser
            .paused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn sweeper_reclaims_an_idle_lease_with_a_distinct_reason() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let record = idle_record("lease-idle", 30, Duration::from_secs(120))?;
    store.put(&record).await?;

    let published = sweep(&store).await?;

    assert!(store.get(&record.lease_id).await?.is_none());
    let msgs = published.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_, payload) = msgs
        .iter()
        .find(|(subj, _)| subj.contains("sandbox.lease.released.lease-idle"))
        .ok_or("no release published")?;
    let event: LeaseReleased = serde_json::from_slice(payload)?;
    assert_eq!(event.reason, LeaseReleasedReason::IdleTimeout);
    Ok(())
}
#[tokio::test]
async fn sweeper_keeps_a_lease_with_recent_activity() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let record = idle_record("lease-busy", 30, Duration::from_secs(120))?;
    store.put(&record).await?;
    store.touch(&record.lease_id, SystemTime::now()).await?;

    let published = sweep(&store).await?;

    assert!(store.get(&record.lease_id).await?.is_some());
    assert!(
        published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    Ok(())
}
#[tokio::test]
async fn sweeper_ignores_inactivity_on_a_lease_without_an_idle_timeout() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let mut record = idle_record("lease-no-idle", 30, Duration::from_secs(120))?;
    record.spec = spec(60, None, None)?;
    store.put(&record).await?;

    sweep(&store).await?;

    assert!(store.get(&record.lease_id).await?.is_some());
    Ok(())
}
#[tokio::test]
async fn sweeper_keeps_the_expired_reason_when_both_deadline_and_idle_passed() -> Result<(), Box<dyn std::error::Error>>
{
    let store = Arc::new(InMemoryActiveLeaseStore::new());
    let mut record = idle_record("lease-both", 30, Duration::from_secs(120))?;
    record.deadline = SystemTime::now() - Duration::from_secs(1);
    store.put(&record).await?;

    let published = sweep(&store).await?;

    let msgs = published.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_, payload) = msgs
        .iter()
        .find(|(subj, _)| subj.contains("sandbox.lease.released.lease-both"))
        .ok_or("no release published")?;
    let event: LeaseReleased = serde_json::from_slice(payload)?;
    assert_eq!(event.reason, LeaseReleasedReason::Expired);
    Ok(())
}
