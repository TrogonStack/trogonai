# Decider Platform

The decider platform is this workspace's event-sourcing runtime: a `Decider` trait for
domain decision logic, a native execution boundary that replays and appends JetStream
streams, a WASM host that runs the exact same decision logic compiled to a component, and
shared testing/observability tooling so both paths stay provably in sync. This is the guide
to read first; it links out to the ADRs and per-crate READMEs for anything it only
summarizes.

## The `decide`/`evolve`/`initial_state` cycle

Every decider is a plain Rust type implementing `trogon_decider::Decider`
(`crates/trogon-decider/src/lib.rs`):

```rust
pub trait Decider: Sized {
    type StreamId: ?Sized;
    type State;
    type Event;
    type DecideError: std::error::Error;
    type EvolveError: std::error::Error;

    const WRITE_PRECONDITION: Option<WritePrecondition> = None;

    fn stream_id(&self) -> &Self::StreamId;
    fn initial_state() -> Self::State;
    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError>;
    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError>;
}
```

`initial_state` seeds state with no events applied. `evolve` folds one stored event into
state and is the only place state changes. `decide` evaluates a command against the current
state and returns a `Decision`, never mutating anything itself. When a rejection crosses the
WASM boundary, the bridge surfaces it with the constant `"rejected"` code on the WIT
`domain-error` and carries the typed error's causal chain in its `details` pairs.

`WRITE_PRECONDITION` (`crates/trogon-decider/src/write_precondition.rs`) is the optional
concurrency guard applied when the decided events are appended:

```rust
pub enum WritePrecondition {
    Any,           // append regardless of current stream state
    StreamExists,  // append only if the stream already has events
    NoStream,      // append only if the stream is empty (first writer wins)
}
```

### `Decision::Events` vs `Decision::Act`

`decide` returns a `Decision<C>` (`crates/trogon-decider/src/decision.rs`):

```rust
#[non_exhaustive]
pub enum Decision<C> where C: Decider {
    Events(Events<C::Event>),
    Act(Act<C>),
}
```

`Decision::events(...)` (or the single-event `Decision::event(...)`) is the common case: a
flat, already-final batch of events. `Decision::act()` starts an `ActBuilder<C>` instead, for
decisions that need to observe the state produced by an earlier step before deciding the
next one:

```rust
Decision::act()
    .execute(|state, command| { /* returns Decision<C> or Result<Decision<C>, C::DecideError> */ })
    .execute(|state, command| { /* sees the state the previous step's events would produce */ })
```

Each `.execute` call returns a new typestate `ActChain<C, S>` (`S: Steps<C>`, built from
`First`/`Then` link types); these link types and `ActChain`/`ActRun`/`Steps` are
`#[doc(hidden)]` implementation detail. Converting the finished chain into a
`Result<Decision<C>, C::DecideError>` boxes the whole plan exactly once into an `Act<C>`, so
`Decision` can hold either form as one uniform type. `evaluate_decision`
(`crates/trogon-decider/src/decision.rs`, `#[doc(hidden)]`) is the shared entry point both the
native runtime and the WASM guest bridge call: `C::decide` then, for an `Act`, runs each step
in turn, folding that step's events via `C::evolve` before handing the resulting state to the
next step. This is why `Decision::Act` behaves identically on both paths: neither path
reimplements the stepping logic.

## The native path: `CommandExecution`

`trogon-decider-runtime`'s `CommandExecution<'a, E, C, S, G>`
(`crates/trogon-decider-runtime/src/execution.rs`) is the runtime boundary that applies one
command to one stream: read, replay, decide, append. Build one with `CommandExecution::new`
and configure it with builder methods before calling `execute`:

```rust
CommandExecution::new(&event_store, &command)
    .with_write_precondition(precondition)   // ignored if C::WRITE_PRECONDITION is set
    .with_headers(headers)
    .with_event_id_generator(generator)
    .with_snapshot(snapshot_store)           // moves to the snapshot-enabled type state
    .with_task_runtime(scheduler)
    .with_snapshot_failure_policy(policy)
    .execute()
    .await
