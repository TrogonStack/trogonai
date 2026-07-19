//! Command execution against a compiled WASM decider module.
//!
//! This mirrors [`trogon_decider_runtime::execution::CommandExecution`]
//! phase-by-phase, but the domain core lives inside a guest component instead
//! of a native `trogon_decider::Decider` implementation. Static evolve/decide
//! functions and a per-decider write precondition const cannot address session
//! state living behind `&mut wasmtime::Store` and a
//! `wasmtime::component::ResourceAny` handle, so this module reimplements the
//! same replay, decide, encode, and append flow directly against the WIT
//! `session` resource.
//!
//! # Snapshot and decide ordering
//!
//! The guest macro's generated `decide` export only reads session state; it
//! never folds its own newly decided events back into that state (verified
//! against `trogon-decider-guest-macros`: `evolve` mutates the session's
//! `RefCell<State>`, while `decide` only borrows it). A snapshot captured
//! immediately after `decide` would therefore silently drop this command's
//! own events. This execution evolves the session with the decided events
//! before calling `snapshot`, so a session resumed from that snapshot ends up
//! in the same state as a full replay from the beginning of the stream.
//!
//! # Guest calls run off the async executor
//!
//! Every guest export call is fuel-metered and wall-clock bounded (see
//! [`crate::WasmDeciderEngine::arm_guest_call`]), but a synchronous guest call
//! still occupies whatever thread calls it for up to that budget. Running it
//! inline on the async executor would stall every other task that executor
//! thread is responsible for. Both [`WithoutSnapshotStore`] and
//! [`WithSnapshotStore`] executions instead move each contiguous run of guest
//! calls onto a blocking thread pool via [`spawn_guest`], resuming the async
//! task only for real I/O (`StreamRead`, `StreamAppend`, `SnapshotRead`).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};
use thiserror::Error;
use trogon_decider_runtime::{
    AppendStreamRequest, DiscardAndReplaySnapshotFailure, Event, EventId, FailOnSnapshotFailure, Headers,
    ReadAfterOverflow, ReadFrom, ReadSnapshotRequest, ReadStreamRequest, Snapshot, SnapshotAheadOfStream,
    SnapshotFailure, SnapshotFailureDecision, SnapshotRead, SnapshotTaskScheduler, SnapshotWrite, StreamAppend,
    StreamPosition, StreamRead, StreamWritePrecondition, WriteSnapshotRequest,
};
use trogon_decider_wit::host::{self, AnyEnvelope, CommandEnvelope, DecideError};
use trogon_semconv::{attribute, metric, span};
use trogon_std::NowV7;
use wasmtime::Store;

use crate::{DomainErrorDetail, OpaqueSnapshotPayload, WasmDeciderEngine, WasmDeciderModule, WasmSnapshotId};

const METER_NAME: &str = "trogon-decider-wasm-runtime";

struct WasmExecutionMetrics {
    execution_duration: Histogram<f64>,
    fuel_consumed: Histogram<u64>,
    traps: Counter<u64>,
}

impl WasmExecutionMetrics {
    fn new() -> Self {
        let meter = global::meter(METER_NAME);
        Self {
            execution_duration: metric::build_decider_wasm_execution_duration(&meter),
            fuel_consumed: metric::build_decider_wasm_fuel_consumed(&meter),
            traps: metric::build_decider_wasm_traps(&meter),
        }
    }
}

static METRICS: OnceLock<WasmExecutionMetrics> = OnceLock::new();

fn metrics() -> &'static WasmExecutionMetrics {
    METRICS.get_or_init(WasmExecutionMetrics::new)
}

/// Module, version, and command type shared by every guest phase span and
/// metric recorded for one command execution.
#[derive(Debug, Clone)]
struct GuestPhaseContext {
    module_name: String,
    module_version: String,
    command_type: String,
}

impl GuestPhaseContext {
    fn new(module: &WasmDeciderModule, command: &CommandEnvelope) -> Self {
        Self {
            module_name: module.name().to_string(),
            module_version: module.version().to_string(),
            command_type: command.type_.clone(),
        }
    }
}

fn phase_attributes(context: &GuestPhaseContext, phase: attribute::GuestPhase) -> [KeyValue; 4] {
    [
        KeyValue::new(attribute::MODULE_NAME, context.module_name.clone()),
        KeyValue::new(attribute::MODULE_VERSION, context.module_version.clone()),
        KeyValue::new(attribute::COMMAND_TYPE, context.command_type.clone()),
        KeyValue::new(attribute::GUEST_PHASE, phase.as_str()),
    ]
}

fn record_phase_metrics(
    context: &GuestPhaseContext,
    phase: attribute::GuestPhase,
    duration: Duration,
    fuel_consumed: u64,
) {
    let attributes = phase_attributes(context, phase);
    metrics().execution_duration.record(duration.as_secs_f64(), &attributes);
    metrics().fuel_consumed.record(fuel_consumed, &attributes);
}

