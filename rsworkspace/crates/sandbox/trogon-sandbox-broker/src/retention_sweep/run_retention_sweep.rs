use super::RetentionSweepConfig;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    Digest, RetentionPolicy, SnapshotDeleted, SnapshotTagAuditEvent, SnapshotTagRecord, TenantId,
};

use crate::bus::BrokerBus;
use crate::snapshot_catalog::lease_restore_sources;
use crate::snapshot_last_used::SnapshotLastUsedStore;
use crate::snapshot_tag::{SnapshotTagStore, read_tag_audit_history};

/// The configured tenants plus, when enabled, those the tag store knows.
pub(crate) async fn resolve_tenants(
    config: &RetentionSweepConfig,
    tag_store: &Arc<dyn SnapshotTagStore>,
) -> Vec<TenantId> {
    let mut tenants = config.tenants.clone();
    if config.discover_tenants {
        match tag_store.tenants().await {
            Ok(found) => tenants.extend(found),
            Err(e) => {
                warn!(error = %e, "retention sweep: failed to enumerate tenants from the tag store");
            }
        }
    }
    let mut seen = HashSet::new();
    tenants.retain(|t| seen.insert(t.as_str().to_owned()));
    tenants
}

/// Restore sources of live and named leases, or `None` when the lease set
/// cannot be read.
pub(crate) async fn live_restore_sources(config: &RetentionSweepConfig) -> Option<HashSet<Digest>> {
    let Some(store) = &config.active_leases else {
        return Some(HashSet::new());
    };
    match store.all().await {
        Ok(leases) => Some(lease_restore_sources(&leases)),
        Err(e) => {
            warn!(error = %e, "retention sweep: failed to read active leases; skipping this tick");
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_retention_sweep(
    tick: Duration,
    config: RetentionSweepConfig,
    tag_store: Arc<dyn SnapshotTagStore>,
    last_used_store: Arc<dyn SnapshotLastUsedStore>,
    jetstream: jetstream::Context,
    oci_client: Arc<trogon_sandbox_oci::OciClient>,
    bus: Arc<dyn BrokerBus>,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(tick);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("retention sweeper shutting down");
                return;
            }
            _ = interval.tick() => {
                sweep_once(&config, &tag_store, &last_used_store, &jetstream, &oci_client, &bus).await;
            }
        }
    }
}

pub(crate) async fn sweep_once(
    config: &RetentionSweepConfig,
    tag_store: &Arc<dyn SnapshotTagStore>,
    last_used_store: &Arc<dyn SnapshotLastUsedStore>,
    jetstream: &jetstream::Context,
    oci_client: &Arc<trogon_sandbox_oci::OciClient>,
    bus: &Arc<dyn BrokerBus>,
) {
    let now = SystemTime::now();
    let Some(restore_sources) = live_restore_sources(config).await else {
        return;
    };
    let tenants = resolve_tenants(config, tag_store).await;

    for tenant in &tenants {
        let tags = match tag_store.list(tenant).await {
            Ok(tags) => tags,
            Err(e) => {
                warn!(tenant = %tenant.as_str(), error = %e, "retention sweep: failed to list snapshot tags");
                continue;
            }
        };

        let mut pinned_digests: HashSet<Digest> = tags.iter().map(|(_, record)| record.digest.clone()).collect();
        pinned_digests.extend(restore_sources.iter().cloned());

        for (tag, record) in tags {
            let Some(policy) = record.retention else {
                continue;
            };

            let history = match read_tag_audit_history(jetstream.clone(), &tag.tenant, &tag.name).await {
                Ok(events) => events,
                Err(e) => {
                    warn!(
                        tenant = %tag.tenant.as_str(),
                        tag = %tag.name,
                        error = %e,
                        "retention sweep: failed to read tag audit history"
                    );
                    continue;
                }
            };

            // The tag's current live digest must never be evicted, even if it would
            // otherwise qualify by age or count, so it is filtered out of history here.
            let mut by_digest: Vec<(Digest, SystemTime)> = Vec::new();
            for event in &history {
                let SnapshotTagAuditEvent::Set { next, at, .. } = event else {
                    continue;
                };
                if *next == record.digest {
                    continue;
                }
                match by_digest.iter_mut().find(|(d, _)| d == next) {
                    Some((_, existing_at)) => {
                        if *at > *existing_at {
                            *existing_at = *at;
                        }
                    }
                    None => by_digest.push((next.clone(), *at)),
                }
            }

            // Retention age resets whenever a digest is used to create/resume a
            // sandbox (Vercel: expiration resets from last use), not only when a
            // tag was last set to point at it.
            let mut last_used: HashMap<Digest, SystemTime> = HashMap::new();
            for (digest, _) in &by_digest {
                match last_used_store.get(digest).await {
                    Ok(Some(at)) => {
                        last_used.insert(digest.clone(), at);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!(
                            tenant = %tag.tenant.as_str(),
                            tag = %tag.name,
                            digest = %digest,
                            error = %e,
                            "retention sweep: failed to read snapshot last-used timestamp"
                        );
                    }
                }
            }
            apply_last_used_resets(&mut by_digest, &last_used);

            let candidates = select_eviction_candidates(&record.digest, &by_digest, &policy, now);

            for candidate in candidates {
                if pinned_digests.contains(&candidate) {
                    continue;
                }
                evict_snapshot(&candidate, &tag.tenant, &record, config, oci_client, bus).await;
            }
        }
    }

    if let Some(catalog) = &config.catalog {
        // The catalog spans every tenant, so tag pins are read for every
        // tenant the store knows, not only the ones the tag pass was scoped to.
        // Any failure skips the pass rather than shrinking the protected set.
        let mut tenants = tenants;
        match tag_store.tenants().await {
            Ok(found) => tenants.extend(found),
            Err(e) => {
                warn!(error = %e, "catalog sweep: failed to enumerate tenants; skipping");
                return;
            }
        }
        let mut seen = HashSet::new();
        tenants.retain(|t| seen.insert(t.as_str().to_owned()));
        let mut tag_pinned: HashSet<Digest> = HashSet::new();
        for tenant in &tenants {
            match tag_store.list(tenant).await {
                Ok(tags) => tag_pinned.extend(tags.into_iter().map(|(_, r)| r.digest)),
                Err(e) => {
                    warn!(tenant = %tenant.as_str(), error = %e, "catalog sweep: failed to list snapshot tags; skipping");
                    return;
                }
            }
        }
        catalog.sweep_once(restore_sources, tag_pinned, now).await;
    }
}

