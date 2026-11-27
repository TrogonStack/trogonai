//! Mirror of named persistent leases into the entity store.
//!
//! In `shadow` mode the legacy name index and active-lease store stay
//! authoritative for every response: the best-effort hooks swallow their error
//! after logging a warning. In `on` mode the entity is the source of truth, so
//! the calls that precede a lease (`begin_start`, `adopt_running`, name
//! lookup, delete) return their error for the caller to surface. The module is only constructed when
//! `SANDBOX_ENTITY_MODE` is `shadow` or `on`, so with `off` no bucket or
//! stream is created and no code path reaches it.
//!
//! Ordering (NATS KV has no transactions): the entity record is written to
//! `Starting{lease}` before the lease is dispatched, advanced to
//! `Running{lease}` once the grant exists, and moved to `Stopped` on release
//! only when it is still bound to that lease. [`EntityShadow::repair`]
//! reconciles whatever a crash left between those writes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tracing::{debug, warn};
use trogon_sandbox_contract::{
    Digest, LeaseRecord, LeaseStatus, SandboxSpec, SandboxTags, SnapshotPublished, SnapshotTag, SnapshotTagName,
};
use trogon_sandbox_contract::{
    EntityLifecycleEvent, EntityState, LeaseId, SandboxEntityId, SandboxEntityRecord, SnapshotHead, TenantId,
};
use trogon_sandbox_protocol::subjects::ENTITY_EVENT_SUBJECT_PREFIX;

mod entity_event_sink;
mod in_memory_entity_event_sink;
mod jet_stream_entity_event_sink;
mod noop_entity_event_sink;
mod repair_report;

pub use entity_event_sink::EntityEventSink;
pub use in_memory_entity_event_sink::InMemoryEntityEventSink;
pub use jet_stream_entity_event_sink::JetStreamEntityEventSink;
pub use noop_entity_event_sink::NoopEntityEventSink;
pub use repair_report::RepairReport;

mod intent;
pub use intent::{Intent, Recorded};

use crate::active_lease::ActiveLeaseStore;
use crate::entity_store::{EntityStoreError, NewSandboxEntity, SandboxEntityMode, SandboxEntityStore, Versioned};
use crate::snapshot_tag::SnapshotTagStore;
use trogon_sandbox_protocol::{EntityEvent, apply_entity_event};

pub use crate::entity_event_sink_error::EntityEventSinkError;

/// `sandbox.entity.event.{hex(tenant)}.{ulid}`.
pub fn entity_event_subject(tenant: &TenantId, id: &SandboxEntityId) -> String {
    format!(
        "{ENTITY_EVENT_SUBJECT_PREFIX}.{}.{id}",
        hex::encode(tenant.as_str().as_bytes())
    )
}

pub struct EntityShadow {
    mode: SandboxEntityMode,
    store: Arc<dyn SandboxEntityStore>,
    events: Arc<dyn EntityEventSink>,
    /// How long an entity may sit unchanged next to a missing lease before the
    /// sweep returns it to `Stopped`. Must exceed the dispatch timeout.
    pending_timeout: Duration,
    head_poll_interval: Duration,
    head_poll_attempts: u32,
    /// Revision each bound entity was last seen at, and since when.
    observed: Mutex<HashMap<SandboxEntityId, (u64, Instant)>>,
}

impl EntityShadow {
    pub fn new(
        mode: SandboxEntityMode,
        store: Arc<dyn SandboxEntityStore>,
        events: Arc<dyn EntityEventSink>,
        pending_timeout: Duration,
    ) -> Self {
        Self {
            mode,
            store,
            events,
            pending_timeout,
            head_poll_interval: Duration::from_millis(500),
            head_poll_attempts: 20,
            observed: Mutex::new(HashMap::new()),
        }
    }

    /// How long and how often to wait for the snapshot builder to re-point the
    /// persistence tag after a release.
    pub fn with_head_poll(mut self, interval: Duration, attempts: u32) -> Self {
        self.head_poll_interval = interval;
        self.head_poll_attempts = attempts;
        self
    }

    pub fn mode(&self) -> SandboxEntityMode {
        self.mode
    }

    /// In `On` mode the entity is the source of truth, so its failures fail
    /// the request. In `Shadow` mode they are only logged.
    pub fn is_authoritative(&self) -> bool {
        self.mode == SandboxEntityMode::On
    }

