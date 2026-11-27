use std::time::{Duration, SystemTime};

use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use trogon_sandbox_contract::{LeaseCounters, LeaseId, LeaseRecord, LeaseStatus, TenantId};
use trogon_sandbox_protocol::subjects::LEASE_USAGE_KV_BUCKET;

use std::sync::Arc;

use super::*;
use trogon_sandbox_contract::ProviderKind;

fn sample_lease_record(id: &str, tenant: &str) -> Result<LeaseRecord, Box<dyn std::error::Error>> {
    Ok(LeaseRecord {
        epoch: None,
        entity_id: None,
        lease_id: LeaseId::new(id),
        tenant: TenantId::new(tenant)?,
        provider: ProviderKind::Mock,
        status: LeaseStatus::Running,
        created_at: SystemTime::now(),
        deadline: SystemTime::now() + Duration::from_secs(600),
        spec: crate::idle::tests::spec(60, None, None)?,
        sandbox_id: String::new(),
        cpu_seconds_consumed: 0,
        wall_seconds_consumed: 0,
        correlation_id: None,
        tags: Default::default(),
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

fn delta(commands: u64, out: u64) -> LeaseCounters {
    LeaseCounters {
        commands_run: commands,
        command_output_bytes: out,
        ..Default::default()
    }
}

#[tokio::test]
async fn adds_accumulate_and_stamp_activity() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryLeaseUsageStore::new();
    let lease = sample_lease_record("l1", "acme")?;
    let later = lease.created_at + Duration::from_secs(5);
    store.add(&lease, &delta(1, 10), later).await?;
    store.add(&lease, &delta(2, 5), later).await?;
    let entry = store.get(&lease.tenant, &lease.lease_id).await?.ok_or("not found")?;
    assert_eq!(entry.counters.commands_run, 3);
    assert_eq!(entry.counters.command_output_bytes, 15);
    assert_eq!(entry.last_activity_at, later);
    Ok(())
}
#[tokio::test]
async fn two_handles_on_one_store_see_the_same_figures() -> Result<(), Box<dyn std::error::Error>> {
    let store: Arc<dyn LeaseUsageStore> = Arc::new(InMemoryLeaseUsageStore::new());
    let other = Arc::clone(&store);
    let lease = sample_lease_record("l1", "acme")?;
    store.add(&lease, &delta(1, 0), SystemTime::now()).await?;
    other.add(&lease, &delta(1, 0), SystemTime::now()).await?;
    assert_eq!(
        store
            .get(&lease.tenant, &lease.lease_id)
            .await?
            .ok_or("not found")?
            .counters
            .commands_run,
        2
    );
    Ok(())
}

#[tokio::test]
async fn pause_time_is_excluded_from_running_time() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryLeaseUsageStore::new();
    let lease = sample_lease_record("l1", "acme")?;
    let t0 = lease.created_at;
    store.mark_paused(&lease, t0 + Duration::from_secs(10)).await?;
    // A repeat keeps the first start.
    store.mark_paused(&lease, t0 + Duration::from_secs(12)).await?;
    let entry = store.get(&lease.tenant, &lease.lease_id).await?.ok_or("not found")?;
    let mid = entry.to_usage(LeaseStatus::Paused, t0 + Duration::from_secs(30));
    assert_eq!(mid.paused_seconds, 20);
    assert_eq!(mid.running_seconds, 10);

    store.mark_resumed(&lease, t0 + Duration::from_secs(40)).await?;
    let entry = store.get(&lease.tenant, &lease.lease_id).await?.ok_or("not found")?;
    let after = entry.to_usage(LeaseStatus::Running, t0 + Duration::from_secs(50));
    assert_eq!(after.paused_seconds, 30);
    assert_eq!(after.running_seconds, 20);
    Ok(())
}

#[tokio::test]
async fn nats_kv_replicas_share_counters_without_losing_concurrent_adds() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let first = NatsKvLeaseUsageStore::new(jetstream::new(server.client.clone())).await?;
    let second = NatsKvLeaseUsageStore::new(jetstream::new(server.client.clone())).await?;
    let lease = sample_lease_record("lease:with:colons", "acme")?;
    let at = SystemTime::now();
    let d = delta(1, 2);
    let (a, b) = tokio::join!(first.add(&lease, &d, at), second.add(&lease, &d, at));
    a?;
    b?;
    first.finish(&lease, at).await?;
    for store in [&first, &second] {
        let entry = store.get(&lease.tenant, &lease.lease_id).await?.ok_or("not found")?;
        assert_eq!(entry.counters.commands_run, 2);
        assert_eq!(entry.counters.command_output_bytes, 4);
        assert_eq!(entry.ended_at, Some(at));
    }
    assert!(second.get(&lease.tenant, &LeaseId::new("other")).await?.is_none());
    Ok(())
}
#[tokio::test]
async fn nats_kv_lists_a_tenants_entries_including_ended_ones() -> Result<(), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvLeaseUsageStore::new(jetstream::new(server.client.clone())).await?;
    let live = sample_lease_record("lease:live", "acme")?;
    let ended = sample_lease_record("lease:ended", "acme")?;
    let foreign = sample_lease_record("lease:foreign", "other")?;
    let at = SystemTime::now();
    for lease in [&live, &ended, &foreign] {
        store.add(lease, &delta(1, 1), at).await?;
    }
    store.finish(&ended, at).await?;
    let mut found = store.tenant_entries(&TenantId::new("acme")?).await?;
    found.sort_by(|a, b| a.lease_id.as_str().cmp(b.lease_id.as_str()));
    let ids: Vec<_> = found.iter().map(|e| e.lease_id.as_str()).collect();
    assert_eq!(ids, ["lease:ended", "lease:live"]);
    assert_eq!(found[0].ended_at, Some(at));
    assert!(store.tenant_entries(&TenantId::new("nobody")?).await?.is_empty());
    Ok(())
}
/// Writes `lease`'s entry under the bare lease id, the way a broker from
/// before tenant-scoped keys did.
async fn write_legacy(
    store: &NatsKvLeaseUsageStore,
    lease: &LeaseRecord,
    commands: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entry = UsageEntry::new(lease);
    entry.counters.commands_run = commands;
    store
        .kv
        .put(legacy_kv_key(&lease.lease_id), Bytes::from(serde_json::to_vec(&entry)?))
        .await?;
    Ok(())
}
#[tokio::test]
async fn nats_kv_tenant_scan_skips_tenants_sharing_a_name_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = nats_store().await?;
    let mine = sample_lease_record("lease:mine", "acme")?;
    let near = sample_lease_record("lease:near", "acme_corp")?;
    let dotted = sample_lease_record("lease:dotted", "acme.corp")?;
    let legacy_mine = sample_lease_record("lease:old-mine", "acme")?;
    let legacy_foreign = sample_lease_record("lease:old-foreign", "other")?;
    let at = SystemTime::now();
    for lease in [&mine, &near, &dotted] {
        store.add(lease, &delta(1, 1), at).await?;
    }
    write_legacy(&store, &legacy_mine, 2).await?;
    write_legacy(&store, &legacy_foreign, 3).await?;
    let mut ids: Vec<_> = store
        .tenant_entries(&TenantId::new("acme")?)
        .await?
        .into_iter()
        .map(|e| e.lease_id.as_str().to_owned())
        .collect();
    ids.sort();
    assert_eq!(ids, ["lease:mine", "lease:old-mine"]);
    Ok(())
}
async fn nats_store() -> Result<(crate::test_support::LocalNats, NatsKvLeaseUsageStore), Box<dyn std::error::Error>> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvLeaseUsageStore::new(jetstream::new(server.client.clone())).await?;
    Ok((server, store))
}