fn record_phase_trap(
    context: &GuestPhaseContext,
    phase: attribute::GuestPhase,
    classification: attribute::TrapClassification,
) {
    let mut attributes = phase_attributes(context, phase).to_vec();
    attributes.push(KeyValue::new(attribute::TRAP_CLASSIFICATION, classification.as_str()));
    metrics().traps.add(1, &attributes);
}

fn phase_fuel_consumed<T>(store: &Store<T>, fuel_budget: u64) -> u64 {
    store
        .get_fuel()
        .map_or(fuel_budget, |remaining| fuel_budget.saturating_sub(remaining))
}

fn trap_classification(error: &wasmtime::Error) -> attribute::TrapClassification {
    if is_epoch_deadline_exceeded(error) {
        attribute::TrapClassification::DeadlineExceeded
    } else {
        attribute::TrapClassification::Trap
    }
}

/// Result of a successful WASM command execution.
#[derive(Debug, Clone)]
pub struct WasmExecutionResult {
    /// The stream high-watermark after the command append completed.
    pub stream_position: StreamPosition,
    /// Domain event envelopes emitted by the guest decider and appended to the stream.
    pub events: Vec<AnyEnvelope>,
}

/// Error taxonomy for a WASM command execution attempt.
#[derive(Debug, Error)]
pub enum WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError> {
    /// The guest rejected the command as a domain failure.
    #[error("command rejected: {0}")]
    Rejected(#[source] DomainErrorDetail),
    /// The guest faulted while deciding the command.
    #[error("command faulted: {0}")]
    Faulted(#[source] DomainErrorDetail),
    /// The guest could not evolve session state from a replayed or decided event.
    #[error("command state evolution failed: {0}")]
    Evolve(#[source] DomainErrorDetail),
    /// The guest could not compute the command's stream id.
    #[error("command stream id resolution failed: {0}")]
    StreamId(#[source] DomainErrorDetail),
    /// A guest call trapped (fuel exhaustion, memory limit, or ABI failure).
    ///
    /// Distinct from [`Self::Faulted`]: a trap is a host-level wasmtime
    /// failure, not a domain error the guest chose to report. Distinct from
    /// [`Self::DeadlineExceeded`]: this variant is every other trap cause.
    #[error("guest call trapped")]
    Trap(#[source] wasmtime::Error),
    /// A guest call exceeded its wall-clock epoch deadline.
    ///
    /// Wasmtime's epoch-based interruption raised this instead of the guest
    /// exhausting its fuel budget first, meaning the guest was still running
    /// but too slowly. Distinguished from [`Self::Trap`] so callers can tell
    /// a hung guest apart from every other host-level trap.
    #[error("guest call exceeded its wall-clock deadline")]
    DeadlineExceeded(#[source] wasmtime::Error),
    /// The module could not be instantiated for this command.
    #[error("failed to instantiate wasm component")]
    Instantiate(#[source] wasmtime::Error),
    /// Snapshot loading failed before replaying stream history.
    #[error("command snapshot read failed: {0}")]
    ReadSnapshot(#[source] ReadSnapshotError),
    /// Stream history loading failed.
    #[error("command stream read failed: {0}")]
    ReadStream(#[source] ReadStreamError),
    /// Appending the decided events failed after the command was accepted.
    #[error("command stream append failed: {0}")]
    Append(#[source] AppendStreamError),
    /// The loaded snapshot claims a position newer than the stream can prove.
    #[error("{0}")]
    SnapshotAheadOfStream(SnapshotAheadOfStream),
    /// The snapshot's recorded position cannot be advanced (u64 overflow).
    #[error("{0}")]
    ReadAfterOverflow(#[source] ReadAfterOverflow),
    /// The blocking task running guest calls panicked or was cancelled.
    #[error("guest execution task failed")]
    Blocking(#[source] tokio::task::JoinError),
}

/// Module identity, command type, and stream id a [`WasmSnapshotFailurePolicy`]
/// inspects before deciding how to react to a snapshot a [`WasmCommandExecution`]
/// cannot trust.
///
/// Mirrors [`trogon_decider_runtime::SnapshotFailureContext`], adapted for the
/// WASM boundary: there is no typed `Decider` to hand the policy, so this
/// carries the identity the wasm execution actually has in hand at this point
/// instead of a command reference.
#[derive(Debug)]
pub struct WasmSnapshotFailureContext<'a, ReadSnapshotError> {
    /// Name of the module executing the command that triggered this failure.
    pub module_name: &'a str,
    /// Version of the module executing the command that triggered this failure.
    pub module_version: &'a str,
    /// Wire type URL of the command that triggered this failure.
    pub command_type: &'a str,
    /// Stream id the command resolved before loading its snapshot.
    pub stream_id: &'a str,
    /// The failure the policy must decide how to handle.
    pub failure: SnapshotFailure<'a, ReadSnapshotError>,
}

/// Chooses how a [`WasmCommandExecution`] reacts to a snapshot it cannot trust.
///
/// Mirrors [`trogon_decider_runtime::SnapshotFailurePolicy`] for the WASM
/// boundary, which has no typed `Decider` to bound the policy on.
/// [`FailOnSnapshotFailure`] keeps today's behavior of failing the command;
/// [`DiscardAndReplaySnapshotFailure`] discards the bad snapshot and replays
/// from the beginning of the stream. Both are reused directly from
/// `trogon_decider_runtime` since neither carries decider-specific state.
pub trait WasmSnapshotFailurePolicy<ReadSnapshotError> {
    /// Decides how the command execution should react to the given snapshot failure.
    fn decide_snapshot_failure(
        &self,
        context: WasmSnapshotFailureContext<'_, ReadSnapshotError>,
    ) -> SnapshotFailureDecision;
}

impl<ReadSnapshotError> WasmSnapshotFailurePolicy<ReadSnapshotError> for FailOnSnapshotFailure {
    fn decide_snapshot_failure(
        &self,
        _context: WasmSnapshotFailureContext<'_, ReadSnapshotError>,
    ) -> SnapshotFailureDecision {
        SnapshotFailureDecision::Fail
    }
}

impl<ReadSnapshotError> WasmSnapshotFailurePolicy<ReadSnapshotError> for DiscardAndReplaySnapshotFailure {
    fn decide_snapshot_failure(
        &self,
        _context: WasmSnapshotFailureContext<'_, ReadSnapshotError>,
    ) -> SnapshotFailureDecision {
        SnapshotFailureDecision::DiscardAndReplay
    }
}

/// Marker type used before a snapshot store is attached to an execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshotStore;

/// Snapshot store, best-effort task scheduler, and snapshot failure policy
/// attached to an execution.
pub struct WithSnapshotStore<'a, S, Sched, F = FailOnSnapshotFailure> {
    store: &'a S,
    task_scheduler: &'a Sched,
    failure_policy: F,
}

/// Builder for one command execution against a [`WasmDeciderModule`].
pub struct WasmCommandExecution<'a, E, Snapshots, G> {
    module: &'a WasmDeciderModule,
    event_store: &'a E,
    command: &'a CommandEnvelope,
    snapshots: Snapshots,
    write_precondition: Option<StreamWritePrecondition>,
    headers: Headers,
    event_id_generator: G,
}

impl<'a, E> WasmCommandExecution<'a, E, WithoutSnapshotStore, trogon_std::UuidV7Generator> {
    /// Starts building an execution for the given module, event store, and command.
    pub fn new(module: &'a WasmDeciderModule, event_store: &'a E, command: &'a CommandEnvelope) -> Self {
        Self {
            module,
            event_store,
            command,
            snapshots: WithoutSnapshotStore,
            write_precondition: None,
            headers: Headers::empty(),
            event_id_generator: trogon_std::UuidV7Generator,
        }
    }
}

impl<'a, E, G> WasmCommandExecution<'a, E, WithoutSnapshotStore, G> {
    /// Attaches a snapshot store and best-effort snapshot task scheduler.
    ///
    /// Defaults to [`FailOnSnapshotFailure`], which fails the command on an
    /// untrusted snapshot exactly as before this policy existed. Chain
    /// [`WasmCommandExecution::with_snapshot_failure_policy`] to recover
    /// instead.
    pub fn with_snapshot_store<S, Sched>(
        self,
        snapshot_store: &'a S,
        snapshot_task_scheduler: &'a Sched,
    ) -> WasmCommandExecution<'a, E, WithSnapshotStore<'a, S, Sched, FailOnSnapshotFailure>, G> {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: WithSnapshotStore {
                store: snapshot_store,
                task_scheduler: snapshot_task_scheduler,
                failure_policy: FailOnSnapshotFailure,
            },
            write_precondition: self.write_precondition,
            headers: self.headers,
            event_id_generator: self.event_id_generator,
        }
    }
}

impl<'a, E, S, Sched, F, G> WasmCommandExecution<'a, E, WithSnapshotStore<'a, S, Sched, F>, G> {
    /// Sets how this execution reacts to a snapshot it cannot trust.
    ///
    /// Defaults to [`FailOnSnapshotFailure`], which fails the command exactly
    /// as before this policy existed. Use [`DiscardAndReplaySnapshotFailure`]
    /// or a custom [`WasmSnapshotFailurePolicy`] to recover instead.
    pub fn with_snapshot_failure_policy<NextF>(
        self,
        failure_policy: NextF,
    ) -> WasmCommandExecution<'a, E, WithSnapshotStore<'a, S, Sched, NextF>, G> {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: WithSnapshotStore {
                store: self.snapshots.store,
                task_scheduler: self.snapshots.task_scheduler,
                failure_policy,
            },
            write_precondition: self.write_precondition,
            headers: self.headers,
            event_id_generator: self.event_id_generator,
        }
    }
}

impl<'a, E, Snapshots, G> WasmCommandExecution<'a, E, Snapshots, G> {
    /// Overrides the stream write precondition used when the module descriptor
    /// does not pin one for this command type.
    pub fn with_write_precondition<W>(mut self, write_precondition: W) -> Self
    where
        W: Into<Option<StreamWritePrecondition>>,
    {
        self.write_precondition = write_precondition.into();
        self
    }

    /// Attaches metadata headers propagated onto every appended event.
    pub fn with_headers(mut self, headers: Headers) -> Self {
        self.headers = headers;
        self
    }

    /// Overrides the event id generator (defaults to [`trogon_std::UuidV7Generator`]).
    pub fn with_event_id_generator<NextG>(
        self,
        event_id_generator: NextG,
    ) -> WasmCommandExecution<'a, E, Snapshots, NextG>
    where
        NextG: NowV7,
    {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: self.snapshots,
            write_precondition: self.write_precondition,
            headers: self.headers,
            event_id_generator,
        }
    }
}

/// Prior stream events replayed as guest envelopes, the stream's current
/// position, and the raw snapshot bytes passed to the guest session
/// constructor.
struct ReplayContext {
    replayed_envelopes: Vec<AnyEnvelope>,
    current_position: Option<StreamPosition>,
    snapshot_bytes: Option<Vec<u8>>,
}

impl<E, G> WasmCommandExecution<'_, E, WithoutSnapshotStore, G>
where
    E: StreamRead<str> + StreamAppend<str>,
    G: NowV7,
{
    /// Runs the command against a fresh guest session and appends its decided events.
    pub async fn execute(
        self,
    ) -> Result<
        WasmExecutionResult,
        WasmCommandError<std::convert::Infallible, <E as StreamRead<str>>::Error, <E as StreamAppend<str>>::Error>,
    > {
        let engine = self.module.engine().clone();
        let decider_pre = self.module.decider_pre().clone();
        let command = self.command.clone();
        let phase_context = GuestPhaseContext::new(self.module, self.command);
        let (mut store, bindings, stream_id) = spawn_guest(move || {
            let mut store = engine.new_store();
            let bindings = instantiate(&mut store, &decider_pre, &engine, &phase_context)?;
            let stream_id = call_stream_id(&mut store, &bindings, &engine, &command)?;
            Ok((store, bindings, stream_id))
        })
        .await?;

        let write_precondition = command_write_precondition(self.module, &self.command.type_);
        let (replayed_envelopes, current_position) =
            if has_no_stream_write_precondition(write_precondition, self.write_precondition) {
                (Vec::new(), None)
            } else {
                let stream_read = <E as StreamRead<str>>::read_stream(
                    self.event_store,
                    ReadStreamRequest {
                        stream_id: stream_id.as_str(),
                        from: ReadFrom::Beginning,
                    },
                )
                .await
                .map_err(WasmCommandError::ReadStream)?;
                let current_position = stream_read.current_position;
                (to_any_envelopes(stream_read.events), current_position)
            };

        let engine = self.module.engine().clone();
        let command = self.command.clone();
        let phase_context = GuestPhaseContext::new(self.module, self.command);
        let decided_envelopes = spawn_guest(move || {
            let session = create_session(&mut store, &bindings, &engine, None, &phase_context)?;
            replay_events(
                &mut store,
                &bindings,
                &engine,
                session,
                &replayed_envelopes,
                &phase_context,
            )?;
            let decided_envelopes = decide(&mut store, &bindings, &engine, session, &command, &phase_context)?;
            // No snapshot observes this session, so the decided events are not
            // folded back into it; folding here would only burn guest fuel.
            host::drop_session(&bindings, &mut store, session)
                .map_err(|error| map_trap(error, WasmCommandError::Trap))?;
            Ok(decided_envelopes)
        })
        .await?;

        let precondition = resolve_write_precondition(write_precondition, self.write_precondition, current_position);
        let events = encode_events(decided_envelopes.clone(), &self.headers, &self.event_id_generator);
        let append_response = <E as StreamAppend<str>>::append_stream(
            self.event_store,
            AppendStreamRequest {
                stream_id: stream_id.as_str(),
                stream_write_precondition: precondition,
                events,
            },
        )
        .await
        .map_err(WasmCommandError::Append)?;

        Ok(WasmExecutionResult {
            stream_position: append_response.stream_position,
            events: decided_envelopes,
        })
    }
}

impl<E, S, Sched, F, G> WasmCommandExecution<'_, E, WithSnapshotStore<'_, S, Sched, F>, G>
where
    E: StreamRead<str> + StreamAppend<str>,
    S: SnapshotRead<OpaqueSnapshotPayload, str>
        + SnapshotWrite<OpaqueSnapshotPayload, str>
        + Clone
        + Send
        + Sync
        + 'static,
    Sched: SnapshotTaskScheduler,
    F: WasmSnapshotFailurePolicy<<S as SnapshotRead<OpaqueSnapshotPayload, str>>::Error>,
    G: NowV7,
{
    /// Runs the command against a fresh guest session, appends its decided
    /// events, and best-effort schedules a snapshot write when the guest
    /// returns one.
    #[allow(clippy::type_complexity)]
    pub async fn execute(
        self,
    ) -> Result<
        WasmExecutionResult,
        WasmCommandError<
            <S as SnapshotRead<OpaqueSnapshotPayload, str>>::Error,
            <E as StreamRead<str>>::Error,
            <E as StreamAppend<str>>::Error,
        >,
    > {
        let engine = self.module.engine().clone();
        let decider_pre = self.module.decider_pre().clone();
        let command = self.command.clone();
        let phase_context = GuestPhaseContext::new(self.module, self.command);
        let (mut store, bindings, stream_id) = spawn_guest(move || {
            let mut store = engine.new_store();
            let bindings = instantiate(&mut store, &decider_pre, &engine, &phase_context)?;
            let stream_id = call_stream_id(&mut store, &bindings, &engine, &command)?;
            Ok((store, bindings, stream_id))
        })
        .await?;

        let write_precondition = command_write_precondition(self.module, &self.command.type_);
        let ReplayContext {
            replayed_envelopes,
            current_position,
            snapshot_bytes,
        } = if has_no_stream_write_precondition(write_precondition, self.write_precondition) {
            ReplayContext {
                replayed_envelopes: Vec::new(),
                current_position: None,
                snapshot_bytes: None,
            }
        } else {
            self.load_replay_context(&stream_id).await?
        };

        let engine = self.module.engine().clone();
        let command = self.command.clone();
        let phase_context = GuestPhaseContext::new(self.module, self.command);
        let (decided_envelopes, new_snapshot_bytes) = spawn_guest(move || {
            let session = create_session(
                &mut store,
                &bindings,
                &engine,
                snapshot_bytes.as_deref(),
                &phase_context,
            )?;
            replay_events(
                &mut store,
                &bindings,
                &engine,
                session,
                &replayed_envelopes,
                &phase_context,
            )?;
            let decided_envelopes = decide(&mut store, &bindings, &engine, session, &command, &phase_context)?;
            fold_decided_events(
                &mut store,
                &bindings,
                &engine,
                session,
                &decided_envelopes,
                &phase_context,
            )?;
            let new_snapshot_bytes = take_snapshot(&mut store, &bindings, &engine, session, &phase_context)?;
            host::drop_session(&bindings, &mut store, session)
                .map_err(|error| map_trap(error, WasmCommandError::Trap))?;
            Ok((decided_envelopes, new_snapshot_bytes))
        })
        .await?;

        let precondition = resolve_write_precondition(write_precondition, self.write_precondition, current_position);
        let events = encode_events(decided_envelopes.clone(), &self.headers, &self.event_id_generator);
        let append_response = <E as StreamAppend<str>>::append_stream(
            self.event_store,
            AppendStreamRequest {
                stream_id: stream_id.as_str(),
                stream_write_precondition: precondition,
                events,
            },
        )
        .await
        .map_err(WasmCommandError::Append)?;

        if let Some(bytes) = new_snapshot_bytes {
            let snapshot_id = WasmSnapshotId::new(self.module.name(), self.module.version(), &stream_id);
            schedule_snapshot_write(
                self.snapshots.task_scheduler,
                self.snapshots.store,
                snapshot_id,
                Snapshot::new(append_response.stream_position, OpaqueSnapshotPayload::new(bytes)),
            );
        }

        Ok(WasmExecutionResult {
            stream_position: append_response.stream_position,
            events: decided_envelopes,
        })
    }

    /// Builds the context a [`WasmSnapshotFailurePolicy`] inspects for one
    /// snapshot failure this execution encountered.
    fn snapshot_failure_context<'ctx, ReadSnapshotError>(
        &'ctx self,
        stream_id: &'ctx str,
        failure: SnapshotFailure<'ctx, ReadSnapshotError>,
    ) -> WasmSnapshotFailureContext<'ctx, ReadSnapshotError> {
        WasmSnapshotFailureContext {
            module_name: self.module.name().as_str(),
            module_version: self.module.version().as_str(),
            command_type: self.command.type_.as_str(),
            stream_id,
            failure,
        }
    }

    /// Loads the snapshot (if any) and the stream events replayed after it.
    ///
    /// A snapshot the configured [`WasmSnapshotFailurePolicy`] cannot trust,
    /// whether it failed to read or claims a position ahead of the stream, is
    /// routed through that policy. [`SnapshotFailureDecision::Fail`] returns
    /// the concrete [`WasmCommandError`] for the failure, matching this
    /// execution's behavior before the policy existed.
    /// [`SnapshotFailureDecision::DiscardAndReplay`] discards the untrusted
    /// snapshot and replays the stream from the beginning instead, exactly as
    /// [`trogon_decider_runtime::CommandExecution`] does natively.
    #[allow(clippy::type_complexity)]
    async fn load_replay_context(
        &self,
        stream_id: &str,
    ) -> Result<
        ReplayContext,
        WasmCommandError<
            <S as SnapshotRead<OpaqueSnapshotPayload, str>>::Error,
            <E as StreamRead<str>>::Error,
            <E as StreamAppend<str>>::Error,
        >,
    > {
        let snapshot_id = WasmSnapshotId::new(self.module.name(), self.module.version(), stream_id);
        let (snapshot_position, mut snapshot_bytes) =
            match <S as SnapshotRead<OpaqueSnapshotPayload, str>>::read_snapshot(
                self.snapshots.store,
                ReadSnapshotRequest {
                    snapshot_id: snapshot_id.as_str(),
                },
            )
            .await
            {
                Ok(response) => match response.snapshot {
                    Some(snapshot) => (Some(snapshot.position), Some(snapshot.payload.into_bytes())),
                    None => (None, None),
                },
                Err(error) => {
                    let context = self.snapshot_failure_context(stream_id, SnapshotFailure::ReadFailed(&error));
                    match self.snapshots.failure_policy.decide_snapshot_failure(context) {
                        SnapshotFailureDecision::Fail => return Err(WasmCommandError::ReadSnapshot(error)),
                        SnapshotFailureDecision::DiscardAndReplay => (None, None),
                    }
                }
            };

        let from = match snapshot_position {
            Some(position) => ReadFrom::after(position).map_err(WasmCommandError::ReadAfterOverflow)?,
            None => ReadFrom::Beginning,
        };
        let stream_read = <E as StreamRead<str>>::read_stream(self.event_store, ReadStreamRequest { stream_id, from })
            .await
            .map_err(WasmCommandError::ReadStream)?;
        let mut current_position = stream_read.current_position;
        let mut replayed_envelopes = to_any_envelopes(stream_read.events);

        if let Some(position) = snapshot_position
            && let Err(ahead_of_stream) = ensure_snapshot_not_ahead(position, current_position)
        {
            let context = self.snapshot_failure_context(stream_id, SnapshotFailure::AheadOfStream(ahead_of_stream));
            match self.snapshots.failure_policy.decide_snapshot_failure(context) {
                SnapshotFailureDecision::Fail => {
                    return Err(WasmCommandError::SnapshotAheadOfStream(ahead_of_stream));
                }
                SnapshotFailureDecision::DiscardAndReplay => {
                    snapshot_bytes = None;

                    let replay = <E as StreamRead<str>>::read_stream(
                        self.event_store,
                        ReadStreamRequest {
                            stream_id,
                            from: ReadFrom::Beginning,
                        },
                    )
                    .await
                    .map_err(WasmCommandError::ReadStream)?;
                    current_position = replay.current_position;
                    replayed_envelopes = to_any_envelopes(replay.events);
                }
            }
        }

        Ok(ReplayContext {
            replayed_envelopes,
            current_position,
            snapshot_bytes,
        })
    }
}

/// Runs a synchronous, guest-touching closure on a blocking thread pool so it
/// never occupies the async executor for the duration of its fuel and epoch
/// budget. See the module-level doc comment for why this is necessary.
async fn spawn_guest<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    task: impl FnOnce() -> Result<T, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>>
    + Send
    + 'static,
) -> Result<T, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>>
where
    T: Send + 'static,
    ReadSnapshotError: Send + 'static,
    ReadStreamError: Send + 'static,
    AppendStreamError: Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result,
        Err(join_error) => Err(WasmCommandError::Blocking(join_error)),
    }
}

