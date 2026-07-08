use std::collections::BTreeSet;
use std::error::Error;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, context, kv};
use buffa::Message as _;
use bytes::Bytes;
use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use tracing::{error, info, warn};
use trogon_decider_nats::StreamStoreError;
use trogon_decider_runtime::{
    EventDecodeOutcome, ReadFrom, ReadStreamRequest, SnapshotRead, SnapshotWrite, StreamAppend, StreamEvent, StreamRead,
};
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamGetRawMessage, JetStreamGetStreamInfo,
    JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry,
    is_create_key_value_already_exists,
};
use trogonai_proto::gateway::credentials::checkpoints_v1 as proto;
use trogonai_proto::gateway::credentials::{CredentialEventCase, CredentialEventPayloadError, state_v1, v1};

use crate::credential::commands::domain::CredentialId;
use crate::credential::handler::{
    CredentialActivationRecoveryCommand, CredentialActivationRecoveryPlanError, CredentialRuntimeHandler,
    activation_recovery_command,
};
use crate::credential::proto::{
    CredentialProtoDecodeError, decode_credential_metadata, decode_message_field, decode_revoked, decode_rotated,
    decode_rotation_failed, decode_rotation_requested, decode_write_failed, decode_write_requested,
};
use crate::credential::{CredentialEvolveError, evolve, initial_state};
use crate::secret_store::SecretStoreMetadata;

const CHECKPOINT_KEY: &str = "v1.recovery-worker";
const DEFAULT_INITIAL_FAILURE_BACKOFF: Duration = Duration::from_secs(30);
const DEFAULT_MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(15 * 60);
const DEFAULT_STUCK_AFTER: Duration = Duration::from_secs(30 * 60);
const RECOVERY_METER_NAME: &str = "trogon-gateway";
pub(crate) const CREDENTIAL_WORKER_CHECKPOINT_BUCKET: &str = "GATEWAY_CREDENTIAL_WORKER_CHECKPOINTS";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CredentialRecoveryReport {
    scanned_events: usize,
    decoded_events: usize,
    skipped_events: usize,
    changed_credentials: usize,
    skipped_credentials: usize,
    planned_recoveries: usize,
    recovered_writes: usize,
    recovered_rotations: usize,
    failed_recoveries: usize,
    checkpoint_loaded_sequence: u64,
    checkpoint_advanced_to: Option<u64>,
    checkpoint_failure_count: u32,
    checkpoint_retry_after_unix_seconds: Option<u64>,
    retry_delayed: bool,
    stuck_recovery: bool,
}

impl CredentialRecoveryReport {
    pub(crate) fn scanned_events(&self) -> usize {
        self.scanned_events
    }

    pub(crate) fn decoded_events(&self) -> usize {
        self.decoded_events
    }

    pub(crate) fn skipped_events(&self) -> usize {
        self.skipped_events
    }

    pub(crate) fn changed_credentials(&self) -> usize {
        self.changed_credentials
    }

    pub(crate) fn skipped_credentials(&self) -> usize {
        self.skipped_credentials
    }

    pub(crate) fn planned_recoveries(&self) -> usize {
        self.planned_recoveries
    }

    pub(crate) fn recovered_writes(&self) -> usize {
        self.recovered_writes
    }

    pub(crate) fn recovered_rotations(&self) -> usize {
        self.recovered_rotations
    }

    pub(crate) fn failed_recoveries(&self) -> usize {
        self.failed_recoveries
    }

    pub(crate) fn checkpoint_loaded_sequence(&self) -> u64 {
        self.checkpoint_loaded_sequence
    }

    pub(crate) fn checkpoint_advanced_to(&self) -> Option<u64> {
        self.checkpoint_advanced_to
    }

    pub(crate) fn checkpoint_failure_count(&self) -> u32 {
        self.checkpoint_failure_count
    }

    pub(crate) fn checkpoint_retry_after_unix_seconds(&self) -> Option<u64> {
        self.checkpoint_retry_after_unix_seconds
    }

    pub(crate) fn retry_delayed(&self) -> bool {
        self.retry_delayed
    }

    pub(crate) fn stuck_recovery(&self) -> bool {
        self.stuck_recovery
    }

    fn has_recovery_activity(&self) -> bool {
        self.planned_recoveries > 0 || self.failed_recoveries > 0 || self.retry_delayed || self.stuck_recovery
    }

