use std::time::Duration;

use async_nats::jetstream::kv;
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueStatus, is_create_key_value_already_exists,
};
use buffa::MessageField;
use buffa_types::google::protobuf::Timestamp;
use trogonai_session_contracts::{
    Actor, AssistantMessageCompletedPayload, CanonicalMessage, CanonicalToolCall,
    ContractValidationError, EventsArchivedPayload, SCHEMA_VERSION_V1, SessionBranchedPayload,
    SessionEvent, SessionEventPayload, SessionId, SessionSnapshot, SnapshotCreatedPayload,
    TerminalContinuity, TerminalContinuityCapturedPayload, ToolCallCompletedPayload,
    ToolCallRequestedPayload, UserMessageAddedPayload,
};

use crate::config::SessionKernelConfig;
use crate::error::SessionKernelError;
use crate::event_log::EventLogBackend;
use crate::lease::{
    SessionLeaseFactory, SessionLeaseGuard, SessionLeaseManager, SessionMutatingOperation, SessionKvLease,
    lease_bucket_config, lease_key_for_session,
};
use crate::materialize::materialize_from_events;
use crate::recovery::{recover_session, RecoveredSession, SnapshotReader};
use crate::snapshot::SnapshotStore;
use crate::telemetry;
use crate::usage::UsageStore;

impl<S> SnapshotReader for SnapshotStore<S>
where
    S: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
{
    async fn load_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionSnapshot>, SessionKernelError> {
        self.load_snapshot(session_id).await
    }
}

/// Shared KV-backed session lease factory.
#[derive(Clone)]
pub struct SessionKvLeaseFactory<S> {
    store: S,
    ttl: Duration,
}

impl<S> SessionKvLeaseFactory<S> {
    pub fn new(store: S, config: &SessionKernelConfig) -> Self {
        Self {
            store,
            ttl: config.lease_ttl(),
        }
    }
}

impl<S> SessionLeaseFactory for SessionKvLeaseFactory<S>
where
    S: trogon_nats::jetstream::JetStreamKeyValueCreateWithTtl
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + trogon_nats::jetstream::JetStreamKeyValueDeleteExpectRevision
        + Clone
        + Send
        + Sync
        + 'static,
{
    type Lease = SessionKvLease<S>;

    fn for_session(&self, session_id: &SessionId) -> Result<Self::Lease, SessionKernelError> {
        let key = lease_key_for_session(session_id)?;
        Ok(SessionKvLease::new(self.store.clone(), key, self.ttl))
    }
}

/// Operative session core: leases, append-only event log, snapshot materialization, recovery.
#[derive(Clone)]
pub struct SessionKernel<E, S, L> {
    config: SessionKernelConfig,
    event_log: E,
    snapshots: SnapshotStore<S>,
    usage: Option<UsageStore<S>>,
    leases: SessionLeaseManager<L>,
}

