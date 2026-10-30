use std::collections::BTreeSet;
use std::error::Error;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, context, kv};
use buffa::Message as _;
use bytes::Bytes;
use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use tracing::{error, info, warn};
use trogon_decider_runtime::{
    EventDecodeOutcome, ReadFrom, ReadStreamRequest, SnapshotRead, SnapshotWrite, StreamAppend, StreamEvent, StreamRead,
};
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamGetRawMessage, JetStreamGetStreamInfo,
    JetStreamKeyValueStatus, JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry,
    is_create_key_value_already_exists,
};
use trogon_semconv::{attribute, metric};
use trogonai_proto::gateway::credentials::checkpoints_v1 as proto;
use trogonai_proto::gateway::credentials::{CredentialEventCase, CredentialEventPayloadError, state_v1, v1};

use super::constants::{
    CHECKPOINT_KEY, CREDENTIAL_WORKER_CHECKPOINT_BUCKET, DEFAULT_INITIAL_FAILURE_BACKOFF, DEFAULT_MAX_FAILURE_BACKOFF,
    DEFAULT_STUCK_AFTER,
};
use super::handler::{CredentialActivationRecoveryCommand, CredentialRuntimeHandler, activation_recovery_command};
use crate::constants::METER_NAME;
use crate::credential::commands::domain::{CredentialId, CredentialVersion};
use crate::credential::commands::proto::{
    CredentialProtoDecodeError, decode_credential_metadata, decode_destroy_failed, decode_destroy_requested,
    decode_destroyed, decode_message_field, decode_revoked, decode_rotated, decode_rotation_failed,
    decode_rotation_requested, decode_write_failed, decode_write_requested,
};
use crate::credential::event_stream::{CredentialEventStreamReadError, read_credential_event_stream};
use crate::credential::{CredentialEvolveError, evolve, initial_state};
use crate::secret_store::{SecretStoreError, SecretStoreMetadata};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CredentialRecoveryReport {
    scanned_events: usize,
    decoded_events: usize,
    skipped_events: usize,
    invalid_events: usize,
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

    pub(crate) fn invalid_events(&self) -> usize {
        self.invalid_events
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
        self.planned_recoveries > 0
            || self.has_failures()
            || self.invalid_events > 0
            || self.retry_delayed
            || self.stuck_recovery
    }

    fn has_failures(&self) -> bool {
        self.failed_recoveries > 0
    }

    fn metric_outcome(&self) -> &'static str {
        if self.stuck_recovery {
            return "stuck";
        }
        if self.retry_delayed {
            return "retry_delayed";
        }
        if self.invalid_events > 0 {
            return "invalid_event";
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
    Secrets: SecretStoreMetadata<Error = SecretStoreError>,
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
                            invalid_events = report.invalid_events(),
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
        let meter = trogon_telemetry::meter(METER_NAME);
        Self {
            passes: metric::build_gateway_credential_recovery_passes(&meter),
            errors: metric::build_gateway_credential_recovery_errors(&meter),
            scanned_events: metric::build_gateway_credential_recovery_scanned_events(&meter),
            recoveries: metric::build_gateway_credential_recovery_recoveries(&meter),
            stuck_reports: metric::build_gateway_credential_recovery_stuck_reports(&meter),
        }
    }

    fn record_report(&self, report: &CredentialRecoveryReport) {
        self.passes
            .add(1, &[KeyValue::new(attribute::OUTCOME, report.metric_outcome())]);
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
        self.passes.add(1, &[KeyValue::new(attribute::OUTCOME, "error")]);
        self.errors.add(1, &[KeyValue::new(attribute::REASON, reason)]);
    }

    fn record_recovery_count(&self, status: &'static str, kind: &'static str, count: usize) {
        if count == 0 {
            return;
        }
        self.recoveries.add(
            count as u64,
            &[
                KeyValue::new(attribute::STATUS, status),
                KeyValue::new(attribute::KIND, kind),
            ],
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
    Secrets: SecretStoreMetadata<Error = SecretStoreError>,
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
    Secrets: SecretStoreMetadata<Error = SecretStoreError>,
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
    let events = read_credential_event_stream(event_stream, from_sequence)
        .await
        .map_err(|source| CredentialRecoveryWorkerError::ReadStream { source })?;
    let max_scanned_sequence = events.iter().map(|event| event.stream_position.as_u64()).max();
    let mut report = recover_pending_credential_activations_from_events(events, event_store, handler).await;
    report.checkpoint_loaded_sequence = checkpoint.last_scanned_sequence();
    report.checkpoint_failure_count = checkpoint.consecutive_failure_count();
    report.checkpoint_retry_after_unix_seconds = checkpoint.retry_after_unix_seconds();
    report.stuck_recovery = checkpoint.stuck_at(now, policy);

    if !report.has_failures() {
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
            invalid_events = report.invalid_events(),
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
) -> CredentialRecoveryReport
where
    EventStore: StreamRead<str>
        + StreamAppend<str>
        + SnapshotRead<state_v1::CredentialStateSnapshot, str>
        + SnapshotWrite<state_v1::CredentialStateSnapshot, str>
        + Clone
        + 'static,
    Secrets: SecretStoreMetadata<Error = SecretStoreError>,
{
    let plan = recovery_plan_from_credential_events(events, event_store).await;
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

    report
}

async fn recovery_plan_from_credential_events<EventStore>(
    events: impl IntoIterator<Item = StreamEvent>,
    event_store: &EventStore,
) -> CredentialRecoveryPlan
where
    EventStore: StreamRead<str>,
{
    let mut report = CredentialRecoveryReport::default();
    let mut changed_credentials: BTreeSet<CredentialId> = BTreeSet::new();

    for event in events {
        report.scanned_events += 1;
        match stream_event_credential_id(&event) {
            Ok(Some(credential_id)) => {
                report.decoded_events += 1;
                changed_credentials.insert(credential_id);
            }
            Ok(None) => {
                report.skipped_events += 1;
            }
            Err(source) => {
                report.invalid_events += 1;
                warn!(
                    stream_sequence = event.stream_position.as_u64(),
                    error = %source,
                    "credential recovery found an invalid credential event"
                );
            }
        }
    }
    report.changed_credentials = changed_credentials.len();

    let mut commands = Vec::new();
    for credential_id in changed_credentials {
        let (state, destroyed_version) = match load_credential_state(event_store, &credential_id).await {
            Ok(loaded) => loaded,
            Err(source) => {
                report.failed_recoveries += 1;
                warn!(
                    credential_id = %credential_id,
                    error = %source,
                    "credential recovery replay failed"
                );
                continue;
            }
        };

        match activation_recovery_command(&credential_id, &state, destroyed_version) {
            Ok(Some(command)) => {
                report.planned_recoveries += 1;
                commands.push(PlannedCredentialRecovery { credential_id, command });
            }
            Ok(None) => {
                report.skipped_credentials += 1;
            }
            Err(source) => {
                report.failed_recoveries += 1;
                warn!(
                    credential_id = %credential_id,
                    error = %source,
                    "credential recovery planning failed"
                );
            }
        }
    }

    CredentialRecoveryPlan { report, commands }
}

fn stream_event_credential_id(event: &StreamEvent) -> Result<Option<CredentialId>, CredentialRecoveryEventError> {
    let EventDecodeOutcome::Decoded(event) = event
        .decode::<v1::CredentialEvent>()
        .map_err(|source| CredentialRecoveryEventError::Decode { source })?
    else {
        return Ok(None);
    };
    let case = event.event.as_ref().ok_or(CredentialRecoveryEventError::MissingCase)?;
    event_credential_id(case)
        .map(Some)
        .map_err(|source| CredentialRecoveryEventError::Invalid { source })
}

async fn load_credential_state<EventStore>(
    event_store: &EventStore,
    credential_id: &CredentialId,
) -> Result<(state_v1::CredentialStateSnapshot, Option<CredentialVersion>), CredentialRecoveryReplayError>
where
    EventStore: StreamRead<str>,
{
    let stream = event_store
        .read_stream(ReadStreamRequest {
            stream_id: credential_id.as_str(),
            from: ReadFrom::Beginning,
        })
        .await
        .map_err(|source| CredentialRecoveryReplayError::ReadCredential {
            source: Box::new(source),
        })?;
    let mut state = initial_state();
    let mut destroyed_version = None;
    for event in stream.events {
        let EventDecodeOutcome::Decoded(event) = event
            .decode::<v1::CredentialEvent>()
            .map_err(|source| CredentialRecoveryReplayError::DecodeEvent { source })?
        else {
            continue;
        };
        if let Some(CredentialEventCase::Destroyed(destroyed)) = event.event.as_ref() {
            destroyed_version = Some(
                decode_destroyed(destroyed)
                    .map_err(|source| CredentialRecoveryReplayError::DecodeDestroyed { source })?
                    .version(),
            );
        }
        state = evolve(state, &event).map_err(|source| CredentialRecoveryReplayError::ReplayCredential { source })?;
    }
    Ok((state, destroyed_version))
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
        CredentialEventCase::DestroyRequested(inner) => Ok(decode_destroy_requested(inner)?.0.id().clone()),
        CredentialEventCase::Destroyed(inner) => Ok(decode_destroyed(inner)?.id().clone()),
        CredentialEventCase::DestroyFailed(inner) => Ok(decode_destroy_failed(inner)?.0.id().clone()),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialRecoveryWorkerError {
    #[error("credential event stream read failed: {source}")]
    ReadStream {
        #[source]
        source: CredentialEventStreamReadError,
    },
    #[error("credential recovery checkpoint failed: {source}")]
    Checkpoint {
        #[source]
        source: CredentialRecoveryCheckpointStoreError,
    },
}

#[derive(Debug, thiserror::Error)]
enum CredentialRecoveryEventError {
    #[error("credential event decode failed: {source}")]
    Decode {
        #[source]
        source: CredentialEventPayloadError,
    },
    #[error("credential event is missing its event case")]
    MissingCase,
    #[error("credential event is invalid: {source}")]
    Invalid {
        #[source]
        source: CredentialProtoDecodeError,
    },
}

#[derive(Debug, thiserror::Error)]
enum CredentialRecoveryReplayError {
    #[error("credential stream read failed: {source}")]
    ReadCredential {
        #[source]
        source: Box<dyn Error + Send + Sync>,
    },
    #[error("credential event decode failed: {source}")]
    DecodeEvent {
        #[source]
        source: CredentialEventPayloadError,
    },
    #[error("credential destroyed event is invalid: {source}")]
    DecodeDestroyed {
        #[source]
        source: CredentialProtoDecodeError,
    },
    #[error("credential stream replay failed: {source}")]
    ReplayCredential {
        #[source]
        source: CredentialEvolveError,
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
        source: IncompatibleCredentialRecoveryCheckpointBucketError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error(
    "credential recovery checkpoint bucket is incompatible: expected history {expected_history}, got {actual_history}; expected max age {expected_max_age:?}, got {actual_max_age:?}"
)]
pub(crate) struct IncompatibleCredentialRecoveryCheckpointBucketError {
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
            source: IncompatibleCredentialRecoveryCheckpointBucketError {
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
mod tests;