#[tokio::test]
async fn nats_kv_writes_new_entries_under_a_tenant_scoped_key() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = nats_store().await?;
    let lease = sample_lease_record("lease:new", "acme")?;
    store.add(&lease, &delta(1, 1), SystemTime::now()).await?;
    let scoped = nats_kv_key(&lease.tenant, &lease.lease_id);
    assert!(scoped.starts_with(&nats_kv_tenant_prefix(&lease.tenant)));
    assert!(store.kv.entry(&scoped).await?.is_some());
    assert!(store.kv.entry(legacy_kv_key(&lease.lease_id)).await?.is_none());
    Ok(())
}
#[tokio::test]
async fn nats_kv_reads_legacy_entries_for_their_own_tenant_only() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = nats_store().await?;
    let lease = sample_lease_record("lease:old", "acme")?;
    write_legacy(&store, &lease, 5).await?;
    let got = store.get(&lease.tenant, &lease.lease_id).await?;
    assert_eq!(got.ok_or("not found")?.counters.commands_run, 5);
    let other = TenantId::new("other")?;
    assert!(store.get(&other, &lease.lease_id).await?.is_none());
    let found = store.tenant_entries(&lease.tenant).await?;
    assert_eq!(found.len(), 1);
    assert!(store.tenant_entries(&other).await?.is_empty());
    Ok(())
}
#[tokio::test]
async fn nats_kv_moves_a_legacy_entry_on_its_next_write() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = nats_store().await?;
    let lease = sample_lease_record("lease:old", "acme")?;
    write_legacy(&store, &lease, 5).await?;
    store.add(&lease, &delta(2, 3), SystemTime::now()).await?;
    let moved = store
        .kv
        .entry(nats_kv_key(&lease.tenant, &lease.lease_id))
        .await?
        .ok_or("not found")?;
    let moved: UsageEntry = serde_json::from_slice(&moved.value)?;
    assert_eq!(moved.counters.commands_run, 7, "the legacy figures carry over");
    assert!(
        store
            .kv
            .entry(legacy_kv_key(&lease.lease_id))
            .await?
            .is_none_or(|entry| entry.operation != kv::Operation::Put),
        "the legacy copy is removed"
    );
    Ok(())
}
#[tokio::test]
async fn nats_kv_counts_an_entry_present_under_both_keys_once() -> Result<(), Box<dyn std::error::Error>> {
    let (_server, store) = nats_store().await?;
    let lease = sample_lease_record("lease:both", "acme")?;
    store.add(&lease, &delta(4, 4), SystemTime::now()).await?;
    // An older broker that has not upgraded writes the bare key again.
    write_legacy(&store, &lease, 1).await?;
    let found = store.tenant_entries(&lease.tenant).await?;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].counters.commands_run, 4, "the scoped entry wins");
    let got = store.get(&lease.tenant, &lease.lease_id).await?;
    assert_eq!(got.ok_or("not found")?.counters.commands_run, 4);
    Ok(())
}
#[tokio::test]
async fn nats_kv_keeps_the_seven_day_retention() -> Result<(), Box<dyn std::error::Error>> {
    let (server, _store) = nats_store().await?;
    let info = jetstream::new(server.client.clone())
        .get_key_value(LEASE_USAGE_KV_BUCKET)
        .await?
        .status()
        .await?;
    assert_eq!(info.max_age(), USAGE_RETENTION);
    Ok(())
}

#[tokio::test]
async fn finish_freezes_the_figures() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryLeaseUsageStore::new();
    let lease = sample_lease_record("l1", "acme")?;
    let t0 = lease.created_at;
    store.add(&lease, &delta(1, 4), t0 + Duration::from_secs(1)).await?;
    store.finish(&lease, t0 + Duration::from_secs(60)).await?;
    // Late counts and a repeat finish change nothing.
    store.add(&lease, &delta(9, 9), t0 + Duration::from_secs(70)).await?;
    store.finish(&lease, t0 + Duration::from_secs(90)).await?;
    let entry = store.get(&lease.tenant, &lease.lease_id).await?.ok_or("not found")?;
    let usage = entry.to_usage(LeaseStatus::Released, t0 + Duration::from_secs(500));
    assert_eq!(usage.commands_run, 1);
    assert_eq!(usage.running_seconds, 60);
    assert_eq!(usage.ended_at, Some(t0 + Duration::from_secs(60)));
    Ok(())
}