```

There are two `execute` methods, selected by type state (whether `.with_snapshot(...)` was
called):

- **Without snapshots**: if the decider declares `WRITE_PRECONDITION = Some(NoStream)`, the
  stream isn't read at all and the command is decided straight against `C::initial_state()`.
  Otherwise the stream is read from the beginning (`ReadFrom::Beginning`), folded through
  `evolve`, decided, and appended.
- **With snapshots**: a snapshot is read first. On a snapshot the configured
  `SnapshotFailurePolicy` cannot trust, the policy decides `Fail` or `DiscardAndReplay`; a
  discarded snapshot falls back to a full replay from the beginning. Otherwise only the
  stream events *after* the snapshot's position are read (`ReadFrom::after(position)`). A
  snapshot whose position is ahead of the stream's current position is also routed through
  the failure policy. After decide and append succeed, a snapshot write is scheduled: always,
  if a bad snapshot was just discarded and overwritten with a trustworthy one; otherwise only
  when the configured `SnapshotPolicy::decide_snapshot` returns `SnapshotDecision::Take`.

`CommandError<DecideError, EvolveError, ReadSnapshotError, ReadStreamError, AppendStreamError,
EventTypeError, PayloadEncodeError, DecodeError>` normalizes failures by phase
(`Decide`, `Evolve`, `ReadSnapshot`, `ReadStream`, `Append`, `EventType`, `EventEncode`,
`DecodeEvent`, `SnapshotAheadOfStream`, `ReadAfterOverflow`) while preserving each phase's
concrete source error type.

### Snapshot policy and failure recovery

`SnapshotPolicy<C>` decides whether to take a snapshot after a successful append
(`decide_snapshot(context: DecideSnapshot<'_, C>) -> SnapshotDecision`). Two implementations
ship: `NoSnapshot` (never) and `FrequencySnapshot` (takes one once at least `frequency`
events have been read or appended since the last snapshot). `CommandSnapshotPolicy: Decider`
lets a decider declare its own default policy via an associated `SNAPSHOT_POLICY` const, so
callers build a configured `Snapshots` without repeating the policy at every call site.

`SnapshotFailurePolicy<C, ReadSnapshotError>` decides how to react to a snapshot execution
cannot trust: `FailOnSnapshotFailure` (the default, fails the command) or
`DiscardAndReplaySnapshotFailure` (discards the bad snapshot and replays from the beginning
of the stream instead of failing).

Scheduled snapshot writes run through a `SnapshotTaskScheduler` (`fn schedule`, plus a
`drain` future that defaults to resolving immediately). `TokioSnapshotTaskScheduler` is
fire-and-forget: its `drain` never actually waits. `DrainableSnapshotTaskScheduler` tracks
in-flight writes so a host can `drain().await` outstanding snapshot writes before shutdown.

### Headers

Event headers are operational metadata (correlation, tenancy, causation, transport
routing), distinct from event payloads, which are the canonical domain facts. The runtime
does not derive required headers from commands or events through a generic callback;
callers set headers explicitly via `CommandExecution::with_headers`. See
[Event Metadata](./event-metadata.md) for the full policy, and
[ADR#0013](../adr/0013-origin-stream-sequence-header.md) for the one reserved header name
this repository defines today, `Trogon-Origin-Stream-Sequence`, used only for provenance
when an event is republished into a different stream from the one it originally lived in.
It is never used for checkpoints, positions, or concurrency; those always use the stream's
current sequence.

## JetStream storage layer (`trogon-decider-nats`)

`trogon-decider-nats` implements the native runtime's storage traits (stream read/append,
snapshot read/write) against NATS JetStream. Per [ADR#0002](../adr/0002-rust-crate-boundaries.md),
this crate is the adapter layer: it knows how to store and replay a stream, not how to
`decide`/`evolve`.

### One physical stream, subject per logical stream

Every decider's events live in **one physical JetStream stream**. A caller-supplied
`StreamSubjectResolver<StreamId>` maps a domain stream identifier to the subject that stores
it (`resolve_subject_state`, returning a `SubjectState { subject, current_position }`), so
each logical stream gets its own subject within that one physical stream:

```text
physical JetStream stream "orders-events"
 ├─ subject "orders.events.order-1"   (logical stream "order-1")
 ├─ subject "orders.events.order-2"   (logical stream "order-2")
 └─ subject "orders.events.order-3"   (logical stream "order-3")
```

`ensure_stream`/`ensure_bucket` (`provision.rs`) idempotently create-or-open the physical
stream and the KV bucket used for snapshots: a create that fails because the resource
already exists resolves via a get instead of surfacing an error, so concurrent callers racing
to provision the same resource both succeed.

### Atomic multi-event append

`append_stream` (`stream_store.rs`) publishes a batch of events to one subject atomically
using three headers on the underlying JetStream messages: `Nats-Batch-Id` and
`Nats-Batch-Sequence` on every message in the batch, `Nats-Batch-Commit` on the final
message, and (when appending is conditional) the expected-last-subject-sequence guard on the
first message. The final message's stream sequence becomes the returned `StreamPosition`.

### Ordered, subject-filtered replay

Replay (`stream_store/replay.rs`) uses an ordered JetStream consumer scoped with
`filter_subject` and `DeliverPolicy::ByStartSequence { start_sequence }`, reading only the
target logical stream's own messages in order rather than scanning every physical sequence.
The consumer is self-healing for most disconnects; residual failure kinds are retried by
recreating the consumer from the last successfully processed sequence, bounded by
`ReplayRetryPolicy`: 5 attempts, 100ms base delay, 5s max delay, exponential backoff between
attempts.

### KV-backed snapshots

Snapshot storage (`snapshot_store.rs`) is a set of free functions generic over a JetStream KV
bucket (`read_snapshot`, `write_snapshot`, `read_checkpoint`, `write_checkpoint`,
`maybe_advance_checkpoint`, `persist_snapshot_change`), not a dedicated struct.
`NatsSnapshotConfig` carries the checkpoint key name; `SnapshotChange<T>` is either an
`Upsert` or a `Delete` mutation applied atomically alongside the checkpoint update.

## Read-side primitives: `Projector` and `Processor`

`trogon-decider-nats` also ships two generic read-side primitives, so consumers of decider
streams don't hand-roll catch-up or redelivery logic:

- **`Projector<Checkpoint>`** drives catch-up for one subject-filtered projection: given a
  `ProjectionCheckpointStore` (load/save a `CheckpointSequence`) and a `ProjectionApply`
  (apply one event, return the checkpoint to record for it), `catch_up` replays events in
  stream order up to the tail observed when the call started, saving the checkpoint after
  each one. Use `Projector` when you're building a read model or side-effect projection that
  needs to resume from wherever it last left off.
- **`Processor<Handler>`** drives a durable pull consumer's message loop for one
  `MessageHandler` (`handle` returns a `HandlerVerdict`: `Ack`, `Retry`, or `Poison`).
  `RedeliveryPolicy` bounds retries (default: 5 max deliveries, 200ms base delay, 30s max
  delay, exponential backoff) before a message is poisoned and `on_poison` is called with a
  `PoisonReason` (`Verdict`, `RedeliveryExhausted`, `HandlerError`, or `Panic`). Use
  `Processor` when you need at-least-once, ack/nak/term message processing against a durable
  consumer, not a resumable full-stream projection.

`CheckpointSequence` wraps the underlying `u64` stream sequence; `CheckpointSequence::NONE`
(value `0`) means no event has been applied yet, distinct from having processed sequence 1.
`next_from_sequence()` returns the sequence a catch-up should resume from (the checkpoint's
sequence plus one).

## The WASM path

The same `Decider` logic can be compiled to a WASM component and executed by
`trogon-decider-wasm-runtime` instead of running natively. Both paths share the same
`decide`/`evolve` semantics (via `evaluate_decision`) and the same event codec, so a decider
behaves identically whether it runs natively or as a guest.

### `export_decider!` and the WIT contract

`export_decider!` (`trogon-decider-guest-macros`) generates the component glue for one or
more commands that share a `Decider::State` and `Decider::Event`:

```rust
export_decider!(
    CreateSchedule {
        type_url = CREATE_SCHEDULE_TYPE_URL,
        proto = v1::CreateSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
        write_precondition = no_stream,
    },
);
```

It expands to a hidden module containing `wit_bindgen::generate!` bindings, a `Component`
unit struct implementing the WIT `Guest` trait (`descriptor()`, `stream_id(command)`), and a
`Session` struct (`RefCell<State>`) implementing `GuestSession`: `new` loads a snapshot or
builds initial state, `evolve` folds events and mutates the `RefCell`, `decide` only borrows
state (it does not fold its own output back in), and `snapshot` encodes current state. The
macro resolves the WIT contract via `trogon_decider_wit::WIT_DIR`, an absolute path baked
into `trogon-decider-wit` at that crate's own compile time, so a component crate can live at
any directory depth relative to it.

The contract itself lives in `trogon-decider-wit`'s `wit/world.wit` (package
`trogon:decider@0.2.0`, world `decider`, interface `handler`): `descriptor()` returns a
`module-descriptor { name, version, commands }`; `stream-id(command)` resolves a
`command-envelope` to its target stream; the `session` resource exposes
`evolve(events)`, `decide(command) -> result<list<any-envelope>, decide-error>`, and
`snapshot() -> option<list<u8>>`. `domain-error { code, message, details }` carries a stable
code, a human-readable message, and the error's source chain as ordered key/value pairs, so
detail that a single `Display` line would otherwise erase survives the WIT boundary.
`trogon-decider-wit` compiles this contract for both sides: `guest` (feature `guest`, via
`wit_bindgen::generate!`, for `wasm32` components) and `host` (feature `host`, via
`wasmtime::component::bindgen!`, for non-`wasm32` runtimes), re-exporting the same
WIT-derived types from both.

### Engine budgets and deadlines

`WasmDeciderEngine`/`WasmEngineConfig` (`trogon-decider-wasm-runtime/src/engine.rs`) configure
a shared wasmtime `Engine` with fuel metering, epoch-based interruption, the pooling
allocator, and per-store resource limits, built once per process and shared across every
loaded module:

| Budget | Default | Purpose |
| --- | --- | --- |
| `fuel_per_call` | 10,000,000 | Instructions a single `decide`/`evolve`/`snapshot` call may execute before trapping `OutOfFuel`. |
| `max_memory_bytes` | 64 MiB | Ceiling on a store's linear-memory growth. |
| `max_table_elements` | 8,192 | Ceiling on any table a session's store may create. |
| `max_instances_per_session` | 8 | Ceiling on core module instantiations per session's store. |
| `max_tables_per_session` | 6 | Ceiling on tables per session's store. |
| `max_memories_per_session` | 4 | Ceiling on memories per session's store. |
| `max_concurrent_sessions` | 256 | Pooling-allocator warm-slot ceiling. |
| `epoch_tick_interval` | 50ms | Cadence of the engine's background epoch ticker. |
| `epoch_ticks_per_call` | 40 | Ticks a call may run before being interrupted, i.e. a 2 second wall-clock budget per call at the default cadence. |

Fuel bounds instructions, not time; the epoch ticker is the wall-clock backstop for a guest
that is still executing but too slowly. A `WasmDeciderModule` is loaded once (`load`), which
compiles the component, structurally enforces zero imports by instantiating it against an
empty `wasmtime::component::Linker` (the same requirement `trogon-decider-sim`'s
`assert_zero_imports` checks for, via `wasm-tools` instead), caches the resulting `DeciderPre`
for cheap per-command instantiation, and probes the guest's `descriptor()` export once.

### Execution ordering: decide, fold, then snapshot

`WasmCommandExecution` mirrors the native `CommandExecution` boundary against the WIT
`session` resource. Guest calls run via `spawn_guest`, which wraps
`tokio::task::spawn_blocking` so a guest's fuel- and epoch-bounded call never occupies the
async executor for its duration.

The guest's generated `decide` export only reads session state; it never folds its own
newly decided events back into it (`evolve` mutates the session's `RefCell<State>`, `decide`
only borrows it). A snapshot taken immediately after `decide` would therefore silently drop
that command's own events. The snapshot-enabled execution path calls `decide`, then
`fold_decided_events` (which replays those events into the session via `evolve`), then
`take_snapshot`, in that order, so a session resumed from that snapshot ends up in the
same state a full replay would produce. The no-snapshot path skips the fold entirely: no
snapshot will observe that session again, so folding would only burn guest fuel for nothing.

`WasmCommandError` distinguishes a guest call that trapped for any other reason (`Trap`) from
one that exceeded its wall-clock epoch deadline (`DeadlineExceeded`), classified by
downcasting the trap to `wasmtime::Trap::Interrupt`.

### Snapshot failure recovery mirrors the native path

`WasmCommandExecution::with_snapshot_failure_policy` mirrors native's
`SnapshotFailurePolicy`: a snapshot the host cannot trust, either because reading it failed or
because it claims a position ahead of the stream, is routed through a
`WasmSnapshotFailurePolicy<ReadSnapshotError>` before either failure is allowed to fail the
command. `FailOnSnapshotFailure` (the default) keeps today's behavior;
`DiscardAndReplaySnapshotFailure` discards the untrusted snapshot and replays the stream from
the beginning instead, exactly as `CommandExecution` does natively. Both policy types are
reused directly from `trogon-decider-runtime`, since neither carries any decider-specific
state.

The WASM boundary has no typed `Decider`, so the policy's input is a `WasmSnapshotFailureContext`
carrying whatever identity the execution actually has in hand at that point instead of a
command reference: the module's name and version, the command's wire type URL, the resolved
stream id, and the failure itself (reused as native's `SnapshotFailure`).

Native forces the post-append snapshot write unconditionally when a bad snapshot was just
discarded, so a `SnapshotPolicy` that would otherwise skip cannot let the bad snapshot linger.
The WASM path needs no equivalent branch: it has no host-side `SnapshotPolicy` to skip with in
the first place, since a snapshot is written whenever the guest's `snapshot()` export returns
one at all.

### Registry rollout: swapping modules without a restart

`DeciderRegistry` routes command types to the `WasmDeciderModule` that declared them, built
once via `DeciderRegistryBuilder` (which rejects command-type collisions across modules).
`DeciderRegistryHandle` wraps a `DeciderRegistry` behind a `RwLock<Arc<DeciderRegistry>>` so
it can be updated while command executions are in flight:

```text
                 ┌──────────────────────────┐
   readers ────► │ RwLock<Arc<DeciderRegistry>> │ ◄──── activate(module) / retire(command_type)
                 └──────────────────────────┘
```

`activate(module)` routes every command type the module declares to it, replacing whatever
module previously owned each route; `retire(command_type)` removes one route. Both compute
the entire next routing table first and take the write lock only long enough to install it
with one assignment, so a module declaring more than one command type is activated or
retired as a unit: a reader never observes some of its command types on the new module
while a sibling command type from the same module is still on the old one. A caller resolves
a module once per command dispatch and keeps using that resolved `Arc`; a later `activate`
or `retire` never reaches back into an execution already running against a module it
resolved earlier.

`WasmSnapshotId` folds a module's identity into every snapshot id it produces:
`{module_name}@{module_version}/{stream_id}`. Routing a command type from module version `v1`
to `v2` via `activate` does not migrate or invalidate `v1`'s snapshots: `v2`'s snapshot id for
a given stream is simply a different string, so `v2`'s first command against a stream `v1`
had already snapshotted finds nothing under `v2`'s id and falls back to a full replay from
the beginning, the same fallback taken for a stream that was never snapshotted at all. That
replay reconstructs the same guest state a snapshot would have, just without resuming from a
saved position. No explicit migration or invalidation step is needed; the version bump
folded into the snapshot id is enough on its own.

## Testing story

Four layers, from unit-level to conformance-level:

- **`TestCase`** (`trogon-decider/src/testing.rs`) is a given/when/then typestate builder for
  unit-testing a native `Decider` directly: seed a `History<C::Event>`, issue a command,
  assert the resulting `Decision` via a `Then`-style expectation (event equality, a
  rejection, or a specific error code), and optionally assert the post-decision state.
- **`trogon-decider-sim`** is the native-vs-wasm parity harness. `SimHost`/`SimInstance`
  (`host.rs`) load a compiled component and run every guest call against the same resource
  budget `WasmDeciderEngine` applies in production, so a decider that would trap in
  production traps the same way in tests instead of running unbounded. `ScenarioIr`/
  `WireEnvelope` (`ir.rs`) express a decider-agnostic given/when/then scenario in wire form
  (type URL plus encoded payload) that both a wasm run (`ScenarioIr::run_wasm`) and a native
  run (`native.rs`'s `run_native`, reusing the exact `trogon-decider-guest-sdk` codec
  functions the guest bridge expands into) can execute. `SimScenario` (`scenario.rs`) is the
  fluent given/when/then builder used directly against a `SimInstance`, supporting multiple
  chained steps against one open session. `assert_parity` (`parity.rs`) runs one scenario
  through both runners and returns an error on the first divergence between their outcomes,
  catching codec, codegen, or WIT-level drift that each runner's own pass/fail would miss.
  `SimFixture` (`fixture.rs`, feature `test-support`) holds checked-in compiled `.wasm`
  fixtures for integration tests.
- **`decider-test` CLI** (`cli/trogon-decider-test`) runs a YAML conformance suite against a
  compiled component with no Rust required. A `Suite` declares scenarios (`Scenario`, each
  either a legacy single `when`/`then` pair or an ordered `steps: Vec<Step>` list run against
  one session) and the event type URLs the decider is declared to produce. Strict mode
  (default on, disabled with `--no-strict`) fails the run if any declared command or event
  type has zero coverage across every scenario's `given`/`when`/`then.events`; detection and
  reporting always run, `--no-strict` only downgrades the failure to a stderr warning.
- **`assert_zero_imports`** (`trogon-decider-sim/src/import_check.rs`) checks a compiled
  component declares no imports via `wasm-tools component wit`, the same zero-import
  requirement `WasmDeciderModule::load` enforces structurally at load time with an empty
  `Linker`.

## Observability

Per [ADR#0008](../adr/0008-opentelemetry-observability.md), the decider platform is
instrumented against `otel/semconv/registry/decider.yaml`'s `registry.trogonai.decider`
attribute group. All decider telemetry is `stability: development`.

Attributes: `command_type`, `module_name`, `module_version`, `write_precondition`
(`any`/`stream_exists`/`no_stream`/`at`), `decision_outcome`
(`decided`/`rejected`/`faulted`), `snapshot_outcome`
(`hit`/`miss`/`discarded_read_failure`/`discarded_ahead_of_stream`/`failed`),
`snapshot_write_success`, `guest_phase` (`instantiate`/`replay`/`decide`/`snapshot`), and
`trap_classification` (`deadline_exceeded`/`trap`). `stream_id` is defined once in
`scheduler.yaml` and reused here.

Spans: `decider.execute_command`, `decider.read_snapshot`, `decider.write_snapshot`,
`decider.append_stream`, `decider.replay_stream`, `decider.wasm.instantiate`,
`decider.wasm.replay`, `decider.wasm.decide`, `decider.wasm.snapshot`.

Metrics: `decider.replay.events` (counter), `decider.snapshot.reads` (counter, by
`snapshot_outcome`), `decider.snapshot.writes` (counter, by `snapshot_write_success`),
`decider.snapshot.kv_read_failures` / `decider.snapshot.kv_write_failures` (counters),
`decider.append.duration` (histogram), `decider.append.conflicts` (counter),
`decider.replay.duration` (histogram), `decider.replay.retries` (counter),
`decider.wasm.execution.duration` (histogram, by `guest_phase`),
`decider.wasm.fuel.consumed` (histogram, by `guest_phase`), `decider.wasm.traps` (counter, by
`trap_classification`).

## Related ADRs

Accepted and reflected in the platform as built:

- [ADR#0002: Rust Crate Boundaries](../adr/0002-rust-crate-boundaries.md): the
  domain/runtime/adapter split this document follows (`trogon-decider` /
  `trogon-decider-runtime` / `trogon-decider-nats`).
- [ADR#0008: OpenTelemetry Observability](../adr/0008-opentelemetry-observability.md): the
  vocabulary requirement behind the Observability section above.
- [ADR#0013: Origin Stream Sequence Header](../adr/0013-origin-stream-sequence-header.md):
  the `Trogon-Origin-Stream-Sequence` header described under Headers above.

Draft, **not yet implemented**; do not treat these as current behavior:

- [ADR#0026: Command Authorization Principal](../adr/0026-command-authorization-principal.md)
- [ADR#0027: Decider Multi-Tenancy Primitive](../adr/0027-decider-multi-tenancy-primitive.md)
- [ADR#0028: Decider Admission Control and Backpressure](../adr/0028-decider-admission-control-and-backpressure.md)
- [ADR#0029: Decider Retention and Truncation Watermark](../adr/0029-decider-retention-and-truncation-watermark.md)
