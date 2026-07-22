//! Persistence for the per-session routing/migration record (event-log-primary flag and
//! legacy-migration provenance). Stored in the snapshots bucket under
//! `sessions.{id}.routing` (cambio-modelo.md § "NATS KV para snapshots"). On resume the
//! kernel reads this record so [`crate::migration::resolve_event_log_primary`] can decide
//! whether to reconstruct the session from event-log replay (Fase 11).

use buffa::Message as _;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet};
use trogonai_session_contracts::{SCHEMA_VERSION_V1, SessionId, SessionRoutingRecord};

use crate::config::SessionKernelConfig;
use crate::error::SessionKernelError;
use crate::features::{EventLogPrimaryMode, SessionKernelFeatureFlags};
use crate::migration::SessionMigrationRecord;
use crate::nats::{session_routing_key, session_snapshots_bucket};

impl SessionMigrationRecord {
    /// Routing record for a freshly created session. Under
    /// [`EventLogPrimaryMode::NewSessionsOnly`] / [`EventLogPrimaryMode::AllMigrated`] a new
    /// session is event-log-primary from birth; under `LegacyMessages` it stays
    /// runner-owned (cambio-modelo.md Fase 11: event-primary only for opt-in sessions).
    pub fn for_new_session(flags: &SessionKernelFeatureFlags) -> Self {
        let event_log_primary = !matches!(flags.event_log_primary_mode(), EventLogPrimaryMode::LegacyMessages);
        Self {
            event_log_primary,
            migrated_from_legacy: false,
            legacy_message_count: 0,
        }
    }

    /// Convert to the durable wire record for KV persistence (ADR-0009).
    pub fn to_proto(&self, session_id: &SessionId) -> SessionRoutingRecord {
        SessionRoutingRecord {
            schema_version: SCHEMA_VERSION_V1,
            session_id: session_id.as_str().to_string(),
            event_log_primary: self.event_log_primary,
            migrated_from_legacy: self.migrated_from_legacy,
            legacy_message_count: self.legacy_message_count as u64,
            ..SessionRoutingRecord::default()
        }
    }

    /// Convert from the durable wire record loaded from KV.
    pub fn from_proto(record: &SessionRoutingRecord) -> Self {
        Self {
            event_log_primary: record.event_log_primary,
            migrated_from_legacy: record.migrated_from_legacy,
            legacy_message_count: record.legacy_message_count as usize,
        }
    }
}

/// NATS KV store for per-session routing/migration records.
#[derive(Clone)]
pub struct MigrationStore<S> {
    store: S,
    config: SessionKernelConfig,
}

impl<S> MigrationStore<S> {
    pub fn new(store: S, config: SessionKernelConfig) -> Self {
        Self { store, config }
    }

    pub fn bucket_name(&self) -> String {
        session_snapshots_bucket(&self.config.nats_prefix)
    }
}

