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

use thiserror::Error;
use trogon_decider_runtime::{
    AppendStreamRequest, Event, EventId, Headers, ReadAfterOverflow, ReadFrom, ReadSnapshotRequest, ReadStreamRequest,
    Snapshot, SnapshotAheadOfStream, SnapshotRead, SnapshotTaskScheduler, SnapshotWrite, StreamAppend, StreamPosition,
    StreamRead, StreamWritePrecondition, WriteSnapshotRequest,
};
use trogon_decider_wit::host::{self, AnyEnvelope, CommandEnvelope, DecideError};
use trogon_std::NowV7;
use wasmtime::Store;

use crate::{DomainErrorDetail, OpaqueSnapshotPayload, WasmDeciderModule, WasmSnapshotId};

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
    /// failure, not a domain error the guest chose to report.
    #[error("guest call trapped")]
    Trap(#[source] wasmtime::Error),
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
}

/// Marker type used before a snapshot store is attached to an execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutSnapshotStore;

/// Snapshot store and best-effort task scheduler attached to an execution.
pub struct WithSnapshotStore<'a, S, Sched> {
    store: &'a S,
    task_scheduler: &'a Sched,
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
    pub fn with_snapshot_store<S, Sched>(
        self,
        snapshot_store: &'a S,
        snapshot_task_scheduler: &'a Sched,
    ) -> WasmCommandExecution<'a, E, WithSnapshotStore<'a, S, Sched>, G> {
        WasmCommandExecution {
            module: self.module,
            event_store: self.event_store,
            command: self.command,
            snapshots: WithSnapshotStore {
                store: snapshot_store,
                task_scheduler: snapshot_task_scheduler,
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
/// position, the snapshot position they resumed from (if any), and the raw
/// snapshot bytes passed to the guest session constructor.
type ReplayContext = (
    Vec<AnyEnvelope>,
    Option<StreamPosition>,
    Option<StreamPosition>,
    Option<Vec<u8>>,
);

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
        let engine = self.module.engine();
        let mut store = engine.new_store();
        let bindings = instantiate(&mut store, self.module)?;
        let stream_id = call_stream_id(&mut store, &bindings, engine, self.command)?;
        let write_precondition = command_write_precondition(self.module, &self.command.type_);

        let (replayed_envelopes, current_position) = if is_no_stream(write_precondition) {
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

        let session = create_session(&mut store, &bindings, engine, None)?;
        replay_events(&mut store, &bindings, engine, session, &replayed_envelopes)?;
        let decided_envelopes = decide(&mut store, &bindings, engine, session, self.command)?;
        // No snapshot observes this session, so the decided events are not
        // folded back into it; folding here would only burn guest fuel.
        host::drop_session(&bindings, &mut store, session).map_err(WasmCommandError::Trap)?;

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

impl<E, S, Sched, G> WasmCommandExecution<'_, E, WithSnapshotStore<'_, S, Sched>, G>
where
    E: StreamRead<str> + StreamAppend<str>,
    S: SnapshotRead<OpaqueSnapshotPayload, str>
        + SnapshotWrite<OpaqueSnapshotPayload, str>
        + Clone
        + Send
        + Sync
        + 'static,
    Sched: SnapshotTaskScheduler,
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
        let engine = self.module.engine();
        let mut store = engine.new_store();
        let bindings = instantiate(&mut store, self.module)?;
        let stream_id = call_stream_id(&mut store, &bindings, engine, self.command)?;
        let write_precondition = command_write_precondition(self.module, &self.command.type_);

        let (replayed_envelopes, current_position, _snapshot_position, snapshot_bytes) =
            if is_no_stream(write_precondition) {
                (Vec::new(), None, None, None)
            } else {
                self.load_replay_context(&stream_id).await?
            };

        let session = create_session(&mut store, &bindings, engine, snapshot_bytes.as_deref())?;
        replay_events(&mut store, &bindings, engine, session, &replayed_envelopes)?;
        let decided_envelopes = decide(&mut store, &bindings, engine, session, self.command)?;
        fold_decided_events(&mut store, &bindings, engine, session, &decided_envelopes)?;

        store
            .set_fuel(engine.config().fuel_per_call())
            .map_err(WasmCommandError::Trap)?;
        let new_snapshot_bytes = host::snapshot(&bindings, &mut store, session).map_err(WasmCommandError::Trap)?;
        host::drop_session(&bindings, &mut store, session).map_err(WasmCommandError::Trap)?;

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
        let response = <S as SnapshotRead<OpaqueSnapshotPayload, str>>::read_snapshot(
            self.snapshots.store,
            ReadSnapshotRequest {
                snapshot_id: snapshot_id.as_str(),
            },
        )
        .await
        .map_err(WasmCommandError::ReadSnapshot)?;
        let (snapshot_position, snapshot_bytes) = match response.snapshot {
            Some(snapshot) => (Some(snapshot.position), Some(snapshot.payload.into_bytes())),
            None => (None, None),
        };

        let from = match snapshot_position {
            Some(position) => ReadFrom::after(position).map_err(WasmCommandError::ReadAfterOverflow)?,
            None => ReadFrom::Beginning,
        };
        let stream_read = <E as StreamRead<str>>::read_stream(self.event_store, ReadStreamRequest { stream_id, from })
            .await
            .map_err(WasmCommandError::ReadStream)?;
        let current_position = stream_read.current_position;

        if let Some(snapshot_position) = snapshot_position {
            ensure_snapshot_not_ahead(snapshot_position, current_position)
                .map_err(WasmCommandError::SnapshotAheadOfStream)?;
        }

        Ok((
            to_any_envelopes(stream_read.events),
            current_position,
            snapshot_position,
            snapshot_bytes,
        ))
    }
}

fn instantiate<ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<crate::engine::GuestState>,
    module: &WasmDeciderModule,
) -> Result<host::Decider, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    store
        .set_fuel(module.engine().config().fuel_per_call())
        .map_err(WasmCommandError::Trap)?;
    module
        .decider_pre()
        .instantiate(store)
        .map_err(WasmCommandError::Instantiate)
}