fn is_epoch_deadline_exceeded(error: &wasmtime::Error) -> bool {
    matches!(error.downcast_ref::<wasmtime::Trap>(), Some(wasmtime::Trap::Interrupt))
}

/// Classifies a wasmtime call failure, surfacing an epoch-deadline interrupt
/// as [`WasmCommandError::DeadlineExceeded`] and delegating every other
/// failure to `fallback`.
fn map_trap<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    error: wasmtime::Error,
    fallback: impl FnOnce(wasmtime::Error) -> WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>,
) -> WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError> {
    if is_epoch_deadline_exceeded(&error) {
        WasmCommandError::DeadlineExceeded(error)
    } else {
        fallback(error)
    }
}

fn instantiate<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<crate::engine::GuestState>,
    decider_pre: &host::DeciderPre<crate::engine::GuestState>,
    engine: &WasmDeciderEngine,
    context: &GuestPhaseContext,
) -> Result<host::Decider, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_INSTANTIATE,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Instantiate.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = decider_pre.instantiate(&mut *store);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Instantiate, duration, fuel_consumed);
        result.map_err(|error| {
            let classification = trap_classification(&error);
            span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
            record_phase_trap(context, attribute::GuestPhase::Instantiate, classification);
            map_trap(error, WasmCommandError::Instantiate)
        })
    })
}