    pub fn store(&self) -> &Arc<dyn SandboxEntityStore> {
        &self.store
    }

    fn note<T>(&self, op: &str, result: Result<T, EntityStoreError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                warn!(error = %error, op, "sandbox entity shadow write failed; lease response unaffected");
                None
            }
        }
    }

    async fn publish(&self, event: EntityLifecycleEvent) {
        if let Err(error) = self.events.publish(&event).await {
            warn!(error = %error, entity_id = %event.entity_id(), "sandbox entity event publish failed");
        }
    }

    /// Get or create the entity for a named sandbox, provisioned to `Stopped`.
    /// A name with no entity yet is the lazy-migration case: its persistence
    /// tag is the name itself, so an existing tag-only sandbox keeps its state.
    async fn ensure(
        &self,
        tenant: &TenantId,
        name: &str,
        spec: &SandboxSpec,
        tags: &SandboxTags,
    ) -> Result<Versioned, EntityStoreError> {
        for _ in 0..2 {
            if let Some(found) = self.store.get_by_name(tenant, name).await? {
                if found.record.state == EntityState::Creating {
                    return self
                        .store
                        .apply_event(tenant, &found.record.entity_id, &EntityEvent::Provisioned)
                        .await;
                }
                return Ok(found);
            }
            let persistence_tag = SnapshotTag {
                tenant: tenant.clone(),
                name: SnapshotTagName::new(name).map_err(|e| EntityStoreError::InvalidArgument(e.to_string()))?,
            };
            let created = match self
                .store
                .create(NewSandboxEntity {
                    tenant: tenant.clone(),
                    name: Some(name.to_string()),
                    created_by: None,
                    spec: spec.clone(),
                    metadata: tags.clone(),
                    drives: spec.drives().to_vec(),
                    persistence_tag: Some(persistence_tag),
                })
                .await
            {
                Ok(created) => created,
                // Lost the race to another replica; read the winner.
                Err(EntityStoreError::NameTaken) => continue,
                Err(other) => return Err(other),
            };
            self.publish(EntityLifecycleEvent::Created {
                entity_id: created.record.entity_id.clone(),
                tenant: tenant.clone(),
                name: Some(name.to_string()),
                at: SystemTime::now(),
            })
            .await;
            return self
                .store
                .apply_event(tenant, &created.record.entity_id, &EntityEvent::Provisioned)
                .await;
        }
        Err(EntityStoreError::NameTaken)
    }

    /// A named request is about to provision `lease`. Binds the entity to it in
    /// `Starting`, ending any stale binding first: the legacy name claim was
    /// just won, so whichever lease the entity still points at is not live.
    pub async fn begin_start(
        &self,
        tenant: &TenantId,
        name: &str,
        lease: &LeaseId,
        spec: &SandboxSpec,
        tags: &SandboxTags,
        from_digest: Option<&Digest>,
    ) -> Result<SandboxEntityId, EntityStoreError> {
        let found = self.ensure(tenant, name, spec, tags).await?;
        let id = found.record.entity_id.clone();
        let started = self
            .store
            .mutate(tenant, &id, &|record| {
                if record.state.lease() == Some(lease) {
                    return Ok(());
                }
                if let Some(stale) = record.state.lease().cloned() {
                    apply_entity_event(&mut *record, &EntityEvent::RunEnded { lease: stale })?;
                }
                apply_entity_event(&mut *record, &EntityEvent::StartRequested { lease: lease.clone() })?;
                Ok(())
            })
            .await?;
        if started.record.generation != found.record.generation {
            self.publish(EntityLifecycleEvent::RunStarted {
                entity_id: id.clone(),
                tenant: tenant.clone(),
                lease: lease.clone(),
                generation: started.record.generation,
                from_digest: from_digest.cloned(),
                at: SystemTime::now(),
            })
            .await;
        }
        Ok(id)
    }

    /// The grant for `lease` exists.
    pub async fn lease_ready(&self, tenant: &TenantId, id: &SandboxEntityId, lease: &LeaseId) {
        let result = self
            .store
            .apply_event(tenant, id, &EntityEvent::LeaseReady { lease: lease.clone() })
            .await;
        self.note("lease_ready", result);
    }

    /// The request that began `lease` was denied; free the entity.
    pub async fn start_failed(&self, tenant: &TenantId, name: &str, lease: &LeaseId) {
        let Some(found) = self.note("start_failed", self.store.get_by_name(tenant, name).await) else {
            return;
        };
        let Some(found) = found else { return };
        if found.record.state.lease() != Some(lease) {
            return;
        }
        let result = self
            .store
            .apply_event(
                tenant,
                &found.record.entity_id,
                &EntityEvent::RunEnded { lease: lease.clone() },
            )
            .await;
        self.note("start_failed", result);
    }

    /// A named request resolved to a lease that was already running (reconnect,
    /// or a lost name claim). Creates the entity if this is its first sight of
    /// the name and makes it `Running` on that lease, copying the lease's
    /// metadata: the lazy-migration case for a sandbox that predates the mode.
    pub async fn adopt_running(
        &self,
        tenant: &TenantId,
        name: &str,
        record: &LeaseRecord,
    ) -> Result<SandboxEntityId, EntityStoreError> {
        let lease = &record.lease_id;
        let found = self.ensure(tenant, name, &record.spec, &record.tags).await?;
        let id = found.record.entity_id.clone();
        let adopted = self
            .store
            .mutate(tenant, &id, &|entity| {
                if entity.state.lease() == Some(lease) {
                    if matches!(entity.state, EntityState::Starting { .. }) {
                        apply_entity_event(&mut *entity, &EntityEvent::LeaseReady { lease: lease.clone() })?;
                    }
                    return Ok(());
                }
                if let Some(stale) = entity.state.lease().cloned() {
                    apply_entity_event(&mut *entity, &EntityEvent::RunEnded { lease: stale })?;
                }
                apply_entity_event(&mut *entity, &EntityEvent::StartRequested { lease: lease.clone() })?;
                apply_entity_event(&mut *entity, &EntityEvent::LeaseReady { lease: lease.clone() })?;
                entity.metadata = record.tags.clone();
                Ok(())
            })
            .await?;
        if adopted.record.generation != found.record.generation {
            self.publish(EntityLifecycleEvent::RunStarted {
                entity_id: id.clone(),
                tenant: tenant.clone(),
                lease: lease.clone(),
                generation: adopted.record.generation,
                from_digest: None,
                at: SystemTime::now(),
            })
            .await;
        }
        Ok(id)
    }

    /// `record`'s lease ended. Moves the entity to `Stopped` only while it is
    /// still bound to that lease, so a late release cannot stop a newer run,
    /// then waits (bounded, in the background) for the snapshot builder to
    /// re-point the persistence tag and records that digest as the head.
    pub async fn run_ended(self: &Arc<Self>, record: &LeaseRecord, tag_store: Arc<dyn SnapshotTagStore>, reason: &str) {
        let tenant = &record.tenant;
        let id = match &record.entity_id {
            Some(id) => id.clone(),
            None => {
                let Some(name) = &record.name else { return };
                match self.store.get_by_name(tenant, name.as_str()).await {
                    Ok(Some(found)) => found.record.entity_id,
                    Ok(None) => return,
                    Err(error) => {
                        warn!(error = %error, "sandbox entity lookup on release failed");
                        return;
                    }
                }
            }
        };
        let lease = &record.lease_id;
        let ended = match self
            .store
            .mutate(tenant, &id, &|entity| {
                if entity.state.lease() != Some(lease) {
                    return Err(EntityStoreError::InvalidArgument(
                        "entity is not bound to this lease".into(),
                    ));
                }
                apply_entity_event(&mut *entity, &EntityEvent::RunEnded { lease: lease.clone() })?;
                Ok(())
            })
            .await
        {
            Ok(ended) => ended,
            Err(error) => {
                debug!(error = %error, "sandbox entity not stopped by this release");
                return;
            }
        };
        let baseline = match tag_store.get(&ended.record.persistence_tag).await {
            Ok(tag) => tag.map(|t| t.digest),
            Err(error) => {
                warn!(error = %error, "persistence tag read on release failed");
                None
            }
        };
        let shadow = Arc::clone(self);
        let tenant = tenant.clone();
        let lease = lease.clone();
        let generation = ended.record.generation;
        let reason = reason.to_string();
        let tag = ended.record.persistence_tag.clone();
        tokio::spawn(async move {
            let captured = shadow.await_capture(&tag_store, &tag, baseline.as_ref()).await;
            if let Some(tag_record) = &captured {
                let head = SnapshotHead {
                    digest: tag_record.digest.clone(),
                    artifact_type: tag_record.artifact_type,
                    captured_at: tag_record.set_at,
                    from_lease: lease.clone(),
                };
                let result = shadow
                    .store
                    .mutate(&tenant, &id, &|entity| {
                        // A newer run may already own the entity; its own
                        // release records its own head.
                        if entity.generation != generation {
                            return Ok(());
                        }
                        entity.head = Some(head.clone());
                        Ok(())
                    })
                    .await;
                shadow.note("record_head", result);
            }
            shadow
                .publish(EntityLifecycleEvent::RunEnded {
                    entity_id: id,
                    tenant,
                    lease,
                    generation,
                    reason,
                    captured_digest: captured.map(|t| t.digest),
                    at: SystemTime::now(),
                })
                .await;
        });
    }

    /// A snapshot was published with the tag the builder re-pointed. When that
    /// is an entity's persistence tag, records the snapshot as its head without
    /// waiting on the poll after release, which a slow capture outlasts. A head
    /// that is already as new is left alone, so a repeat or a late event from
    /// an older run changes nothing.
    pub async fn snapshot_published(&self, event: &SnapshotPublished) {
        let Some(published) = &event.tag else { return };
        let tag = SnapshotTag {
            tenant: event.tenant.clone(),
            name: published.name.clone(),
        };
        let found = match self.store.get_by_name(&event.tenant, published.name.as_str()).await {
            Ok(Some(found)) if found.record.persistence_tag == tag => found,
            Ok(_) => return,
            Err(error) => {
                warn!(error = %error, "sandbox entity lookup for a published snapshot failed");
                return;
            }
        };
        let head = SnapshotHead {
            digest: event.digest.clone(),
            artifact_type: event.artifact_type,
            captured_at: published.set_at,
            from_lease: event.lease_id.clone(),
        };
        let result = self
            .store
            .mutate(&event.tenant, &found.record.entity_id, &|entity| {
                if entity
                    .head
                    .as_ref()
                    .is_some_and(|current| current.captured_at >= head.captured_at)
                {
                    return Ok(());
                }
                entity.head = Some(head.clone());
                Ok(())
            })
            .await;
        self.note("record_head_from_event", result);
    }

    async fn await_capture(
        &self,
        tag_store: &Arc<dyn SnapshotTagStore>,
        tag: &SnapshotTag,
        baseline: Option<&Digest>,
    ) -> Option<trogon_sandbox_contract::SnapshotTagRecord> {
        for attempt in 0..self.head_poll_attempts.max(1) {
            if attempt > 0 {
                tokio::time::sleep(self.head_poll_interval).await;
            }
            match tag_store.get(tag).await {
                Ok(Some(found)) if Some(&found.digest) != baseline => return Some(found),
                Ok(_) => {}
                Err(error) => warn!(error = %error, "persistence tag poll failed"),
            }
        }
        None
    }

    /// The lease the entity named `name` is bound to. The outer `None` means
    /// the tenant has no such entity (a sandbox that predates the mode); the
    /// inner `None` means it exists and is not running.
    pub async fn current_lease_by_name(
        &self,
        tenant: &TenantId,
        name: &str,
    ) -> Result<Option<Option<LeaseId>>, EntityStoreError> {
        Ok(self
            .store
            .get_by_name(tenant, name)
            .await?
            .map(|found| found.record.current_lease))
    }

    /// Drops the entity and its name claim. `Ok(false)` when it did not exist.
    pub async fn delete_entity(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<bool, EntityStoreError> {
        match self.store.delete(tenant, id).await {
            Ok(()) => {
                self.publish(EntityLifecycleEvent::Deleted {
                    entity_id: id.clone(),
                    tenant: tenant.clone(),
                    at: SystemTime::now(),
                })
                .await;
                Ok(true)
            }
            Err(EntityStoreError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The named sandbox was deleted; drop its entity and name claim.
    /// `Ok(false)` when the name had no entity.
    pub async fn delete_named(&self, tenant: &TenantId, name: &str) -> Result<bool, EntityStoreError> {
        match self.store.get_by_name(tenant, name).await? {
            Some(found) => self.delete_entity(tenant, &found.record.entity_id).await,
            None => Ok(false),
        }
    }

    /// Moves `record` to `Stopped` because its bound `lease` is gone, and
    /// publishes the end of the run. `false` when the entity no longer sits
    /// on that lease (a newer writer got there first) or the write failed.
    pub async fn end_run(&self, record: &SandboxEntityRecord, lease: &LeaseId, reason: &str) -> bool {
        let ended = self
            .store
            .apply_event(
                &record.tenant,
                &record.entity_id,
                &EntityEvent::RunEnded { lease: lease.clone() },
            )
            .await;
        let Some(ended) = self.note("end_run", ended) else {
            return false;
        };
        self.publish(EntityLifecycleEvent::RunEnded {
            entity_id: record.entity_id.clone(),
            tenant: record.tenant.clone(),
            lease: lease.clone(),
            generation: ended.record.generation,
            reason: reason.into(),
            captured_digest: None,
            at: SystemTime::now(),
        })
        .await;
        true
    }

    /// One repair pass at `now`.
    ///
    /// - An entity bound to a lease whose record is gone (or released) and
    ///   that has not changed for `pending_timeout` returns to `Stopped`. This
    ///   covers a crash between the `Starting` write and the lease write, and
    ///   releases that never reached the hook (expiry, reclamation).
    /// - An entity still `Starting` next to a live lease advances to `Running`.
    /// - A name claim whose entity is gone is removed.
    pub async fn repair(&self, active: &dyn ActiveLeaseStore, now: Instant) -> RepairReport {
        let mut report = RepairReport::default();
        let entities = match self.store.list_all().await {
            Ok(entities) => entities,
            Err(error) => {
                warn!(error = %error, "sandbox entity repair could not list entities");
                return report;
            }
        };
        let mut seen = HashMap::new();
        for found in entities {
            let record = &found.record;
            let Some(lease) = record.state.lease().cloned() else {
                continue;
            };
            let since = {
                let observed = self.observed.lock().unwrap_or_else(|e| e.into_inner());
                match observed.get(&record.entity_id) {
                    Some((revision, since)) if *revision == found.revision => *since,
                    _ => now,
                }
            };
            seen.insert(record.entity_id.clone(), (found.revision, since));
            let live = match active.get(&lease).await {
                Ok(Some(lease_record)) => {
                    if lease_record.tenant != record.tenant {
                        false
                    } else {
                        if lease_record.status == LeaseStatus::Running
                            && matches!(record.state, EntityState::Starting { .. })
                            && self
                                .store
                                .apply_event(
                                    &record.tenant,
                                    &record.entity_id,
                                    &EntityEvent::LeaseReady { lease: lease.clone() },
                                )
                                .await
                                .is_ok()
                        {
                            report.runs_confirmed += 1;
                        }
                        lease_record.status != LeaseStatus::Released
                    }
                }
                Ok(None) => false,
                Err(error) => {
                    warn!(error = %error, "sandbox entity repair could not read a lease");
                    true
                }
            };
            if live || now.saturating_duration_since(since) < self.pending_timeout {
                continue;
            }
            let ended = self
                .store
                .apply_event(
                    &record.tenant,
                    &record.entity_id,
                    &EntityEvent::RunEnded { lease: lease.clone() },
                )
                .await;
            if let Some(ended) = self.note("repair", ended) {
                report.runs_ended += 1;
                seen.remove(&record.entity_id);
                self.publish(EntityLifecycleEvent::RunEnded {
                    entity_id: record.entity_id.clone(),
                    tenant: record.tenant.clone(),
                    lease,
                    generation: ended.record.generation,
                    reason: "repair".into(),
                    captured_digest: None,
                    at: SystemTime::now(),
                })
                .await;
            }
        }
        *self.observed.lock().unwrap_or_else(|e| e.into_inner()) = seen;
        match self.store.remove_dangling_name_claims().await {
            Ok(removed) => report.claims_removed = removed,
            Err(error) => {
                warn!(error = %error, "sandbox entity repair could not clean name claims")
            }
        }
        report
    }
}