impl<S> MigrationStore<S>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    /// Load the routing record for a session, or `None` if it was never written (a
    /// legacy session that predates the kernel — treated as not event-primary).
    pub async fn load_routing(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionMigrationRecord>, SessionKernelError> {
        let key = session_routing_key(session_id);
        let Some(bytes) = self
            .store
            .get(key)
            .await
            .map_err(|err| SessionKernelError::RoutingLoad(err.to_string()))?
        else {
            return Ok(None);
        };
        let record = SessionRoutingRecord::decode_from_slice(&bytes)
            .map_err(|err| SessionKernelError::Decode(err.to_string()))?;
        Ok(Some(SessionMigrationRecord::from_proto(&record)))
    }

    /// Persist the routing record for a session (create or update).
    pub async fn save_routing(
        &self,
        session_id: &SessionId,
        record: &SessionMigrationRecord,
    ) -> Result<(), SessionKernelError> {
        let key = session_routing_key(session_id);
        let bytes = Bytes::from(record.to_proto(session_id).encode_to_vec());

        if let Some(entry) = self
            .store
            .entry(key.clone())
            .await
            .map_err(|err| SessionKernelError::RoutingStore(err.to_string()))?
        {
            self.store
                .update(&key, bytes, entry.revision)
                .await
                .map_err(|err| SessionKernelError::RoutingStore(err.to_string()))?;
            return Ok(());
        }

        self.store
            .create(&key, bytes)
            .await
            .map_err(|err| SessionKernelError::RoutingStore(err.to_string()))?;
        Ok(())
    }

    /// Mark a freshly created session's routing record according to the configured
    /// event-log-primary mode, persisting it. Returns the record that was written.
    pub async fn mark_new_session(
        &self,
        session_id: &SessionId,
        flags: &SessionKernelFeatureFlags,
    ) -> Result<SessionMigrationRecord, SessionKernelError> {
        let record = SessionMigrationRecord::for_new_session(flags);
        self.save_routing(session_id, &record).await?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::SessionKernelFeatureFlags;
    use crate::migration::resolve_event_log_primary;
    use trogon_nats::jetstream::MockJetStreamKvStore;

    fn config() -> SessionKernelConfig {
        SessionKernelConfig::default()
    }

    fn flags_with_mode(mode: &str) -> SessionKernelFeatureFlags {
        SessionKernelFeatureFlags::default().with_event_log_primary_mode(mode)
    }

    #[test]
    fn for_new_session_is_event_primary_only_outside_legacy_mode() {
        assert!(!SessionMigrationRecord::for_new_session(&flags_with_mode("legacy_messages")).event_log_primary);
        assert!(SessionMigrationRecord::for_new_session(&flags_with_mode("new_sessions")).event_log_primary);
        assert!(SessionMigrationRecord::for_new_session(&flags_with_mode("all_migrated")).event_log_primary);
    }

    #[test]
    fn proto_roundtrip_preserves_fields() {
        let session_id = SessionId::new("sess_routing").unwrap();
        let record = SessionMigrationRecord {
            event_log_primary: true,
            migrated_from_legacy: true,
            legacy_message_count: 7,
        };
        let restored = SessionMigrationRecord::from_proto(&record.to_proto(&session_id));
        assert_eq!(restored, record);
    }

    #[tokio::test]
    async fn save_then_resolve_marks_session_event_primary() {
        let session_id = SessionId::new("sess_routing").unwrap();
        let store = MockJetStreamKvStore::new();
        // First write creates: entry() returns None so save_routing takes the create path.
        store.enqueue_entry_none();
        store.enqueue_create_result(Ok(1));
        let migration = MigrationStore::new(store.clone(), config());

        let written = migration
            .mark_new_session(&session_id, &flags_with_mode("new_sessions"))
            .await
            .unwrap();
        assert!(written.event_log_primary);

        // The create call carried the routing key and a non-empty payload.
        let creates = store.create_calls();
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].0, "sessions.sess_routing.routing");
        assert!(!creates[0].1.is_empty());
    }

    #[tokio::test]
    async fn load_decodes_persisted_record() {
        let session_id = SessionId::new("sess_routing").unwrap();
        let record = SessionMigrationRecord {
            event_log_primary: true,
            migrated_from_legacy: false,
            legacy_message_count: 0,
        };
        let bytes = Bytes::from(record.to_proto(&session_id).encode_to_vec());

        let store = MockJetStreamKvStore::new();
        store.enqueue_get_some(bytes);
        let migration = MigrationStore::new(store, config());

        let loaded = migration.load_routing(&session_id).await.unwrap().unwrap();
        assert_eq!(loaded, record);
        // NewSessionsOnly + event_log_primary record → reads prefer the event log.
        assert!(resolve_event_log_primary(&flags_with_mode("new_sessions"), &loaded));
    }

    #[tokio::test]
    async fn load_missing_record_is_none() {
        let session_id = SessionId::new("sess_legacy").unwrap();
        let store = MockJetStreamKvStore::new();
        store.enqueue_get_none();
        let migration = MigrationStore::new(store, config());
        assert!(migration.load_routing(&session_id).await.unwrap().is_none());
    }
}