fn call_stream_id<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    command: &CommandEnvelope,
) -> Result<String, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    engine
        .arm_guest_call(
            store,
            engine.config().fuel_per_call(),
            engine.config().epoch_ticks_per_call(),
        )
        .map_err(WasmCommandError::Trap)?;
    host::call_stream_id(bindings, store, command)
        .map_err(|error| map_trap(error, WasmCommandError::Trap))?
        .map_err(|error| WasmCommandError::StreamId(error.into()))
}

fn create_session<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    snapshot: Option<&[u8]>,
    context: &GuestPhaseContext,
) -> Result<host::Session, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_INSTANTIATE,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Instantiate.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::create_session(bindings, store, snapshot);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Instantiate, duration, fuel_consumed);
        result.map_err(|error| {
            let classification = trap_classification(&error);
            span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
            record_phase_trap(context, attribute::GuestPhase::Instantiate, classification);
            map_trap(error, WasmCommandError::Trap)
        })
    })
}

fn replay_events<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    events: &[AnyEnvelope],
    context: &GuestPhaseContext,
) -> Result<(), WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    if events.is_empty() {
        return Ok(());
    }
    let span = tracing::info_span!(
        span::DECIDER_WASM_REPLAY,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Replay.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = replay_fuel(engine.config().fuel_per_call(), events.len());
        engine
            .arm_guest_call(
                store,
                fuel_budget,
                replay_epoch_ticks(engine.config().epoch_ticks_per_call(), events.len()),
            )
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::evolve(bindings, store, session, events);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Replay, duration, fuel_consumed);
        match result {
            Ok(inner) => inner.map_err(|error| WasmCommandError::Evolve(error.into())),
            Err(error) => {
                let classification = trap_classification(&error);
                span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
                record_phase_trap(context, attribute::GuestPhase::Replay, classification);
                Err(map_trap(error, WasmCommandError::Trap))
            }
        }
    })
}