    fn metric_outcome(&self) -> &'static str {
        if self.retry_delayed {
            return "retry_delayed";
        }
        if self.failed_recoveries > 0 {
            return "failed_recovery";
        }
        if self.checkpoint_advanced_to.is_some() {
            return "advanced";
        }
        if self.planned_recoveries > 0 || self.recovered_writes > 0 || self.recovered_rotations > 0 {
            return "recovered";
        }
        if self.stuck_recovery {
            return "stuck";
        }
        "idle"
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CredentialRecoveryPolicy {
    initial_failure_backoff: Duration,
    max_failure_backoff: Duration,
    stuck_after: Duration,
}

impl CredentialRecoveryPolicy {
    fn failure_backoff(self, failure_count: u32) -> Duration {
        let shift = failure_count.saturating_sub(1).min(31);
        let seconds = self.initial_failure_backoff.as_secs().max(1);
        Duration::from_secs(
            seconds
                .saturating_mul(1_u64 << shift)
                .min(self.max_failure_backoff.as_secs()),
        )
    }
}

impl Default for CredentialRecoveryPolicy {
    fn default() -> Self {
        Self {
            initial_failure_backoff: DEFAULT_INITIAL_FAILURE_BACKOFF,
            max_failure_backoff: DEFAULT_MAX_FAILURE_BACKOFF,
            stuck_after: DEFAULT_STUCK_AFTER,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CredentialRecoveryPlan {
    report: CredentialRecoveryReport,
    commands: Vec<PlannedCredentialRecovery>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlannedCredentialRecovery {
    credential_id: CredentialId,
    command: CredentialActivationRecoveryCommand,
}

pub(crate) async fn run<EventStream, EventStore, Secrets>(
    event_stream: EventStream,
    event_store: EventStore,
    handler: CredentialRuntimeHandler<EventStore, Secrets>,
    checkpoints: CredentialRecoveryKvCheckpointStore<kv::Store>,
    interval: Duration,
) where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreMetadata,
{
    let mut interval = tokio::time::interval(interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let metrics = CredentialRecoveryMetrics::new();

    loop {
        tokio::select! {
            _ = trogon_std::signal::shutdown_signal() => {
                info!("credential recovery worker stopped");
                return;
            }
            _ = interval.tick() => {
                match recover_pending_credential_activations(&event_stream, &event_store, &handler, &checkpoints).await {
                    Ok(report) if report.has_recovery_activity() => {
                        metrics.record_report(&report);
                        info!(
                            scanned_events = report.scanned_events(),
                            decoded_events = report.decoded_events(),
                            skipped_events = report.skipped_events(),
                            changed_credentials = report.changed_credentials(),
                            skipped_credentials = report.skipped_credentials(),
                            planned_recoveries = report.planned_recoveries(),
                            recovered_writes = report.recovered_writes(),
                            recovered_rotations = report.recovered_rotations(),
                            failed_recoveries = report.failed_recoveries(),
                            checkpoint_loaded_sequence = report.checkpoint_loaded_sequence(),
                            checkpoint_advanced_to = ?report.checkpoint_advanced_to(),
                            checkpoint_failure_count = report.checkpoint_failure_count(),
                            checkpoint_retry_after_unix_seconds = ?report.checkpoint_retry_after_unix_seconds(),
                            retry_delayed = report.retry_delayed(),
                            stuck_recovery = report.stuck_recovery(),
                            "credential recovery pass completed"
                        );
                    }
                    Ok(report) => {
                        metrics.record_report(&report);
                    }
                    Err(source) => {
                        metrics.record_error("worker_error");
                        error!(error = %source, "credential recovery pass failed");
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct CredentialRecoveryMetrics {
    passes: Counter<u64>,
    errors: Counter<u64>,
    scanned_events: Counter<u64>,
    recoveries: Counter<u64>,
    stuck_reports: Counter<u64>,
}

impl CredentialRecoveryMetrics {
    fn new() -> Self {
        let meter = trogon_telemetry::meter(RECOVERY_METER_NAME);
        Self {
            passes: meter
                .u64_counter("gateway.credential.recovery.passes")
                .with_description("Credential recovery worker passes by outcome.")
                .build(),
            errors: meter
                .u64_counter("gateway.credential.recovery.errors")
                .with_description("Credential recovery worker pass errors by reason.")
                .build(),
            scanned_events: meter
                .u64_counter("gateway.credential.recovery.scanned_events")
                .with_description("Raw credential stream events scanned by the recovery worker.")
                .build(),
            recoveries: meter
                .u64_counter("gateway.credential.recovery.recoveries")
                .with_description("Credential recovery commands by status and kind.")
                .build(),
            stuck_reports: meter
                .u64_counter("gateway.credential.recovery.stuck_reports")
                .with_description("Recovery passes that observed a stuck recovery checkpoint.")
                .build(),
        }
    }

    fn record_report(&self, report: &CredentialRecoveryReport) {
        self.passes.add(1, &[KeyValue::new("outcome", report.metric_outcome())]);
        if report.scanned_events() > 0 {
            self.scanned_events.add(report.scanned_events() as u64, &[]);
        }
        self.record_recovery_count("planned", "all", report.planned_recoveries());
        self.record_recovery_count("recovered", "write", report.recovered_writes());
        self.record_recovery_count("recovered", "rotation", report.recovered_rotations());
        self.record_recovery_count("failed", "all", report.failed_recoveries());
        if report.stuck_recovery() {
            self.stuck_reports.add(1, &[]);
        }
    }

    fn record_error(&self, reason: &'static str) {
        self.passes.add(1, &[KeyValue::new("outcome", "error")]);
        self.errors.add(1, &[KeyValue::new("reason", reason)]);
    }

    fn record_recovery_count(&self, status: &'static str, kind: &'static str, count: usize) {
        if count == 0 {
            return;
        }
        self.recoveries.add(
            count as u64,
            &[KeyValue::new("status", status), KeyValue::new("kind", kind)],
        );
    }
}

pub(crate) async fn recover_pending_credential_activations<EventStream, EventStore, Secrets, Checkpoints>(
    event_stream: &EventStream,
    event_store: &EventStore,
    handler: &CredentialRuntimeHandler<EventStore, Secrets>,
    checkpoints: &Checkpoints,
) -> Result<CredentialRecoveryReport, CredentialRecoveryWorkerError>
where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreMetadata,
    Checkpoints: CredentialRecoveryCheckpointStore,
{
    recover_pending_credential_activations_at(
        event_stream,
        event_store,
        handler,
        checkpoints,
        SystemTime::now(),
        CredentialRecoveryPolicy::default(),
    )
    .await
}

async fn recover_pending_credential_activations_at<EventStream, EventStore, Secrets, Checkpoints>(
    event_stream: &EventStream,
    event_store: &EventStore,
    handler: &CredentialRuntimeHandler<EventStore, Secrets>,
    checkpoints: &Checkpoints,
    now: SystemTime,
    policy: CredentialRecoveryPolicy,
) -> Result<CredentialRecoveryReport, CredentialRecoveryWorkerError>
where
    EventStream: JetStreamGetStreamInfo + JetStreamGetRawMessage,
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreMetadata,
    Checkpoints: CredentialRecoveryCheckpointStore,
{
    let checkpoint = checkpoints
        .load()
        .await
        .map_err(|source| CredentialRecoveryWorkerError::Checkpoint { source })?;
    if checkpoint.retry_delayed_at(now) {
        let report = report_from_checkpoint(checkpoint, now, policy, true);
        if report.stuck_recovery() {
            warn!(
                checkpoint_loaded_sequence = report.checkpoint_loaded_sequence(),
                checkpoint_failure_count = report.checkpoint_failure_count(),
                checkpoint_retry_after_unix_seconds = ?report.checkpoint_retry_after_unix_seconds(),
                "credential recovery is stuck behind retry backoff"
            );
        }
        return Ok(report);
    }

    let from_sequence = checkpoint.next_sequence();
    let events = trogon_decider_nats::read_stream(event_stream, from_sequence)
        .await
        .map_err(|source| CredentialRecoveryWorkerError::ReadStream { source })?;
    let max_scanned_sequence = events.iter().map(|event| event.stream_position.as_u64()).max();
    let mut report = recover_pending_credential_activations_from_events(events, event_store, handler)
        .await
        .map_err(|source| CredentialRecoveryWorkerError::BuildPlan { source })?;
    report.checkpoint_loaded_sequence = checkpoint.last_scanned_sequence();
    report.checkpoint_failure_count = checkpoint.consecutive_failure_count();
    report.checkpoint_retry_after_unix_seconds = checkpoint.retry_after_unix_seconds();
    report.stuck_recovery = checkpoint.stuck_at(now, policy);

    if report.failed_recoveries == 0 {
        if let Some(last_scanned_sequence) = max_scanned_sequence {
            let checkpoint = CredentialRecoveryCheckpoint::new(last_scanned_sequence);
            checkpoints
                .save(checkpoint)
                .await
                .map_err(|source| CredentialRecoveryWorkerError::Checkpoint { source })?;
            report.checkpoint_advanced_to = Some(last_scanned_sequence);
            report.checkpoint_failure_count = 0;
            report.checkpoint_retry_after_unix_seconds = None;
            report.stuck_recovery = false;
        }
    } else {
        let checkpoint = checkpoint.record_failure(now, policy);
        checkpoints
            .save(checkpoint)
            .await
            .map_err(|source| CredentialRecoveryWorkerError::Checkpoint { source })?;
        report.checkpoint_failure_count = checkpoint.consecutive_failure_count();
        report.checkpoint_retry_after_unix_seconds = checkpoint.retry_after_unix_seconds();
        report.stuck_recovery = checkpoint.stuck_at(now, policy);
        warn!(
            failed_recoveries = report.failed_recoveries(),
            checkpoint_loaded_sequence = report.checkpoint_loaded_sequence(),
            checkpoint_failure_count = report.checkpoint_failure_count(),
            checkpoint_retry_after_unix_seconds = ?report.checkpoint_retry_after_unix_seconds(),
            stuck_recovery = report.stuck_recovery(),
            "credential recovery checkpoint was not advanced"
        );
    }

    Ok(report)
}

fn report_from_checkpoint(
    checkpoint: CredentialRecoveryCheckpoint,
    now: SystemTime,
    policy: CredentialRecoveryPolicy,
    retry_delayed: bool,
) -> CredentialRecoveryReport {
    CredentialRecoveryReport {
        checkpoint_loaded_sequence: checkpoint.last_scanned_sequence(),
        checkpoint_failure_count: checkpoint.consecutive_failure_count(),
        checkpoint_retry_after_unix_seconds: checkpoint.retry_after_unix_seconds(),
        retry_delayed,
        stuck_recovery: checkpoint.stuck_at(now, policy),
        ..Default::default()
    }
}

async fn recover_pending_credential_activations_from_events<EventStore, Secrets>(
    events: impl IntoIterator<Item = StreamEvent>,
    event_store: &EventStore,
    handler: &CredentialRuntimeHandler<EventStore, Secrets>,
) -> Result<CredentialRecoveryReport, CredentialRecoveryPlanBuildError>
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreMetadata,
{
    let plan = recovery_plan_from_credential_events(events, event_store).await?;
    let mut report = plan.report;

    for planned in plan.commands {
        match planned.command {
            CredentialActivationRecoveryCommand::Write(command) => {
                match handler.recover_write_activation(command).await {
                    Ok(_) => report.recovered_writes += 1,
                    Err(source) => {
                        report.failed_recoveries += 1;
                        warn!(
                            credential_id = %planned.credential_id,
                            error = %source,
                            "credential write activation recovery failed"
                        );
                    }
                }
            }
            CredentialActivationRecoveryCommand::Rotation(command) => {
                match handler.recover_rotation_activation(command).await {
                    Ok(_) => report.recovered_rotations += 1,
                    Err(source) => {
                        report.failed_recoveries += 1;
                        warn!(
                            credential_id = %planned.credential_id,
                            error = %source,
                            "credential rotation activation recovery failed"
                        );
                    }
                }
            }
        }
    }

    Ok(report)
}

async fn recovery_plan_from_credential_events<EventStore>(
    events: impl IntoIterator<Item = StreamEvent>,
    event_store: &EventStore,
) -> Result<CredentialRecoveryPlan, CredentialRecoveryPlanBuildError>
where
    EventStore: StreamRead<str>,
{
    let mut report = CredentialRecoveryReport::default();
    let mut changed_credentials: BTreeSet<CredentialId> = BTreeSet::new();

    for event in events {
        report.scanned_events += 1;
        match event
            .decode::<v1::CredentialEvent>()
            .map_err(|source| CredentialRecoveryPlanBuildError::DecodeEvent { source })?
        {
            EventDecodeOutcome::Decoded(event) => {
                report.decoded_events += 1;
                let case = event
                    .event
                    .as_ref()
                    .ok_or(CredentialRecoveryPlanBuildError::MissingEvent)?;
                let credential_id = event_credential_id(case)
                    .map_err(|source| CredentialRecoveryPlanBuildError::InvalidEvent { source })?;
                changed_credentials.insert(credential_id);
            }
            EventDecodeOutcome::Skipped => {
                report.skipped_events += 1;
            }
        }
    }
    report.changed_credentials = changed_credentials.len();

    let mut commands = Vec::new();
    for credential_id in changed_credentials {
        let state = load_credential_state(event_store, &credential_id).await?;

        match activation_recovery_command(credential_id.as_str(), &state).map_err(|source| {
            CredentialRecoveryPlanBuildError::PlanRecovery {
                credential_id: credential_id.clone(),
                source: Box::new(source),
            }
        })? {
            Some(command) => {
                report.planned_recoveries += 1;
                commands.push(PlannedCredentialRecovery { credential_id, command });
            }
            None => {
                report.skipped_credentials += 1;
            }
        }
    }

    Ok(CredentialRecoveryPlan { report, commands })
}

async fn load_credential_state<EventStore>(
    event_store: &EventStore,
    credential_id: &CredentialId,
) -> Result<state_v1::CredentialStateSnapshot, CredentialRecoveryPlanBuildError>
where
    EventStore: StreamRead<str>,
{
    let stream = event_store
        .read_stream(ReadStreamRequest {
            stream_id: credential_id.as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .map_err(|source| CredentialRecoveryPlanBuildError::ReadCredential {
            credential_id: credential_id.clone(),
            source: Box::new(source),
        })?;
    let mut state = initial_state();
    for event in stream.events {
        let EventDecodeOutcome::Decoded(event) = event
            .decode::<v1::CredentialEvent>()
            .map_err(|source| CredentialRecoveryPlanBuildError::DecodeEvent { source })?
        else {
            continue;
        };
        state = evolve(state, &event).map_err(|source| CredentialRecoveryPlanBuildError::ReplayCredential {
            credential_id: credential_id.clone(),
            source,
        })?;
    }
    Ok(state)
}

fn event_credential_id(event: &CredentialEventCase) -> Result<CredentialId, CredentialProtoDecodeError> {
    match event {
        CredentialEventCase::WriteRequested(inner) => Ok(decode_write_requested(inner)?.0),
        CredentialEventCase::WriteFailed(inner) => Ok(decode_write_failed(inner)?.0),
        CredentialEventCase::Activated(inner) => {
            let metadata = decode_message_field("event.metadata", &inner.metadata)?;
            Ok(decode_credential_metadata("event.metadata", metadata)?
                .reference()
                .id()
                .clone())
        }
        CredentialEventCase::RotationRequested(inner) => Ok(decode_rotation_requested(inner)?.id().clone()),
        CredentialEventCase::RotationFailed(inner) => Ok(decode_rotation_failed(inner)?.0.id().clone()),
        CredentialEventCase::Revoked(inner) => Ok(decode_revoked(inner)?.id().clone()),
        CredentialEventCase::Rotated(inner) => Ok(decode_rotated(inner)?.0.id().clone()),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialRecoveryWorkerError {
    #[error("credential event stream read failed: {source}")]
    ReadStream {
        #[source]
        source: StreamStoreError,
    },
    #[error("credential recovery checkpoint failed: {source}")]
    Checkpoint {
        #[source]
        source: CredentialRecoveryCheckpointStoreError,
    },
    #[error("credential recovery plan failed: {source}")]
    BuildPlan {
        #[source]
        source: CredentialRecoveryPlanBuildError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialRecoveryPlanBuildError {
    #[error("credential event decode failed: {source}")]
    DecodeEvent {
        #[source]
        source: CredentialEventPayloadError,
    },
    #[error("credential event is missing its event case")]
    MissingEvent,
    #[error("credential event is invalid: {source}")]
    InvalidEvent {
        #[source]
        source: CredentialProtoDecodeError,
    },
    #[error("credential stream read failed for {credential_id}: {source}")]
    ReadCredential {
        credential_id: CredentialId,
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("credential stream replay failed for {credential_id}: {source}")]
    ReplayCredential {
        credential_id: CredentialId,
        #[source]
        source: CredentialEvolveError,
    },
    #[error("credential recovery planning failed for {credential_id}: {source}")]
    PlanRecovery {
        credential_id: CredentialId,
        #[source]
        source: Box<CredentialActivationRecoveryPlanError>,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CredentialRecoveryCheckpoint {
    last_scanned_sequence: u64,
    consecutive_failure_count: u32,
    first_failure_unix_seconds: Option<u64>,
    retry_after_unix_seconds: Option<u64>,
}

impl CredentialRecoveryCheckpoint {
    pub(crate) fn new(last_scanned_sequence: u64) -> Self {
        Self {
            last_scanned_sequence,
            consecutive_failure_count: 0,
            first_failure_unix_seconds: None,
            retry_after_unix_seconds: None,
        }
    }

    pub(crate) fn with_failure_state(
        last_scanned_sequence: u64,
        consecutive_failure_count: u32,
        first_failure_unix_seconds: Option<u64>,
        retry_after_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            last_scanned_sequence,
            consecutive_failure_count,
            first_failure_unix_seconds,
            retry_after_unix_seconds,
        }
    }

    pub(crate) fn last_scanned_sequence(self) -> u64 {
        self.last_scanned_sequence
    }

    pub(crate) fn consecutive_failure_count(self) -> u32 {
        self.consecutive_failure_count
    }

    pub(crate) fn retry_after_unix_seconds(self) -> Option<u64> {
        self.retry_after_unix_seconds
    }

    pub(crate) fn first_failure_unix_seconds(self) -> Option<u64> {
        self.first_failure_unix_seconds
    }

    pub(crate) fn next_sequence(self) -> u64 {
        self.last_scanned_sequence.saturating_add(1).max(1)
    }

    pub(crate) fn retry_delayed_at(self, now: SystemTime) -> bool {
        self.retry_after_unix_seconds
            .is_some_and(|retry_after| unix_seconds(now) < retry_after)
    }

    fn record_failure(self, now: SystemTime, policy: CredentialRecoveryPolicy) -> Self {
        let failure_count = self.consecutive_failure_count.saturating_add(1);
        let now_seconds = unix_seconds(now);
        let retry_after = now_seconds.saturating_add(policy.failure_backoff(failure_count).as_secs());
        Self {
            last_scanned_sequence: self.last_scanned_sequence,
            consecutive_failure_count: failure_count,
            first_failure_unix_seconds: self.first_failure_unix_seconds.or(Some(now_seconds)),
            retry_after_unix_seconds: Some(retry_after),
        }
    }

    pub(crate) fn stuck_at(self, now: SystemTime, policy: CredentialRecoveryPolicy) -> bool {
        self.first_failure_unix_seconds.is_some_and(|first_failure| {
            unix_seconds(now).saturating_sub(first_failure) >= policy.stuck_after.as_secs()
        })
    }
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs()
}

pub(crate) trait CredentialRecoveryCheckpointStore: Clone + Send + Sync + 'static {
    fn load(
        &self,
    ) -> impl std::future::Future<Output = Result<CredentialRecoveryCheckpoint, CredentialRecoveryCheckpointStoreError>> + Send;

    fn save(
        &self,
        checkpoint: CredentialRecoveryCheckpoint,
    ) -> impl std::future::Future<Output = Result<(), CredentialRecoveryCheckpointStoreError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialRecoveryCheckpointStoreError {
    #[error("credential recovery checkpoint codec failed: {source}")]
    Codec {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("credential recovery checkpoint backend failed: {source}")]
    Backend {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("credential recovery checkpoint changed concurrently")]
    Conflict,
}

impl CredentialRecoveryCheckpointStoreError {
    fn backend(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Backend {
            source: Box::new(source),
        }
    }

    fn codec(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Codec {
            source: Box::new(source),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CredentialRecoveryKvCheckpointStore<S> {
    kv: S,
}

impl<S> CredentialRecoveryKvCheckpointStore<S>
where
    S: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    pub(crate) fn new(kv: S) -> Self {
        Self { kv }
    }
}

impl<S> CredentialRecoveryCheckpointStore for CredentialRecoveryKvCheckpointStore<S>
where
    S: JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate,
{
    async fn load(&self) -> Result<CredentialRecoveryCheckpoint, CredentialRecoveryCheckpointStoreError> {
        let Some(entry) = self
            .kv
            .entry(CHECKPOINT_KEY.to_string())
            .await
            .map_err(CredentialRecoveryCheckpointStoreError::backend)?
        else {
            return Ok(CredentialRecoveryCheckpoint::default());
        };

        if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
            return Ok(CredentialRecoveryCheckpoint::default());
        }

        decode_checkpoint(&entry.value)
    }

    async fn save(
        &self,
        checkpoint: CredentialRecoveryCheckpoint,
    ) -> Result<(), CredentialRecoveryCheckpointStoreError> {
        let encoded = Bytes::from(encode_checkpoint(checkpoint));
        for _ in 0..3 {
            let Some(entry) = self
                .kv
                .entry(CHECKPOINT_KEY.to_string())
                .await
                .map_err(CredentialRecoveryCheckpointStoreError::backend)?
            else {
                match self.kv.create(CHECKPOINT_KEY, encoded.clone()).await {
                    Ok(_) => return Ok(()),
                    Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(CredentialRecoveryCheckpointStoreError::backend(source)),
                }
            };

            if matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge) {
                match self.kv.create(CHECKPOINT_KEY, encoded.clone()).await {
                    Ok(_) => return Ok(()),
                    Err(source) if source.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(CredentialRecoveryCheckpointStoreError::backend(source)),
                }
            }

            match self.kv.update(CHECKPOINT_KEY, encoded.clone(), entry.revision).await {
                Ok(_) => return Ok(()),
                Err(source) if source.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(source) => return Err(CredentialRecoveryCheckpointStoreError::backend(source)),
            }
        }

        Err(CredentialRecoveryCheckpointStoreError::Conflict)
    }
}

pub(crate) async fn open_checkpoint_store(
    context: jetstream::Context,
) -> Result<CredentialRecoveryKvCheckpointStore<kv::Store>, CredentialRecoveryCheckpointOpenError> {
    let store = provision_checkpoint_bucket::<_, kv::Store>(&context).await?;
    Ok(CredentialRecoveryKvCheckpointStore::new(store))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialRecoveryCheckpointOpenError {
    #[error("failed to create credential recovery checkpoint bucket: {0}")]
    Create(#[source] Box<context::CreateKeyValueError>),
    #[error("failed to open existing credential recovery checkpoint bucket: {0}")]
    OpenExisting(#[source] Box<context::KeyValueError>),
    #[error("failed to inspect credential recovery checkpoint bucket: {0}")]
    Inspect(#[source] kv::StatusError),
    #[error("{source}")]
    Incompatible {
        #[source]
        source: IncompatibleCredentialRecoveryCheckpointBucket,
    },
}

#[derive(Debug, thiserror::Error)]
#[error(
    "credential recovery checkpoint bucket is incompatible: expected history {expected_history}, got {actual_history}; expected max age {expected_max_age:?}, got {actual_max_age:?}"
)]
pub(crate) struct IncompatibleCredentialRecoveryCheckpointBucket {
    expected_history: i64,
    actual_history: i64,
    expected_max_age: Duration,
    actual_max_age: Duration,
}

pub(crate) async fn provision_checkpoint_bucket<C, S>(client: &C) -> Result<S, CredentialRecoveryCheckpointOpenError>
where
    C: JetStreamCreateKeyValue<Store = S> + JetStreamGetKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let store = match client.create_key_value(checkpoint_bucket_config()).await {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => client
            .get_key_value(CREDENTIAL_WORKER_CHECKPOINT_BUCKET)
            .await
            .map_err(|source| CredentialRecoveryCheckpointOpenError::OpenExisting(Box::new(source)))?,
        Err(source) => return Err(CredentialRecoveryCheckpointOpenError::Create(Box::new(source))),
    };

    validate_checkpoint_bucket(&store).await?;
    Ok(store)
}

pub(crate) fn checkpoint_bucket_config() -> kv::Config {
    kv::Config {
        bucket: CREDENTIAL_WORKER_CHECKPOINT_BUCKET.to_string(),
        history: 1,
        max_age: Duration::ZERO,
        ..Default::default()
    }
}

async fn validate_checkpoint_bucket<S>(store: &S) -> Result<(), CredentialRecoveryCheckpointOpenError>
where
    S: JetStreamKeyValueStatus,
{
    let status = store
        .status()
        .await
        .map_err(CredentialRecoveryCheckpointOpenError::Inspect)?;
    let history = status.history();
    let max_age = status.max_age();
    if history != 1 || max_age != Duration::ZERO {
        return Err(CredentialRecoveryCheckpointOpenError::Incompatible {
            source: IncompatibleCredentialRecoveryCheckpointBucket {
                expected_history: 1,
                actual_history: history,
                expected_max_age: Duration::ZERO,
                actual_max_age: max_age,
            },
        });
    }
    Ok(())
}

fn encode_checkpoint(checkpoint: CredentialRecoveryCheckpoint) -> Vec<u8> {
    proto::CredentialRecoveryWorkerCheckpoint {
        last_scanned_sequence: Some(checkpoint.last_scanned_sequence()),
        consecutive_failure_count: Some(checkpoint.consecutive_failure_count()),
        first_failure_unix_seconds: checkpoint.first_failure_unix_seconds,
        retry_after_unix_seconds: checkpoint.retry_after_unix_seconds(),
    }
    .encode_to_vec()
}

fn decode_checkpoint(value: &[u8]) -> Result<CredentialRecoveryCheckpoint, CredentialRecoveryCheckpointStoreError> {
    let checkpoint = proto::CredentialRecoveryWorkerCheckpoint::decode_from_slice(value)
        .map_err(CredentialRecoveryCheckpointStoreError::codec)?;
    Ok(CredentialRecoveryCheckpoint::with_failure_state(
        checkpoint.last_scanned_sequence.unwrap_or_default(),
        checkpoint.consecutive_failure_count.unwrap_or_default(),
        checkpoint.first_failure_unix_seconds,
        checkpoint.retry_after_unix_seconds,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, UNIX_EPOCH};

    use async_nats::HeaderMap;
    use async_nats::header::NATS_MESSAGE_ID;
    use async_nats::jetstream::message::StreamMessage;
    use bytes::Bytes;
    use chrono::Utc;
    use time::OffsetDateTime;
    use trogon_decider_nats::TROGON_EVENT_TYPE;
    use trogon_decider_runtime::{
        AppendStreamRequest, AppendStreamResponse, Event, EventEncode, EventId, EventType, Headers, ReadFrom,
        ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest, ReadStreamResponse, Snapshot, SnapshotRead,
        SnapshotWrite, StreamPosition, StreamWritePrecondition, WriteSnapshotRequest, WriteSnapshotResponse,
    };
    use trogon_nats::jetstream::{
        JetStreamGetStream, MockJetStreamConsumerFactory, MockJetStreamKvClient, MockJetStreamKvStore,
    };
    use trogon_std::SecretString;
    use uuid::Uuid;

    use super::*;
    use crate::credential::commands::domain::{
        CredentialKind, CredentialOwnerId, CredentialRef, CredentialScope, CredentialVersion, SourceKind,
    };
    use crate::credential::handler::{CredentialHandler, PutCredential, RotateCredential};
    use crate::credential::processor::runtime_projection::{RuntimeCredentialRegistry, RuntimeIntegrationKey};
    use crate::credential::proto::{decode_active_state, write_requested_to_proto};
    use crate::secret_store::MockOpenBaoSecretStore;
    use crate::source_integration_id::SourceIntegrationId;
    use trogonai_proto::gateway::credentials::CredentialStateSnapshotCase;

    #[derive(Debug, thiserror::Error)]
    #[error("worker test stream store rejected the append")]
    struct WorkerTestStreamStoreError;

    #[derive(Clone, Default)]
    struct WorkerTestStreamStore {
        events: Arc<Mutex<Vec<StreamEvent>>>,
        snapshots: Arc<Mutex<BTreeMap<String, Snapshot<state_v1::CredentialStateSnapshot>>>>,
        write_preconditions: Arc<Mutex<Vec<StreamWritePrecondition>>>,
        fail_append_at: Arc<Mutex<Option<usize>>>,
    }

    impl WorkerTestStreamStore {
        fn fail_append_at(&self, append_count: usize) {
            *self.fail_append_at.lock().unwrap() = Some(append_count);
        }

        fn events_as_raw_stream_scan(&self) -> Vec<StreamEvent> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .map(|mut event| {
                    event.stream_id = "gateway.credentials.events.v1.subject".to_string();
                    event
                })
                .collect()
        }

        fn push_credential_event(&self, stream_id: &str, event: v1::CredentialEvent) {
            let mut events = self.events.lock().unwrap();
            let stream_position = position(events.len() as u64 + 1);
            events.push(StreamEvent {
                stream_id: stream_id.to_string(),
                event: runtime_event(stream_position.as_u64(), event),
                stream_position,
                recorded_at: Utc::now(),
            });
        }
    }

    impl StreamRead<str> for WorkerTestStreamStore {
        type Error = WorkerTestStreamStoreError;

        async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
            let start = match request.from {
                ReadFrom::Beginning => 1,
                ReadFrom::Position(position) => position.as_u64(),
            };
            let events = self.events.lock().unwrap();
            Ok(ReadStreamResponse {
                current_position: current_position(&events, request.stream_id),
                events: events
                    .iter()
                    .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                    .cloned()
                    .collect(),
            })
        }
    }

    impl StreamAppend<str> for WorkerTestStreamStore {
        type Error = WorkerTestStreamStoreError;

        async fn append_stream(
            &self,
            request: AppendStreamRequest<'_, str>,
        ) -> Result<AppendStreamResponse, Self::Error> {
            let mut events = self.events.lock().unwrap();
            let append_count = self.write_preconditions.lock().unwrap().len() + 1;
            self.write_preconditions
                .lock()
                .unwrap()
                .push(request.stream_write_precondition);
            {
                let mut fail_append_at = self.fail_append_at.lock().unwrap();
                if *fail_append_at == Some(append_count) {
                    *fail_append_at = None;
                    return Err(WorkerTestStreamStoreError);
                }
            }

            let current_position = current_position(&events, request.stream_id);
            match request.stream_write_precondition {
                StreamWritePrecondition::Any => {}
                StreamWritePrecondition::StreamExists if current_position.is_some() => {}
                StreamWritePrecondition::NoStream if current_position.is_none() => {}
                StreamWritePrecondition::At(position) if current_position == Some(position) => {}
                _ => return Err(WorkerTestStreamStoreError),
            }

            let mut last_position = current_position;
            for event in request.events {
                let stream_position = position(events.len() as u64 + 1);
                last_position = Some(stream_position);
                events.push(StreamEvent {
                    stream_id: request.stream_id.to_string(),
                    event,
                    stream_position,
                    recorded_at: Utc::now(),
                });
            }

            Ok(AppendStreamResponse {
                stream_position: last_position.expect("append request must contain events"),
            })
        }
    }

    impl SnapshotRead<state_v1::CredentialStateSnapshot, str> for WorkerTestStreamStore {
        type Error = WorkerTestStreamStoreError;

        async fn read_snapshot(
            &self,
            request: ReadSnapshotRequest<'_, str>,
        ) -> Result<ReadSnapshotResponse<state_v1::CredentialStateSnapshot>, Self::Error> {
            Ok(ReadSnapshotResponse {
                snapshot: self.snapshots.lock().unwrap().get(request.snapshot_id).cloned(),
            })
        }
    }

    impl SnapshotWrite<state_v1::CredentialStateSnapshot, str> for WorkerTestStreamStore {
        type Error = WorkerTestStreamStoreError;

        async fn write_snapshot(
            &self,
            request: WriteSnapshotRequest<'_, state_v1::CredentialStateSnapshot, str>,
        ) -> Result<WriteSnapshotResponse, Self::Error> {
            self.snapshots
                .lock()
                .unwrap()
                .insert(request.snapshot_id.to_string(), request.snapshot);
            Ok(WriteSnapshotResponse)
        }
    }

    #[tokio::test]
    async fn recovery_plan_groups_by_payload_credential_id_instead_of_raw_subject() {
        let store = WorkerTestStreamStore::default();
        store.push_credential_event(credential_id().as_str(), write_requested_event());
        let events = store.events_as_raw_stream_scan();

        let plan = recovery_plan_from_credential_events(events, &store).await.unwrap();

        assert_eq!(plan.report.scanned_events(), 1);
        assert_eq!(plan.report.decoded_events(), 1);
        assert_eq!(plan.report.planned_recoveries(), 1);
        assert_eq!(
            plan.commands,
            vec![PlannedCredentialRecovery {
                credential_id: credential_id(),
                command: CredentialActivationRecoveryCommand::Write(
                    crate::credential::handler::RecoverCredentialWriteActivation::new(credential_ref(1))
                ),
            }]
        );
    }

    #[tokio::test]
    async fn worker_recovers_pending_write_activation_after_secret_store_success() {
        let events = WorkerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        events.fail_append_at(2);
        let error = handler.put(put_command("super-secret")).await.unwrap_err();
        assert!(error.to_string().contains("credential write activation failed"));

        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler =
            CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
        let report = recover_pending_credential_activations_from_events(
            events.events_as_raw_stream_scan(),
            &events,
            &runtime_handler,
        )
        .await
        .unwrap();

        assert_eq!(report.planned_recoveries(), 1);
        assert_eq!(report.recovered_writes(), 1);
        assert_eq!(report.failed_recoveries(), 0);
        assert_eq!(
            runtime_credentials
                .resolver(secrets)
                .resolve(&runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "super-secret"
        );
    }

    #[tokio::test]
    async fn worker_recovers_pending_rotation_activation_after_secret_store_success() {
        let events = WorkerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        let active = handler.put(put_command("old-secret")).await.unwrap();
        let active = active.into_state();
        let Some(CredentialStateSnapshotCase::Active(active)) = active.state.as_ref() else {
            panic!("expected active credential");
        };
        let (active_metadata, _) = decode_active_state("active", active).unwrap();
        let active_ref = active_metadata.reference().clone();
        events.fail_append_at(4);
        let error = handler
            .rotate(RotateCredential::new(
                active_ref,
                SecretString::new("new-secret").unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("credential rotation activation failed"));

        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler =
            CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());
        let report = recover_pending_credential_activations_from_events(
            events.events_as_raw_stream_scan(),
            &events,
            &runtime_handler,
        )
        .await
        .unwrap();

        assert_eq!(report.planned_recoveries(), 1);
        assert_eq!(report.recovered_rotations(), 1);
        assert_eq!(report.failed_recoveries(), 0);
        assert_eq!(
            runtime_credentials
                .resolver(secrets)
                .resolve(&runtime_key(), CredentialKind::WebhookSecret)
                .await
                .unwrap()
                .as_plaintext()
                .unwrap()
                .as_str(),
            "new-secret"
        );
    }

    #[tokio::test]
    async fn worker_continues_when_recovery_metadata_is_not_available() {
        let store = WorkerTestStreamStore::default();
        store.push_credential_event(credential_id().as_str(), write_requested_event());
        let events = store.events_as_raw_stream_scan();
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

        let report = recover_pending_credential_activations_from_events(events, &store, &runtime_handler)
            .await
            .unwrap();

        assert_eq!(report.planned_recoveries(), 1);
        assert_eq!(report.recovered_writes(), 0);
        assert_eq!(report.failed_recoveries(), 1);
    }

    #[tokio::test]
    async fn checkpoint_store_loads_default_when_record_is_missing() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

        let checkpoint = checkpoints.load().await.unwrap();

        assert_eq!(checkpoint.last_scanned_sequence(), 0);
        assert_eq!(kv.entry_calls(), vec![CHECKPOINT_KEY.to_string()]);
        assert!(kv.create_calls().is_empty());
        assert!(kv.update_calls().is_empty());
    }

    #[tokio::test]
    async fn checkpoint_store_creates_checkpoint_when_record_is_missing() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

        checkpoints.save(CredentialRecoveryCheckpoint::new(42)).await.unwrap();

        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, CHECKPOINT_KEY);
        assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 42);
        assert!(kv.update_calls().is_empty());
    }

    #[tokio::test]
    async fn checkpoint_store_updates_checkpoint_when_record_exists() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(
            Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(7))),
            11,
            kv::Operation::Put,
        );
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());

        checkpoints.save(CredentialRecoveryCheckpoint::new(43)).await.unwrap();

        let calls = kv.update_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, CHECKPOINT_KEY);
        assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 43);
        assert_eq!(calls[0].2, 11);
        assert!(kv.create_calls().is_empty());
    }

    #[tokio::test]
    async fn checkpoint_codec_preserves_retry_state() {
        let original = CredentialRecoveryCheckpoint::with_failure_state(9, 3, Some(100), Some(220));

        let decoded = decode_checkpoint(&encode_checkpoint(original)).unwrap();

        assert_eq!(decoded.last_scanned_sequence(), 9);
        assert_eq!(decoded.consecutive_failure_count(), 3);
        assert_eq!(decoded.first_failure_unix_seconds, Some(100));
        assert_eq!(decoded.retry_after_unix_seconds(), Some(220));
    }

    #[tokio::test]
    async fn worker_advances_checkpoint_after_successful_recovery_scan() {
        let events = WorkerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        events.fail_append_at(2);
        handler.put(put_command("super-secret")).await.unwrap_err();

        let stream = raw_stream_with_events(events.events_as_raw_stream_scan()).await;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        kv.enqueue_entry_none();
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler =
            CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials.clone());

        let report = recover_pending_credential_activations(&stream, &events, &runtime_handler, &checkpoints)
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 1);
        assert_eq!(report.planned_recoveries(), 1);
        assert_eq!(report.recovered_writes(), 1);
        assert_eq!(report.failed_recoveries(), 0);
        assert_eq!(report.checkpoint_loaded_sequence(), 0);
        assert_eq!(report.checkpoint_advanced_to(), Some(1));
        assert_eq!(report.checkpoint_failure_count(), 0);
        assert_eq!(report.checkpoint_retry_after_unix_seconds(), None);
        assert!(!report.retry_delayed());
        assert!(!report.stuck_recovery());
        assert_eq!(stream.raw_message_calls(), vec![1]);
        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 1);
    }

    #[tokio::test]
    async fn worker_starts_scan_after_loaded_checkpoint() {
        let events = WorkerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        events.fail_append_at(2);
        handler.put(put_command("super-secret")).await.unwrap_err();

        let raw_events = vec![
            stream_event("gateway.credentials.events.v1.old", 1, write_requested_event()),
            stream_event("gateway.credentials.events.v1.current", 2, write_requested_event()),
        ];
        let stream = raw_stream_with_events(raw_events).await;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(
            Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1))),
            3,
            kv::Operation::Put,
        );
        kv.enqueue_entry(
            Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::new(1))),
            4,
            kv::Operation::Put,
        );
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials);

        let report = recover_pending_credential_activations(&stream, &events, &runtime_handler, &checkpoints)
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 1);
        assert_eq!(report.checkpoint_loaded_sequence(), 1);
        assert_eq!(report.checkpoint_advanced_to(), Some(2));
        assert_eq!(stream.raw_message_calls(), vec![2]);
        let calls = kv.update_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(decode_checkpoint(&calls[0].1).unwrap().last_scanned_sequence(), 2);
        assert_eq!(calls[0].2, 4);
    }