impl<E, S, L> SessionKernel<E, S, L>
where
    E: EventLogBackend,
    S: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    L: SessionLeaseFactory + Clone + 'static,
{
    pub fn new(
        config: SessionKernelConfig,
        event_log: E,
        snapshots: SnapshotStore<S>,
        leases: SessionLeaseManager<L>,
    ) -> Self {
        Self {
            config,
            event_log,
            snapshots,
            usage: None,
            leases,
        }
    }

    /// Attach a KV-backed usage store (`<PREFIX>_SESSION_USAGE`). When set,
    /// `materialize_state` persists the materialized token usage to it.
    pub fn with_usage_store(mut self, usage: UsageStore<S>) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn config(&self) -> &SessionKernelConfig {
        &self.config
    }

    pub fn snapshots(&self) -> &SnapshotStore<S> {
        &self.snapshots
    }

    pub fn usage(&self) -> Option<&UsageStore<S>> {
        self.usage.as_ref()
    }

    pub async fn acquire_session_lease(
        &self,
        session_id: &SessionId,
        operation: SessionMutatingOperation,
    ) -> Result<SessionLeaseGuard<<L as SessionLeaseFactory>::Lease>, SessionKernelError> {
        match self.leases.acquire_session_lease(session_id, operation).await {
            Ok(guard) => Ok(guard),
            Err(SessionKernelError::SessionBusy { session_id, .. }) => {
                telemetry::metrics::record_lease_contention(session_id.as_str());
                Err(SessionKernelError::session_busy(session_id))
            }
            Err(err) => Err(err),
        }
    }

    pub async fn renew_session_lease(
        &self,
        guard: &mut SessionLeaseGuard<<L as SessionLeaseFactory>::Lease>,
    ) -> Result<(), SessionKernelError> {
        self.leases.renew_session_lease(guard).await
    }

    pub async fn release_session_lease(
        &self,
        guard: SessionLeaseGuard<<L as SessionLeaseFactory>::Lease>,
    ) -> Result<(), SessionKernelError> {
        self.leases.release_session_lease(guard).await
    }

    pub async fn append_event(&self, mut event: SessionEvent) -> Result<SessionEvent, SessionKernelError> {
        self.assign_next_seq(&mut event).await?;
        let appended = self.event_log.append(event).await?;
        telemetry::metrics::record_event_appended(
            appended.session_id.as_str(),
            event_payload_name(&appended),
        );
        Ok(appended)
    }

    pub async fn append_event_idempotent(
        &self,
        mut event: SessionEvent,
        idempotency_key: &str,
    ) -> Result<SessionEvent, SessionKernelError> {
        let session_id = SessionId::new(&event.session_id)
            .map_err(|err| SessionKernelError::ContractValidation(ContractValidationError::InvalidSessionId(err)))?;
        if let Some(existing) = self
            .event_log
            .find_by_idempotency_key(&session_id, idempotency_key)
            .await?
        {
            telemetry::metrics::record_event_deduplicated(session_id.as_str());
            return Ok(existing);
        }

        event.idempotency_key = idempotency_key.to_string();
        self.assign_next_seq(&mut event).await?;
        match self.event_log.append(event).await {
            Ok(appended) => {
                telemetry::metrics::record_event_appended(
                    appended.session_id.as_str(),
                    event_payload_name(&appended),
                );
                Ok(appended)
            }
            Err(err) => {
                if let Some(existing) = self
                    .event_log
                    .find_by_idempotency_key(&session_id, idempotency_key)
                    .await?
                {
                    telemetry::metrics::record_event_deduplicated(session_id.as_str());
                    Ok(existing)
                } else {
                    Err(err)
                }
            }
        }
    }

    pub async fn load_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionSnapshot>, SessionKernelError> {
        self.snapshots.load_snapshot(session_id).await
    }

    pub async fn materialize_state(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionSnapshot, SessionKernelError> {
        let existing = self.snapshots.load_snapshot(session_id).await?;
        let events = self.event_log.read_session_events(session_id).await?;
        let snapshot = materialize_from_events(session_id.as_str(), &events, existing)?;
        self.snapshots.save_snapshot(&snapshot).await?;
        if let Some(usage_store) = &self.usage
            && let Some(usage) = snapshot
                .state
                .as_option()
                .and_then(|state| state.usage.as_option())
        {
            usage_store.save_usage(session_id, usage).await?;
        }
        Ok(snapshot)
    }

    /// Create a branch (`child`) from `parent` at `branched_at_seq` (§Fork/Branch).
    ///
    /// Seeds the child snapshot from the parent's materialized state so the child
    /// inherits the conversation, config and artifacts (shared by ref/hash via each
    /// `ArtifactMetadata.storage_ref`/`sha256` — no bytes are copied), then records a
    /// `session_branched` event as the child's first event carrying the lineage
    /// (`parent_session_id`, `branched_at_seq`). The child's events begin at its own
    /// `seq`; parent and child diverge from here.
    pub async fn fork_session(
        &self,
        parent_session_id: &SessionId,
        child_session_id: &SessionId,
        branched_at_seq: u64,
        operation_id: &str,
        actor: Actor,
        created_at: Timestamp,
    ) -> Result<SessionSnapshot, SessionKernelError> {
        // Read the parent's materialized state to seed the child (artifacts are shared
        // by reference; only metadata is carried, never the object-store bytes).
        let parent = self.materialize_state(parent_session_id).await?;

        // Append the branch event first so the child has a valid (non-zero) seq.
        let event = SessionEvent {
            schema_version: SCHEMA_VERSION_V1,
            event_id: format!("evt_branch_{}", child_session_id.as_str()),
            session_id: child_session_id.as_str().to_string(),
            seq: 0,
            operation_id: operation_id.to_string(),
            correlation_id: format!("corr_branch_{}", child_session_id.as_str()),
            idempotency_key: format!(
                "idem_branch_{}_{}",
                parent_session_id.as_str(),
                child_session_id.as_str()
            ),
            created_at: MessageField::some(created_at.clone()),
            actor: MessageField::some(actor),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(
                    SessionBranchedPayload {
                        parent_session_id: parent_session_id.as_str().to_string(),
                        child_session_id: child_session_id.as_str().to_string(),
                        branched_at_seq,
                        ..SessionBranchedPayload::default()
                    }
                    .into(),
                ),
                ..SessionEventPayload::default()
            }),
            ..SessionEvent::default()
        };
        let appended = self.append_event(event).await?;

        // Build the child snapshot from the parent's state plus the lineage, at the
        // branch event's seq.
        let mut child_state = parent.state.into_option().unwrap_or_default();
        let session = child_state.session.get_or_insert_default();
        session.id = child_session_id.as_str().to_string();
        session.parent_session_id = Some(parent_session_id.as_str().to_string());
        session.branched_at_seq = Some(branched_at_seq);
        let child_snapshot = SessionSnapshot {
            schema_version: SCHEMA_VERSION_V1,
            session_id: child_session_id.as_str().to_string(),
            last_applied_seq: appended.seq,
            state: MessageField::some(child_state),
            materialized_at: MessageField::some(created_at),
            ..SessionSnapshot::default()
        };
        self.snapshots.save_snapshot(&child_snapshot).await?;
        Ok(child_snapshot)
    }

    /// Reconstruct a session's transcript into the event log from a canonical
    /// conversation (§3 event log), emitting `user_message_added` /
    /// `assistant_message_completed` events idempotently (stable per-index keys so
    /// repeated calls only append newly-grown turns), then materialize the snapshot.
    ///
    /// This makes the event log — not a directly-built snapshot — the source of
    /// truth for the conversation. Returns the materialized snapshot, or an empty
    /// (unsaved) snapshot when there are no messages.
    pub async fn record_conversation(
        &self,
        session_id: &SessionId,
        messages: &[CanonicalMessage],
        actor: Actor,
        created_at: Timestamp,
    ) -> Result<SessionSnapshot, SessionKernelError> {
        if messages.is_empty() {
            return Ok(SessionSnapshot {
                schema_version: SCHEMA_VERSION_V1,
                session_id: session_id.as_str().to_string(),
                last_applied_seq: 0,
                ..SessionSnapshot::default()
            });
        }
        for (idx, message) in messages.iter().enumerate() {
            let idempotency_key = format!("idem_msg_{}_{idx}", session_id.as_str());
            let kind = if message.role == "user" {
                UserMessageAddedPayload {
                    message: MessageField::some(message.clone()),
                    ..UserMessageAddedPayload::default()
                }
                .into()
            } else {
                AssistantMessageCompletedPayload {
                    message_id: message.message_id.clone(),
                    message: MessageField::some(message.clone()),
                    ..AssistantMessageCompletedPayload::default()
                }
                .into()
            };
            let event = SessionEvent {
                schema_version: SCHEMA_VERSION_V1,
                event_id: format!("evt_msg_{}_{idx}", session_id.as_str()),
                session_id: session_id.as_str().to_string(),
                seq: 0,
                operation_id: format!("op_record_{}", session_id.as_str()),
                correlation_id: format!("corr_record_{}", session_id.as_str()),
                idempotency_key: idempotency_key.clone(),
                created_at: MessageField::some(created_at.clone()),
                actor: MessageField::some(actor.clone()),
                payload: MessageField::some(SessionEventPayload {
                    kind: Some(kind),
                    ..SessionEventPayload::default()
                }),
                ..SessionEvent::default()
            };
            self.append_event_idempotent(event, &idempotency_key).await?;
        }
        self.materialize_state(session_id).await
    }

    /// Reconstruct structured tool-call events into the event log (§3) from canonical
    /// tool calls: `tool_call_requested` for each, and `tool_call_completed` for those
    /// with a result. Idempotent per `tool_execution_id` so repeated calls only append
    /// newly-grown tool calls. Materializes and returns the snapshot.
    pub async fn record_tool_calls(
        &self,
        session_id: &SessionId,
        tool_calls: &[CanonicalToolCall],
        actor: Actor,
        created_at: Timestamp,
    ) -> Result<SessionSnapshot, SessionKernelError> {
        for tool in tool_calls {
            let exec = if tool.tool_execution_id.is_empty() {
                tool.id.clone()
            } else {
                tool.tool_execution_id.clone()
            };
            let requested_key = format!("idem_toolreq_{}_{exec}", session_id.as_str());
            let requested = SessionEvent {
                schema_version: SCHEMA_VERSION_V1,
                event_id: format!("evt_toolreq_{}_{exec}", session_id.as_str()),
                session_id: session_id.as_str().to_string(),
                seq: 0,
                operation_id: format!("op_toolrec_{}", session_id.as_str()),
                correlation_id: format!("corr_toolrec_{}", session_id.as_str()),
                idempotency_key: requested_key.clone(),
                created_at: MessageField::some(created_at.clone()),
                actor: MessageField::some(actor.clone()),
                payload: MessageField::some(SessionEventPayload {
                    kind: Some(
                        ToolCallRequestedPayload {
                            tool_call_id: tool.id.clone(),
                            tool_execution_id: exec.clone(),
                            name: tool.name.clone(),
                            input_json: tool.input_json.clone(),
                            parent_tool_use_id: tool.parent_tool_use_id.clone(),
                            ..ToolCallRequestedPayload::default()
                        }
                        .into(),
                    ),
                    ..SessionEventPayload::default()
                }),
                ..SessionEvent::default()
            };
            self.append_event_idempotent(requested, &requested_key).await?;

            if let Some(result) = tool.result.as_option() {
                let completed_key = format!("idem_tooldone_{}_{exec}", session_id.as_str());
                let completed = SessionEvent {
                    schema_version: SCHEMA_VERSION_V1,
                    event_id: format!("evt_tooldone_{}_{exec}", session_id.as_str()),
                    session_id: session_id.as_str().to_string(),
                    seq: 0,
                    operation_id: format!("op_toolrec_{}", session_id.as_str()),
                    correlation_id: format!("corr_toolrec_{}", session_id.as_str()),
                    idempotency_key: completed_key.clone(),
                    created_at: MessageField::some(created_at.clone()),
                    actor: MessageField::some(actor.clone()),
                    payload: MessageField::some(SessionEventPayload {
                        kind: Some(
                            ToolCallCompletedPayload {
                                tool_call_id: tool.id.clone(),
                                tool_execution_id: exec.clone(),
                                result: MessageField::some(result.clone()),
                                ..ToolCallCompletedPayload::default()
                            }
                            .into(),
                        ),
                        ..SessionEventPayload::default()
                    }),
                    ..SessionEvent::default()
                };
                self.append_event_idempotent(completed, &completed_key).await?;
            }
        }
        self.materialize_state(session_id).await
    }

    /// Capture preserved terminal/process continuity into the event log
    /// (§ Terminal and Process Policy), materializing it onto `state.terminal`.
    /// Callers (runners) supply the portable terminal fields they can observe.
    pub async fn record_terminal_continuity(
        &self,
        session_id: &SessionId,
        terminal: TerminalContinuity,
        operation_id: &str,
        actor: Actor,
        created_at: Timestamp,
    ) -> Result<SessionSnapshot, SessionKernelError> {
        let event = SessionEvent {
            schema_version: SCHEMA_VERSION_V1,
            event_id: format!("evt_term_{operation_id}"),
            session_id: session_id.as_str().to_string(),
            seq: 0,
            operation_id: operation_id.to_string(),
            correlation_id: format!("corr_term_{operation_id}"),
            idempotency_key: format!("idem_term_{operation_id}"),
            created_at: MessageField::some(created_at),
            actor: MessageField::some(actor),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(
                    TerminalContinuityCapturedPayload {
                        terminal: MessageField::some(terminal),
                        ..TerminalContinuityCapturedPayload::default()
                    }
                    .into(),
                ),
                ..SessionEventPayload::default()
            }),
            ..SessionEvent::default()
        };
        self.append_event(event).await?;
        self.materialize_state(session_id).await
    }

    /// Take a periodic snapshot checkpoint (§ Event Log Compaction and Retention):
    /// materialize and persist the snapshot, then record `snapshot_created` at its
    /// `last_applied_seq` as the retention audit trail. Returns the snapshot.
    pub async fn checkpoint_snapshot(
        &self,
        session_id: &SessionId,
        operation_id: &str,
        actor: Actor,
        created_at: Timestamp,
    ) -> Result<SessionSnapshot, SessionKernelError> {
        let snapshot = self.materialize_state(session_id).await?;
        let event = SessionEvent {
            schema_version: SCHEMA_VERSION_V1,
            event_id: format!("evt_snap_{operation_id}"),
            session_id: session_id.as_str().to_string(),
            seq: 0,
            operation_id: operation_id.to_string(),
            correlation_id: format!("corr_snap_{operation_id}"),
            idempotency_key: format!("idem_snap_{operation_id}"),
            created_at: MessageField::some(created_at),
            actor: MessageField::some(actor),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(
                    SnapshotCreatedPayload {
                        last_applied_seq: snapshot.last_applied_seq,
                        ..SnapshotCreatedPayload::default()
                    }
                    .into(),
                ),
                ..SessionEventPayload::default()
            }),
            ..SessionEvent::default()
        };
        self.append_event(event).await?;
        Ok(snapshot)
    }

    /// Record that events up to `through_seq` are eligible for archival
    /// (§ Event Log Compaction and Retention), emitting `events_archived` with the
    /// count as the retention watermark/audit trail. The physical stream purge is a
    /// JetStream operational action performed out of band. Returns the archived count.
    pub async fn archive_events_through(
        &self,
        session_id: &SessionId,
        through_seq: u64,
        operation_id: &str,
        actor: Actor,
        created_at: Timestamp,
    ) -> Result<u64, SessionKernelError> {
        let events = self.event_log.read_session_events(session_id).await?;
        let archived_count = events.iter().filter(|event| event.seq <= through_seq).count() as u64;
        let event = SessionEvent {
            schema_version: SCHEMA_VERSION_V1,
            event_id: format!("evt_arch_{operation_id}"),
            session_id: session_id.as_str().to_string(),
            seq: 0,
            operation_id: operation_id.to_string(),
            correlation_id: format!("corr_arch_{operation_id}"),
            idempotency_key: format!("idem_arch_{operation_id}"),
            created_at: MessageField::some(created_at),
            actor: MessageField::some(actor),
            payload: MessageField::some(SessionEventPayload {
                kind: Some(
                    EventsArchivedPayload {
                        through_seq,
                        archived_count,
                        ..EventsArchivedPayload::default()
                    }
                    .into(),
                ),
                ..SessionEventPayload::default()
            }),
            ..SessionEvent::default()
        };
        self.append_event(event).await?;
        Ok(archived_count)
    }

    pub async fn recover(&self, session_id: &SessionId) -> Result<RecoveredSession, SessionKernelError> {
        let recovered = recover_session(&self.event_log, &self.snapshots, session_id).await?;
        if recovered.state == crate::state::RecoveryState::StaleSnapshot {
            telemetry::metrics::record_snapshot_stale(session_id.as_str());
        }
        Ok(recovered)
    }

    async fn assign_next_seq(&self, event: &mut SessionEvent) -> Result<(), SessionKernelError> {
        let session_id = SessionId::new(&event.session_id)
            .map_err(|err| SessionKernelError::ContractValidation(ContractValidationError::InvalidSessionId(err)))?;
        let last = self.event_log.last_seq(&session_id).await?;
        let expected = last.saturating_add(1);
        if event.seq == 0 {
            event.seq = expected;
            return Ok(());
        }
        if event.seq != expected {
            return Err(SessionKernelError::SeqMismatch {
                session_id,
                actual: event.seq,
                expected,
            });
        }
        Ok(())
    }
}