/// Fuel budget for one batched replay `evolve` call.
///
/// The batch grows with stream length while `fuel_per_call` is fixed, so a
/// flat budget would trap legitimate commands on long streams. Scaling
/// linearly keeps fuel a per-event bound, which is the guarantee the sandbox
/// actually needs.
fn replay_fuel(fuel_per_call: u64, event_count: usize) -> u64 {
    u64::try_from(event_count)
        .map(|count| fuel_per_call.saturating_mul(count))
        .unwrap_or(u64::MAX)
}

/// Epoch tick budget for one batched replay `evolve` call, scaled the same
/// way as [`replay_fuel`] so a long replay gets a proportionally larger
/// wall-clock allowance instead of tripping the single-call deadline.
fn replay_epoch_ticks(epoch_ticks_per_call: u64, event_count: usize) -> u64 {
    u64::try_from(event_count)
        .map(|count| epoch_ticks_per_call.saturating_mul(count))
        .unwrap_or(u64::MAX)
}

fn decide<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    command: &CommandEnvelope,
    context: &GuestPhaseContext,
) -> Result<Vec<AnyEnvelope>, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_DECIDE,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Decide.as_str(),
        decision_outcome = tracing::field::Empty,
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::decide(bindings, store, session, command);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Decide, duration, fuel_consumed);
        match result {
            Ok(inner) => {
                let decision_outcome = match &inner {
                    Ok(_) => attribute::DecisionOutcome::Decided,
                    Err(DecideError::Rejected(_)) => attribute::DecisionOutcome::Rejected,
                    Err(DecideError::Faulted(_)) => attribute::DecisionOutcome::Faulted,
                };
                span.record(attribute::DECISION_OUTCOME, decision_outcome.as_str());
                inner.map_err(map_decide_error)
            }
            Err(error) => {
                let classification = trap_classification(&error);
                span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
                record_phase_trap(context, attribute::GuestPhase::Decide, classification);
                Err(map_trap(error, WasmCommandError::Trap))
            }
        }
    })
}