    #[tokio::test]
    async fn worker_does_not_advance_checkpoint_when_recovery_fails() {
        let store = WorkerTestStreamStore::default();
        store.push_credential_event(credential_id().as_str(), write_requested_event());
        let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

        let report = recover_pending_credential_activations(&stream, &store, &runtime_handler, &checkpoints)
            .await
            .unwrap();

        assert_eq!(report.scanned_events(), 1);
        assert_eq!(report.planned_recoveries(), 1);
        assert_eq!(report.failed_recoveries(), 1);
        assert_eq!(report.checkpoint_loaded_sequence(), 0);
        assert_eq!(report.checkpoint_advanced_to(), None);
        assert_eq!(report.checkpoint_failure_count(), 1);
        assert!(report.checkpoint_retry_after_unix_seconds().is_some());
        assert_eq!(stream.raw_message_calls(), vec![1]);
        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        let saved = decode_checkpoint(&calls[0].1).unwrap();
        assert_eq!(saved.last_scanned_sequence(), 0);
        assert_eq!(saved.consecutive_failure_count(), 1);
        assert!(saved.first_failure_unix_seconds.is_some());
        assert!(saved.retry_after_unix_seconds().is_some());
        assert!(kv.update_calls().is_empty());
    }

    #[tokio::test]
    async fn worker_saves_retry_backoff_when_recovery_fails() {
        let store = WorkerTestStreamStore::default();
        store.push_credential_event(credential_id().as_str(), write_requested_event());
        let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry_none();
        kv.enqueue_entry_none();
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);
        let now = UNIX_EPOCH + Duration::from_secs(1_000);