pub(crate) async fn evict_snapshot(
    candidate: &Digest,
    tenant: &TenantId,
    record: &SnapshotTagRecord,
    config: &RetentionSweepConfig,
    oci_client: &Arc<trogon_sandbox_oci::OciClient>,
    bus: &Arc<dyn BrokerBus>,
) {
    if !record
        .retention
        .as_ref()
        .is_some_and(|policy| policy.delete_evicted.deletes())
    {
        return;
    }
    let Some(remover) = &config.remover else {
        warn!(tenant = %tenant.as_str(), digest = %candidate, "retention deletion has no host accounting authority");
        return;
    };
    let (bytes, generation) = match remover.remove(tenant, candidate).await {
        Ok(crate::snapshot_tracker::SnapshotRemoval::Removed {
            bytes,
            generation: Some(generation),
        }) => (bytes, generation),
        Ok(_) => return,
        Err(error) => {
            warn!(tenant = %tenant.as_str(), digest = %candidate, %error, "retention deletion remains pending");
            return;
        }
    };
    let event = SnapshotDeleted {
        digest: candidate.clone(),
        tenant: tenant.clone(),
        bytes,
        generation: Some(generation),
    };
    if let Ok(payload) = serde_json::to_vec(&event)
        && let Err(error) = bus
            .publish(
                trogon_sandbox_protocol::subjects::SNAPSHOT_DELETED,
                Bytes::from(payload),
            )
            .await
    {
        warn!(tenant = %tenant.as_str(), digest = %candidate, %error, "retention deletion notification remains pending");
    }
    let _ = oci_client;
}

/// Takes the more recent of a digest's tag-audit timestamp and its last-used
/// timestamp, so a digest that keeps getting leased directly (bypassing tag
/// resolution) does not age out of retention.
pub(crate) fn apply_last_used_resets(by_digest: &mut [(Digest, SystemTime)], last_used: &HashMap<Digest, SystemTime>) {
    for (digest, at) in by_digest.iter_mut() {
        if let Some(used_at) = last_used.get(digest)
            && *used_at > *at
        {
            *at = *used_at;
        }
    }
}

pub(crate) fn select_eviction_candidates(
    current_digest: &Digest,
    history: &[(Digest, SystemTime)],
    policy: &RetentionPolicy,
    now: SystemTime,
) -> Vec<Digest> {
    let mut sorted: Vec<&(Digest, SystemTime)> =
        history.iter().filter(|(digest, _)| digest != current_digest).collect();
    sorted.sort_by_key(|entry| std::cmp::Reverse(entry.1));

    let keep_by_count = policy.max_tags.get().saturating_sub(1) as usize;
    let max_age = Duration::from_secs(u64::from(policy.max_age_days) * 24 * 60 * 60);

    sorted
        .into_iter()
        .enumerate()
        .filter_map(|(idx, (digest, at))| {
            let within_count = idx < keep_by_count;
            let age = now.duration_since(*at).unwrap_or(Duration::ZERO);
            let within_age = age <= max_age;
            if within_count && within_age {
                None
            } else {
                Some(digest.clone())
            }
        })
        .collect()
}