fn map_decide_error<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    error: DecideError,
) -> WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError> {
    match error {
        DecideError::Rejected(detail) => WasmCommandError::Rejected(detail.into()),
        DecideError::Faulted(detail) => WasmCommandError::Faulted(detail.into()),
    }
}

/// Folds the guest's own newly decided events back into session state before
/// a snapshot can observe them. See the module-level doc comment for why this
/// call is required.
fn fold_decided_events<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    decided_envelopes: &[AnyEnvelope],
    context: &GuestPhaseContext,
) -> Result<(), WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    replay_events(store, bindings, engine, session, decided_envelopes, context)
}

/// Captures a snapshot of the guest session's current state.
///
/// Must run after [`fold_decided_events`] folds this command's own decided
/// events back into the session; see the module-level doc comment.
fn take_snapshot<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &WasmDeciderEngine,
    session: host::Session,
    context: &GuestPhaseContext,
) -> Result<Option<Vec<u8>>, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    let span = tracing::info_span!(
        span::DECIDER_WASM_SNAPSHOT,
        module_name = %context.module_name,
        module_version = %context.module_version,
        command_type = %context.command_type,
        guest_phase = attribute::GuestPhase::Snapshot.as_str(),
        trap_classification = tracing::field::Empty,
    );
    span.in_scope(|| {
        let fuel_budget = engine.config().fuel_per_call();
        engine
            .arm_guest_call(store, fuel_budget, engine.config().epoch_ticks_per_call())
            .map_err(WasmCommandError::Trap)?;
        let start = Instant::now();
        let result = host::snapshot(bindings, store, session);
        let duration = start.elapsed();
        let fuel_consumed = phase_fuel_consumed(store, fuel_budget);
        record_phase_metrics(context, attribute::GuestPhase::Snapshot, duration, fuel_consumed);
        result.map_err(|error| {
            let classification = trap_classification(&error);
            span.record(attribute::TRAP_CLASSIFICATION, classification.as_str());
            record_phase_trap(context, attribute::GuestPhase::Snapshot, classification);
            map_trap(error, WasmCommandError::Trap)
        })
    })
}