        let report = recover_pending_credential_activations_at(
            &stream,
            &store,
            &runtime_handler,
            &checkpoints,
            now,
            test_policy(),
        )
        .await
        .unwrap();

        assert_eq!(report.failed_recoveries(), 1);
        assert_eq!(report.checkpoint_failure_count(), 1);
        assert_eq!(report.checkpoint_retry_after_unix_seconds(), Some(1_010));
        assert_eq!(stream.raw_message_calls(), vec![1]);
        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        let saved = decode_checkpoint(&calls[0].1).unwrap();
        assert_eq!(saved.last_scanned_sequence(), 0);
        assert_eq!(saved.consecutive_failure_count(), 1);
        assert_eq!(saved.first_failure_unix_seconds, Some(1_000));
        assert_eq!(saved.retry_after_unix_seconds(), Some(1_010));
    }

    #[tokio::test]
    async fn worker_skips_scan_until_retry_after_is_reached() {
        let store = WorkerTestStreamStore::default();
        store.push_credential_event(credential_id().as_str(), write_requested_event());
        let stream = raw_stream_with_events(store.events_as_raw_stream_scan()).await;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(
            Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::with_failure_state(
                0,
                2,
                Some(900),
                Some(1_000),
            ))),
            4,
            kv::Operation::Put,
        );
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

        let report = recover_pending_credential_activations_at(
            &stream,
            &store,
            &runtime_handler,
            &checkpoints,
            UNIX_EPOCH + Duration::from_secs(999),
            test_policy(),
        )
        .await
        .unwrap();

        assert_eq!(report.scanned_events(), 0);
        assert_eq!(report.failed_recoveries(), 0);
        assert_eq!(report.checkpoint_loaded_sequence(), 0);
        assert_eq!(report.checkpoint_failure_count(), 2);
        assert_eq!(report.checkpoint_retry_after_unix_seconds(), Some(1_000));
        assert!(report.retry_delayed());
        assert!(!report.stuck_recovery());
        assert!(stream.raw_message_calls().is_empty());
        assert!(kv.create_calls().is_empty());
        assert!(kv.update_calls().is_empty());
    }

    #[tokio::test]
    async fn worker_reports_stuck_recovery_after_failure_age_threshold() {
        let store = WorkerTestStreamStore::default();
        let stream = raw_stream_with_events(Vec::new()).await;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(
            Bytes::from(encode_checkpoint(CredentialRecoveryCheckpoint::with_failure_state(
                0,
                4,
                Some(100),
                Some(1_000),
            ))),
            4,
            kv::Operation::Put,
        );
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv);
        let secrets = MockOpenBaoSecretStore::default();
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler = CredentialRuntimeHandler::new(store.clone(), secrets, runtime_credentials);

        let report = recover_pending_credential_activations_at(
            &stream,
            &store,
            &runtime_handler,
            &checkpoints,
            UNIX_EPOCH + Duration::from_secs(999),
            test_policy(),
        )
        .await
        .unwrap();

        assert!(report.retry_delayed());
        assert!(report.stuck_recovery());
        assert_eq!(report.checkpoint_failure_count(), 4);
    }

    #[tokio::test]
    async fn worker_resets_retry_state_after_successful_recovery() {
        let events = WorkerTestStreamStore::default();
        let secrets = MockOpenBaoSecretStore::default();
        let handler = CredentialHandler::new(events.clone(), secrets.clone());
        events.fail_append_at(2);
        handler.put(put_command("super-secret")).await.unwrap_err();

        let stream = raw_stream_with_events(events.events_as_raw_stream_scan()).await;
        let checkpoint = CredentialRecoveryCheckpoint::with_failure_state(0, 2, Some(1_000), Some(1_010));
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(Bytes::from(encode_checkpoint(checkpoint)), 7, kv::Operation::Put);
        kv.enqueue_entry(Bytes::from(encode_checkpoint(checkpoint)), 8, kv::Operation::Put);
        let checkpoints = CredentialRecoveryKvCheckpointStore::new(kv.clone());
        let runtime_credentials = RuntimeCredentialRegistry::default();
        let runtime_handler = CredentialRuntimeHandler::new(events.clone(), secrets.clone(), runtime_credentials);

        let report = recover_pending_credential_activations_at(
            &stream,
            &events,
            &runtime_handler,
            &checkpoints,
            UNIX_EPOCH + Duration::from_secs(1_011),
            test_policy(),
        )
        .await
        .unwrap();

        assert_eq!(report.failed_recoveries(), 0);
        assert_eq!(report.recovered_writes(), 1);
        assert_eq!(report.checkpoint_advanced_to(), Some(1));
        assert_eq!(report.checkpoint_failure_count(), 0);
        assert_eq!(report.checkpoint_retry_after_unix_seconds(), None);
        let calls = kv.update_calls();
        assert_eq!(calls.len(), 1);
        let saved = decode_checkpoint(&calls[0].1).unwrap();
        assert_eq!(saved.last_scanned_sequence(), 1);
        assert_eq!(saved.consecutive_failure_count(), 0);
        assert_eq!(saved.first_failure_unix_seconds, None);
        assert_eq!(saved.retry_after_unix_seconds(), None);
    }

    #[test]
    fn recovery_report_metric_outcome_is_bounded() {
        assert_eq!(
            CredentialRecoveryReport {
                retry_delayed: true,
                ..Default::default()
            }
            .metric_outcome(),
            "retry_delayed"
        );
        assert_eq!(
            CredentialRecoveryReport {
                failed_recoveries: 1,
                ..Default::default()
            }
            .metric_outcome(),
            "failed_recovery"
        );
        assert_eq!(
            CredentialRecoveryReport {
                checkpoint_advanced_to: Some(7),
                ..Default::default()
            }
            .metric_outcome(),
            "advanced"
        );
        assert_eq!(
            CredentialRecoveryReport {
                recovered_writes: 1,
                ..Default::default()
            }
            .metric_outcome(),
            "recovered"
        );
        assert_eq!(
            CredentialRecoveryReport {
                stuck_recovery: true,
                ..Default::default()
            }
            .metric_outcome(),
            "stuck"
        );
        assert_eq!(CredentialRecoveryReport::default().metric_outcome(), "idle");
    }

    #[test]
    fn recovery_metrics_recording_is_infallible() {
        let metrics = CredentialRecoveryMetrics::new();
        let report = CredentialRecoveryReport {
            scanned_events: 5,
            planned_recoveries: 2,
            recovered_writes: 1,
            recovered_rotations: 1,
            failed_recoveries: 1,
            stuck_recovery: true,
            ..Default::default()
        };

        metrics.record_report(&report);
        metrics.record_error("test_error");
    }

    #[tokio::test]
    async fn provision_checkpoint_bucket_uses_expected_runtime_contract() {
        let client = MockJetStreamKvClient::new();

        let _store: MockJetStreamKvStore = provision_checkpoint_bucket(&client).await.unwrap();

        let configs = client.create_configs();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].bucket, CREDENTIAL_WORKER_CHECKPOINT_BUCKET);
        assert_eq!(configs[0].history, 1);
        assert_eq!(configs[0].max_age, Duration::ZERO);
    }

    #[tokio::test]
    async fn provision_checkpoint_bucket_opens_existing_bucket() {
        let existing = MockJetStreamKvStore::new();
        let client = MockJetStreamKvClient::new();
        client.fail_create_already_exists();
        client.set_get_result(existing);

        let _store: MockJetStreamKvStore = provision_checkpoint_bucket(&client).await.unwrap();

        assert_eq!(
            client.requested_buckets(),
            vec![CREDENTIAL_WORKER_CHECKPOINT_BUCKET.to_string()]
        );
    }

    fn current_position(events: &[StreamEvent], stream_id: &str) -> Option<StreamPosition> {
        events
            .iter()
            .filter(|event| event.stream_id() == stream_id)
            .map(|event| event.stream_position)
            .max()
    }

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).unwrap()
    }

    fn stream_event(stream_id: &str, stream_position: u64, event: v1::CredentialEvent) -> StreamEvent {
        StreamEvent {
            stream_id: stream_id.to_string(),
            event: runtime_event(stream_position, event),
            stream_position: position(stream_position),
            recorded_at: Utc::now(),
        }
    }

    fn write_requested_event() -> v1::CredentialEvent {
        v1::CredentialEvent {
            event: Some(
                write_requested_to_proto(
                    &credential_id(),
                    &owner_id(),
                    SourceKind::GitHub,
                    CredentialKind::WebhookSecret,
                )
                .into(),
            ),
        }
    }

    async fn raw_stream_with_events(events: Vec<StreamEvent>) -> trogon_nats::jetstream::MockJetStreamStream {
        let stream = MockJetStreamConsumerFactory::new();
        stream.set_info(make_stream_info(
            events
                .iter()
                .map(|event| event.stream_position.as_u64())
                .max()
                .unwrap_or(0),
        ));
        for event in events {
            stream.add_raw_message(event.stream_position.as_u64(), raw_stream_message(event));
        }
        stream.get_stream("gateway.credentials.events.v1").await.unwrap()
    }

    fn raw_stream_message(event: StreamEvent) -> StreamMessage {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, event.event.id.to_string().as_str());
        headers.insert(TROGON_EVENT_TYPE, event.event.r#type.as_str());
        StreamMessage {
            subject: event.stream_id.into(),
            sequence: event.stream_position.as_u64(),
            headers,
            payload: Bytes::from(event.event.content),
            time: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn make_stream_info(last_sequence: u64) -> async_nats::jetstream::stream::Info {
        serde_json::from_value(serde_json::json!({
            "config": {
                "name": "GATEWAY_CREDENTIAL_EVENTS",
                "subjects": [],
                "retention": "limits",
                "max_consumers": -1,
                "max_msgs": -1,
                "max_bytes": -1,
                "discard": "old",
                "max_age": 0,
                "storage": "file",
                "num_replicas": 1
            },
            "created": "1970-01-01T00:00:00Z",
            "state": {
                "messages": last_sequence,
                "bytes": 0_u64,
                "first_seq": if last_sequence == 0 { 0_u64 } else { 1_u64 },
                "first_ts": "1970-01-01T00:00:00Z",
                "last_seq": last_sequence,
                "last_ts": "1970-01-01T00:00:00Z",
                "consumer_count": 0_usize,
                "num_subjects": 0_u64
            },
            "cluster": null,
            "mirror": null,
            "sources": []
        }))
        .expect("test stream info must be valid")
    }

    fn runtime_event(id: u64, event: v1::CredentialEvent) -> Event {
        Event {
            id: EventId::new(Uuid::from_u128(id as u128)),
            r#type: EventType::event_type(&event).unwrap().to_string(),
            content: EventEncode::encode(&event).unwrap(),
            headers: Headers::empty(),
        }
    }

    fn owner_id() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
    }

    fn integration_id() -> SourceIntegrationId {
        SourceIntegrationId::new("primary").unwrap()
    }

    fn scope() -> CredentialScope {
        CredentialScope::integration(owner_id(), SourceKind::GitHub, integration_id())
    }

    fn credential_id() -> CredentialId {
        CredentialId::new("openbao:tenant-1:github/primary:webhook_secret").unwrap()
    }

    fn credential_ref(version: u64) -> CredentialRef {
        CredentialRef::new(
            credential_id(),
            CredentialVersion::new(version).unwrap(),
            &scope(),
            CredentialKind::WebhookSecret,
        )
    }

    fn runtime_key() -> RuntimeIntegrationKey {
        RuntimeIntegrationKey::new(SourceKind::GitHub, &integration_id())
    }

    fn test_policy() -> CredentialRecoveryPolicy {
        CredentialRecoveryPolicy {
            initial_failure_backoff: Duration::from_secs(10),
            max_failure_backoff: Duration::from_secs(40),
            stuck_after: Duration::from_secs(100),
        }
    }

    fn put_command(value: &str) -> PutCredential {
        PutCredential::new(
            credential_id(),
            scope(),
            CredentialKind::WebhookSecret,
            SecretString::new(value).unwrap(),
        )
    }
}