pub async fn provision_lease_store<J, S>(
    js: &J,
    config: &SessionKernelConfig,
) -> Result<S, SessionKernelError>
where
    J: JetStreamCreateKeyValue<Store = S> + JetStreamGetKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let lease_config = lease_bucket_config(config)?;
    let kv_config = async_nats::jetstream::kv::Config {
        bucket: lease_config.bucket().as_str().to_owned(),
        history: 1,
        max_age: Duration::ZERO,
        limit_markers: Some(lease_config.ttl()),
        ..Default::default()
    };

    let store = match js.create_key_value(kv_config).await {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => js
            .get_key_value(lease_config.bucket().as_str().to_owned())
            .await
            .map_err(|err| SessionKernelError::Provision(err.to_string()))?,
        Err(source) => {
            return Err(SessionKernelError::Provision(source.to_string()));
        }
    };

    store
        .status()
        .await
        .map_err(|err| SessionKernelError::Provision(err.to_string()))?;

    Ok(store)
}

pub async fn provision_snapshot_store<J, S>(
    js: &J,
    config: &SessionKernelConfig,
) -> Result<S, SessionKernelError>
where
    J: JetStreamGetKeyValue<Store = S> + trogon_nats::jetstream::JetStreamCreateKeyValue<Store = S>,
{
    let bucket = crate::nats::session_snapshots_bucket(&config.nats_prefix);
    match js.get_key_value(bucket.clone()).await {
        Ok(store) => Ok(store),
        Err(_) => js
            .create_key_value(kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .map_err(|err| SessionKernelError::Provision(err.to_string())),
    }
}

pub async fn provision_usage_store<J, S>(
    js: &J,
    config: &SessionKernelConfig,
) -> Result<S, SessionKernelError>
where
    J: JetStreamGetKeyValue<Store = S> + trogon_nats::jetstream::JetStreamCreateKeyValue<Store = S>,
{
    let bucket = crate::nats::session_usage_bucket(&config.nats_prefix);
    match js.get_key_value(bucket.clone()).await {
        Ok(store) => Ok(store),
        Err(_) => js
            .create_key_value(kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .map_err(|err| SessionKernelError::Provision(err.to_string())),
    }
}

fn event_payload_name(event: &SessionEvent) -> &'static str {
    event
        .payload
        .as_option()
        .and_then(|payload| payload.kind.as_ref())
        .map(|kind| match kind {
            trogonai_session_contracts::session_event_payload::Kind::SessionCreated(_) => "session_created",
            trogonai_session_contracts::session_event_payload::Kind::UserMessageAdded(_) => "user_message_added",
            trogonai_session_contracts::session_event_payload::Kind::AssistantMessageStarted(_) => {
                "assistant_message_started"
            }
            trogonai_session_contracts::session_event_payload::Kind::AssistantMessageCompleted(_) => {
                "assistant_message_completed"
            }
            trogonai_session_contracts::session_event_payload::Kind::ToolCallRequested(_) => "tool_call_requested",
            trogonai_session_contracts::session_event_payload::Kind::ToolCallApproved(_) => "tool_call_approved",
            trogonai_session_contracts::session_event_payload::Kind::ToolCallStarted(_) => "tool_call_started",
            trogonai_session_contracts::session_event_payload::Kind::ToolCallCompleted(_) => "tool_call_completed",
            trogonai_session_contracts::session_event_payload::Kind::ToolCallFailed(_) => "tool_call_failed",
            trogonai_session_contracts::session_event_payload::Kind::ArtifactCreated(_) => "artifact_created",
            trogonai_session_contracts::session_event_payload::Kind::FileChanged(_) => "file_changed",
            trogonai_session_contracts::session_event_payload::Kind::SummaryCreated(_) => "summary_created",
            trogonai_session_contracts::session_event_payload::Kind::ContextTwinUpdated(_) => "context_twin_updated",
            trogonai_session_contracts::session_event_payload::Kind::SwitchAdaptationPlanCreated(_) => {
                "switch_adaptation_plan_created"
            }
            trogonai_session_contracts::session_event_payload::Kind::SwitchSafetyEvaluated(_) => {
                "switch_safety_evaluated"
            }
            trogonai_session_contracts::session_event_payload::Kind::ContinuityCheckpointStarted(_) => {
                "continuity_checkpoint_started"
            }
            trogonai_session_contracts::session_event_payload::Kind::ContinuityCheckpointCompleted(_) => {
                "continuity_checkpoint_completed"
            }
            trogonai_session_contracts::session_event_payload::Kind::ModelSwitched(_) => "model_switched",
            trogonai_session_contracts::session_event_payload::Kind::RunnerAttached(_) => "runner_attached",
            trogonai_session_contracts::session_event_payload::Kind::RunnerDetached(_) => "runner_detached",
            trogonai_session_contracts::session_event_payload::Kind::PermissionRuleAdded(_) => {
                "permission_rule_added"
            }
            trogonai_session_contracts::session_event_payload::Kind::TodoUpdated(_) => "todo_updated",
            trogonai_session_contracts::session_event_payload::Kind::SessionCompacted(_) => "session_compacted",
            trogonai_session_contracts::session_event_payload::Kind::SessionBranched(_) => "session_branched",
            trogonai_session_contracts::session_event_payload::Kind::OperationCancelRequested(_) => {
                "operation_cancel_requested"
            }
            trogonai_session_contracts::session_event_payload::Kind::RunnerCancelRequested(_) => {
                "runner_cancel_requested"
            }
            trogonai_session_contracts::session_event_payload::Kind::RunnerCancelled(_) => "runner_cancelled",
            trogonai_session_contracts::session_event_payload::Kind::OperationCancelled(_) => "operation_cancelled",
            trogonai_session_contracts::session_event_payload::Kind::OperationCancelFailed(_) => {
                "operation_cancel_failed"
            }
            trogonai_session_contracts::session_event_payload::Kind::OperationRequiresReconciliation(_) => {
                "operation_requires_reconciliation"
            }
            trogonai_session_contracts::session_event_payload::Kind::ForceSwitchRequested(_) => {
                "force_switch_requested"
            }
            trogonai_session_contracts::session_event_payload::Kind::ForceSwitchConfirmed(_) => {
                "force_switch_confirmed"
            }
            trogonai_session_contracts::session_event_payload::Kind::ForceSwitchCompleted(_) => {
                "force_switch_completed"
            }
            trogonai_session_contracts::session_event_payload::Kind::ForceSwitchRejected(_) => {
                "force_switch_rejected"
            }
            trogonai_session_contracts::session_event_payload::Kind::RunnerFailed(_) => "runner_failed",
            trogonai_session_contracts::session_event_payload::Kind::InvalidEventRejected(_) => {
                "invalid_event_rejected"
            }
            trogonai_session_contracts::session_event_payload::Kind::SnapshotCreated(_) => "snapshot_created",
            trogonai_session_contracts::session_event_payload::Kind::EventsArchived(_) => "events_archived",
            trogonai_session_contracts::session_event_payload::Kind::ArtifactGcMarked(_) => "artifact_gc_marked",
            trogonai_session_contracts::session_event_payload::Kind::ArtifactGcDeleted(_) => "artifact_gc_deleted",
            trogonai_session_contracts::session_event_payload::Kind::RedactionApplied(_) => "redaction_applied",
            trogonai_session_contracts::session_event_payload::Kind::TerminalContinuityCaptured(_) => {
                "terminal_continuity_captured"
            }
            trogonai_session_contracts::session_event_payload::Kind::CompactorModelPreserved(_) => {
                "compactor_model_preserved"
            }
            trogonai_session_contracts::session_event_payload::Kind::CompactorModelUnavailable(_) => {
                "compactor_model_unavailable"
            }
            trogonai_session_contracts::session_event_payload::Kind::FallbackToDefaultCompactor(_) => {
                "fallback_to_default_compactor"
            }
        })
        .unwrap_or("unknown")
}