fn to_any_envelopes(stream_events: Vec<trogon_decider_runtime::StreamEvent>) -> Vec<AnyEnvelope> {
    stream_events
        .into_iter()
        .map(|stream_event| AnyEnvelope {
            type_: stream_event.event.r#type,
            payload: stream_event.event.content,
        })
        .collect()
}

fn encode_events<G>(envelopes: Vec<AnyEnvelope>, headers: &Headers, event_id_generator: &G) -> Vec<Event>
where
    G: NowV7,
{
    envelopes
        .into_iter()
        .map(|envelope| Event {
            id: EventId::new(event_id_generator.now_v7()),
            r#type: envelope.type_,
            content: envelope.payload,
            headers: headers.clone(),
        })
        .collect()
}

fn command_write_precondition(module: &WasmDeciderModule, command_type: &str) -> Option<host::WritePrecondition> {
    module
        .commands()
        .iter()
        .find(|spec| spec.command_type().as_str() == command_type)
        .and_then(|spec| spec.write_precondition())
}

fn has_no_stream_write_precondition(
    spec_write_precondition: Option<host::WritePrecondition>,
    override_write_precondition: Option<StreamWritePrecondition>,
) -> bool {
    matches!(
        configured_write_precondition(spec_write_precondition, override_write_precondition),
        Some(StreamWritePrecondition::NoStream)
    )
}