fn call_stream_id<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &crate::WasmDeciderEngine,
    command: &CommandEnvelope,
) -> Result<String, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    store
        .set_fuel(engine.config().fuel_per_call())
        .map_err(WasmCommandError::Trap)?;
    host::call_stream_id(bindings, store, command)
        .map_err(WasmCommandError::Trap)?
        .map_err(|error| WasmCommandError::StreamId(error.into()))
}

fn create_session<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &crate::WasmDeciderEngine,
    snapshot: Option<&[u8]>,
) -> Result<host::Session, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    store
        .set_fuel(engine.config().fuel_per_call())
        .map_err(WasmCommandError::Trap)?;
    host::create_session(bindings, store, snapshot).map_err(WasmCommandError::Trap)
}

fn replay_events<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &crate::WasmDeciderEngine,
    session: host::Session,
    events: &[AnyEnvelope],
) -> Result<(), WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    if events.is_empty() {
        return Ok(());
    }
    store
        .set_fuel(engine.config().fuel_per_call())
        .map_err(WasmCommandError::Trap)?;
    host::evolve(bindings, store, session, events)
        .map_err(WasmCommandError::Trap)?
        .map_err(|error| WasmCommandError::Evolve(error.into()))
}

fn decide<T, ReadSnapshotError, ReadStreamError, AppendStreamError>(
    store: &mut Store<T>,
    bindings: &host::Decider,
    engine: &crate::WasmDeciderEngine,
    session: host::Session,
    command: &CommandEnvelope,
) -> Result<Vec<AnyEnvelope>, WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    store
        .set_fuel(engine.config().fuel_per_call())
        .map_err(WasmCommandError::Trap)?;
    host::decide(bindings, store, session, command)
        .map_err(WasmCommandError::Trap)?
        .map_err(map_decide_error)
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
    engine: &crate::WasmDeciderEngine,
    session: host::Session,
    decided_envelopes: &[AnyEnvelope],
) -> Result<(), WasmCommandError<ReadSnapshotError, ReadStreamError, AppendStreamError>> {
    replay_events(store, bindings, engine, session, decided_envelopes)
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

fn is_no_stream(write_precondition: Option<host::WritePrecondition>) -> bool {
    matches!(write_precondition, Some(host::WritePrecondition::NoStream))
}

fn resolve_write_precondition(
    spec_write_precondition: Option<host::WritePrecondition>,
    override_write_precondition: Option<StreamWritePrecondition>,
    current_position: Option<StreamPosition>,
) -> StreamWritePrecondition {
    spec_write_precondition
        .map(to_stream_write_precondition)
        .or(override_write_precondition)
        .unwrap_or_else(|| current_position.into())
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