fn resolve_write_precondition(
    spec_write_precondition: Option<host::WritePrecondition>,
    override_write_precondition: Option<StreamWritePrecondition>,
    current_position: Option<StreamPosition>,
) -> StreamWritePrecondition {
    configured_write_precondition(spec_write_precondition, override_write_precondition)
        .unwrap_or_else(|| current_position.into())
}

fn configured_write_precondition(
    spec_write_precondition: Option<host::WritePrecondition>,
    override_write_precondition: Option<StreamWritePrecondition>,
) -> Option<StreamWritePrecondition> {
    spec_write_precondition
        .map(to_stream_write_precondition)
        .or(override_write_precondition)
}

/// Maps the WIT write precondition onto the storage port's precondition type.
///
/// Both types are foreign to this crate, so a `From` impl would violate the
/// orphan rules; a plain function keeps the mapping local instead.
fn to_stream_write_precondition(value: host::WritePrecondition) -> StreamWritePrecondition {
    match value {
        host::WritePrecondition::Any => StreamWritePrecondition::Any,
        host::WritePrecondition::StreamExists => StreamWritePrecondition::StreamExists,
        host::WritePrecondition::NoStream => StreamWritePrecondition::NoStream,
    }
}

fn ensure_snapshot_not_ahead(
    snapshot_position: StreamPosition,
    current_position: Option<StreamPosition>,
) -> Result<(), SnapshotAheadOfStream> {
    match current_position {
        Some(stream_position) if snapshot_position <= stream_position => Ok(()),
        stream_position => Err(SnapshotAheadOfStream {
            snapshot_position,
            stream_position,
        }),
    }
}

fn schedule_snapshot_write<S, Sched>(
    task_scheduler: &Sched,
    snapshot_store: &S,
    snapshot_id: WasmSnapshotId,
    snapshot: Snapshot<OpaqueSnapshotPayload>,
) where
    S: SnapshotWrite<OpaqueSnapshotPayload, str> + Clone + Send + Sync + 'static,
    Sched: SnapshotTaskScheduler,
{
    let snapshot_store = snapshot_store.clone();
    let snapshot_id_for_log = snapshot_id.to_string();
    task_scheduler.schedule(async move {
        if let Err(source) = snapshot_store
            .write_snapshot(WriteSnapshotRequest {
                snapshot_id: snapshot_id.as_str(),
                snapshot,
            })
            .await
        {
            tracing::warn!(snapshot_id = %snapshot_id_for_log, error = %source, "failed to write wasm decider snapshot");
        }
    });
}

#[cfg(test)]
mod tests;
