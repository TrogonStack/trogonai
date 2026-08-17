# Decider Platform TODO

Working backlog for `rsworkspace/crates/decider/*`, derived from a read of the eight crates,
[`docs/architecture/decider.md`](./docs/architecture/decider.md), ADRs 0026-0029, and the open
`decider` issues (#464-#470).

Organized **bottom-up by dependency layer**, so work lands in an order where nothing is built on a
contract that is about to change underneath it. Each item keeps a priority tag (`P0`-`P4`) so
urgency is not lost to the layering.

Legend: `[ ]` open, `[x]` done, `[~]` in progress.

## Layer map

Verified from each crate's `Cargo.toml`, not from prose:

```text
L0  trogon-decider (domain, no deps)      trogon-decider-wit (contract, no deps)
L1  trogon-decider-runtime -> domain      trogon-decider-guest-macros -> wit
L2  trogon-decider-nats -> runtime        trogon-decider-wasm-runtime -> runtime, wit
    trogon-decider-guest-sdk -> domain, guest-macros
L3  trogon-decider-sim -> domain, guest-sdk, wasm-runtime, wit
L4  cli/trogon-decider-test -> sim        wasm-components/* -> domain, guest-sdk
L5  (does not exist) host -> nats, wasm-runtime
```

Read bottom-up, the shape of the work is: **the two foundation layers are clean, the runtime layer
holds one keystone refactor, the adapter layer holds the real correctness bugs, and the top layer is
missing entirely.**

---

## L0 - `trogon-decider` (domain)

The `decide`/`evolve`/`Decision`/`Act` surface is sound - it pulls nothing but `thiserror`, compiles
for `wasm32`, and is the one part of the platform both execution paths share without duplication
(via `evaluate_decision`). One modelling defect, in `WritePrecondition`, where an `Option` is
overloaded to mean the opposite of what it reads as.

- [x] `P1` **Replace `Option<WritePrecondition>` with a total enum.** *(landed)*

      `WRITE_PRECONDITION: Option<WritePrecondition>` (`src/lib.rs:140`) overloads `None` to mean
      "no domain opinion, delegate", and the delegation resolves to the *strictest* guard in the
      system: `At(observed_position)` on a non-empty stream, `NoStream` on an empty one
      (`runtime/src/execution.rs:937`, `runtime/src/stream/append_stream.rs:43`). Meanwhile
      `Some(Any)` means genuinely unguarded. So `None` and `Any` read as synonyms and are
      opposites, and the trait doc at `src/lib.rs:139` states the wrong one - it calls `None`
      "no precondition", which is what `Any` is. Three of the four `Some` values are *less* safe
      than omitting the const entirely.

      **The two inputs answer different questions.** The current design treats the decider const and
      the caller's builder value as two contenders for one slot, resolved by precedence - which is
      why `None` had to mean "unspecified, fall through", and why the lowest-information source (a
      `const` that has never seen a stream) outranks a caller holding an observed position. They are
      not competing. The const answers *"what does this command's meaning require?"*; the builder
      answers *"does this request carry a client's expected revision?"*. Model them as two orthogonal
      inputs and the precedence chain disappears.

      Derived from the business scenarios:

      | # | Scenario | Needs | Who knows |
      | --- | --- | --- | --- |
      | 1 | Create-once (`CreateSchedule`) - two concurrent creates, one wins | `NoStream` | decider, at compile time |
      | 2 | Ordinary transition (`PauseSchedule`) - reject a decision made on stale state | `At(observed)` | runtime, having just read |
      | 3 | Commuting high-volume events (session messages, tool calls) - guarding is pure cost | `Any` | decider; "these commute" is a domain fact |
      | 4 | Must-already-exist, where `initial_state()` is itself valid | `StreamExists` | decider; see below |
      | 5 | Client-supplied expected revision (UI loaded rev 7, submits 2 min later) | `At(7)` | app layer, at runtime |
      | 6 | Backfill / admin repair | - | bypasses `CommandExecution`, writes via `append_stream` |
      | 7 | Command retry should be a no-op | - | dedup (`message_id` item at L2), not a precondition |

      Scenario 4 is why `StreamExists` earns its place rather than deferring to a domain error in
      `decide`: for a decider whose `initial_state()` is a meaningful valid state, folding zero
      events and folding a created-but-quiet aggregate produce the *same* state. `decide` cannot
      distinguish "never created" from "created, nothing happened yet" - that distinction exists only
      in the stream's emptiness, which `decide` never observes. Without `StreamExists` you would add
      an `exists: bool` to state purely to compensate.

      The resulting type - required const, no `Option`, no default:

      ```rust
      pub enum WritePrecondition {
          /// Append only if the stream is still exactly as replay observed it.
          StreamUnchanged,
          /// Append only if the stream is empty (first writer wins).
          NoStream,
          /// Append only if the stream already has events.
          StreamExists,
          /// Append regardless of current stream state.
          Any,
      }

      const WRITE_PRECONDITION: WritePrecondition;   // required; no default
      ```

      Resolution becomes one total match over both inputs, replacing a rule currently split between
      `execute_command_write_precondition` (`runtime/src/execution.rs:77`) and the
      `.unwrap_or_else(...)` tail at `:937`:

      ```rust
      match (declared, expected_revision) {
          // No client revision: the domain's requirement stands.
          (StreamUnchanged, None)    => observed.into(),
          (NoStream,        None)    => StreamWritePrecondition::NoStream,
          (StreamExists,    None)    => StreamWritePrecondition::StreamExists,
          (Any,             None)    => StreamWritePrecondition::Any,

          // Client supplied a revision: that is what the caller is asserting.
          // `At(p)` implies the stream exists, so it satisfies `StreamExists` too.
          (StreamUnchanged, Some(p)) => StreamWritePrecondition::At(p),
          (StreamExists,    Some(p)) => StreamWritePrecondition::At(p),
          (Any,             Some(p)) => StreamWritePrecondition::At(p),

          // Contradiction: "must not exist yet" and "must be at revision p".
          (NoStream,        Some(_)) => return Err(PreconditionConflict::CreateWithRevision),
      }
      ```

      **Why a client revision may override even `Any`.** A caller's `p` is always read *before* the
      host replays (it arrives on the request that triggers the replay), and stream positions are
      monotonic, so `p <= observed`. `At(p)` is therefore always equal to or stricter than what the
      declared variant would have produced: it can cause a rejection, never a wrongly-accepted
      write. Honoring it is safe for every variant except `NoStream`, where the two are jointly
      unsatisfiable rather than merely redundant. The alternative (letting `Any` discard the
      caller's `p`) would silently disarm a guard the caller believes is active, which is the same
      class of lie as today's `None`, pointed the other way.

      This safety argument rests on `p <= observed`. If a future path ever lets a revision be
      supplied *after* replay (a retry loop that re-reads, say), the argument breaks and the arm
      needs revisiting. Worth a debug assertion at the resolution site.

      Rename the builder to name its scenario: `with_expected_revision(StreamPosition)`, not
      `with_write_precondition(..)`. The old name invites "override whatever the decider said"; the
      new one says "this request carries a client's expected revision", which is all scenario 5
      needs. It also cannot express `Any`, so a caller weakening the domain's requirement stops being
      *representable* rather than needing to be detected and rejected.

      `StreamWritePrecondition` is unchanged - it is the resolved storage-level type and is already
      modeled correctly, `At(StreamPosition)` included.

      **`StreamUnchanged` is a new capability, not a rename.** Today a decider can only abstain or
      weaken; it cannot mandate optimistic concurrency.
      [ADR#0035](./docs/adr/0035-session-store-decider-aggregate.md) rejected a trait-level `At(N)`
      because a `const` cannot know `N`, concluding a decider "could only weaken, never strengthen".
      That is right about `At(N)` and wrong about the goal: `StreamUnchanged` names the intent and
      lets the runtime supply the position, so it is declarable at compile time and can only
      strengthen. The representation was the problem, not the capability. **Amend [ADR#0035](./docs/adr/0035-session-store-decider-aggregate.md)'s
      rejected-alternatives section when this lands.**

      Migration is small - almost nothing declares this today:
      - `scheduler-domain/src/commands/create_schedule.rs:44` - `Some(NoStream)` -> `NoStream`. The
        only production declaration in the workspace.
      - Two test deciders (`runtime/tests/execute_command_span.rs:32`,
        `runtime/tests/replay_events_metric.rs:30`) - `Some(Any)` -> `Any`.
      - Every other `Decider` impl gains one line, since the const is now required. That is the
        point: each decider's concurrency posture becomes greppable at its own definition. Anything
        relying on today's implicit `None` becomes `StreamUnchanged`, which is what it was already
        getting.
      - `runtime/src/execution.rs:80`, `:937`, and `From<WritePrecondition>` at `:1291` - absorbed
        into the match above.
      - `with_write_precondition` (`runtime/src/execution.rs:849`,
        `wasm-runtime/src/execution.rs:359`) -> `with_expected_revision`, narrowed from
        `StreamWritePrecondition` to `StreamPosition`. Every call site in the workspace is a test,
        so this costs nothing today - but do not delete the capability: the L5 host is exactly where
        client-supplied revisions arrive, and no `const` can express scenario 5.
      - `wit/world.wit:34` mirrors the variants and `command-spec.write-precondition` drops its
        `option<>` wrapper. **That is the ABI break - land it with the descriptor decision below,
        not as a separate bump.**
- [x] `P2` **Add a `weakened_write_precondition` dylint.** `dylints/trogon_lints` already registers
      17 lints (`src/lib.rs:52`); add one denying `WRITE_PRECONDITION = WritePrecondition::Any`
      without an `#[allow(..., reason = "...")]`. `Any` is the one variant that turns concurrency
      control off entirely (scenarios 3, 6, and 7), and those are the cases where the *why* matters
      and is invisible at the call site. The required const makes every decider's choice
      *visible*; the lint makes the one dangerous choice *argued*. Starts green: no production
      decider declares `Any` today.
- [ ] `P4` **Evaluate issue #468** - split the `Decider` trait into `StateModel` and `Command`. Filed
      as an *optional* refactor. Touching L0 ripples through every layer above, so this needs a
      concrete payoff before it is worth the blast radius. Decide yes/no; do not leave it drifting.

## L0 - `trogon-decider-wit` (contract)

Changing `wit/world.wit` (package `trogon:decider@0.3.0`) is a versioned contract bump that moves
guest and host in lockstep. **Anything here must be decided before the L2 work that would consume
it**, which is the main reason this file is ordered bottom-up.

- [x] `P1` **Decide whether snapshot cadence belongs in `module-descriptor`.** *(decided: yes;
      landed)* `command-spec` gained a required `snapshot-policy`, fed from a new
      `Decider::SNAPSHOT_CADENCE` const (`trogon-decider/src/snapshot_cadence.rs`).

      Decided on evidence rather than preference: native already declares cadence at the command via
      `CommandSnapshotPolicy::SNAPSHOT_POLICY`, so a host-side-only WASM policy would make the same
      decider snapshot at two different rates depending on which path ran it, defeating the parity
      guarantee `trogon-decider-sim` exists to enforce. The "untrusted guest imposes KV cost"
      objection is answered by `WasmCommandExecution::with_snapshot_cadence`, the host override that
      mirrors passing an explicit `Snapshots` policy on the native path.

      `SnapshotCadence` defaults to `Never` where `WritePrecondition` has no default at all: an
      omitted precondition can be wrong, an omitted cadence is only slower.
- [x] `P1` **Mirror the `write-precondition` variant change** *(landed in the same `0.3.0` bump)* from the L0 domain item above:
      `world.wit:34` gains `inherit` and `stream-unchanged`, and `command-spec.write-precondition`
      (`:48`) drops its `option<>` wrapper. Its doc comment carries the same defect as the Rust
      side - "or none for no constraint" describes `any`, not `none`.

      **Bundle these two items into a single `0.2.0` -> `0.3.0` bump.** They are the only two
      pending contract changes; shipping them separately costs two lockstep guest/host rollouts for
      one release's worth of value.
- [ ] `P4` **Issue #464** - typed cross-language codec and proto-to-WIT emitter. Contract-shaped
      work; sequence it here so it does not collide with the descriptor decision above.

---

## L1 - `trogon-decider-runtime`

The keystone layer. One refactor here removes the duplication that currently makes every policy cost
four implementations.

- [x] `P1` **Extract a shared execution skeleton.** Four near-identical `execute` bodies
      reimplemented read -> replay -> decide -> append -> snapshot, one per (native | wasm) x
      (snapshots | no snapshots). They are now two: one skeleton per crate, with the snapshot half
      behind a sealed trait.
      - `trogon-decider-runtime`: `ExecutionSnapshots<C>`, implemented by `WithoutSnapshots` and
        `Snapshots`. `load_replay_context` owns the snapshot read and the stream read (owning both
        is what lets the discard-and-replay policy re-read from the beginning); `store` persists
        what the execution ended at.
      - `trogon-decider-wasm-runtime`: `WasmExecutionSnapshots`, implemented by
        `WithoutSnapshotStore` and `WithSnapshotStore`, plus a `snapshot_cadence` hook that narrows
        the module's declared cadence to `Never` when there is no store, so the guest is not asked
        to fold and serialize bytes nobody will read.

      The bounds live on the implementations, not on the trait: snapshotting needs `C::State` to be
      `Clone + Send + SnapshotType`, and a decider that never snapshots still owes none of that, so
      both public contracts are unchanged.

      **Deviation from the wording below, deliberate.** The item asked to "parameterize over a
      'session' abstraction (native fold vs. guest resource handle)", i.e. one skeleton across both
      crates. That was not done, because the wasm path bundles create-session, replay, decide,
      fold-decided-events, take-snapshot and conclude-session into a single `spawn_guest` call:
      a shared session trait would have split those into separately awaited phases and cost a
      thread hop each, to unify two bodies that were already going to be one per crate. The
      follow-up items tagged *(lands in the skeleton)* are therefore a two-place change rather than
      a one-place change, which is the price paid for keeping the guest call unsplit.

      Fixed while refactoring: the wasm snapshot path called raw `read_stream` where the store-less
      path called the limit-bounded `read_stream_for_execution`, so a configured `ReplayLimit` was
      silently unbounded behind a snapshot store. The L2 parity bundle claimed the bounded read had
      landed; it had landed on one of the two wasm configurations. Both are bounded now, and
      `tests/schedules_execution.rs` pins the bound on both, plus on the discard-and-replay
      recovery read.

      > *Original wording:* Parameterize over a "session" abstraction (native fold vs. guest
      > resource handle). Every item below tagged *(lands in the skeleton)* becomes a one-place
      > change once this exists. **Do this first.** It is the reason the recommended order is L1
      > before L2, and it spans into L2's wasm-runtime, so land the runtime half and the wasm half
      > together.
- [x] `P2` **Bounded OCC retry** *(lands in the skeleton)*. `grep retry` in
      `trogon-decider-runtime/src/execution.rs` returns nothing. Optimistic-concurrency conflicts go
      straight to the caller, so two writers on one stream produce caller-visible errors instead of a
      re-read-decide-retry loop. Cheap now that snapshots make the re-read inexpensive.

      **Done.** `ConflictRetryLimit` (a `NonZeroU32` value object, so "no retries" is the absence of
      a limit rather than a limit of zero) plus `CommandExecution::with_conflict_retry` and its
      `WasmCommandExecution` twin. A retry needs all three of: the store classifying the failure as
      `AppendFailure::WriteConflict` through the new `StreamAppend::classify_append_failure` hook
      (default `Fatal`, so no existing store is retried behind its back), a declared
      `WritePrecondition::StreamUnchanged`, and no caller-supplied expected revision. Every other
      precondition asserts something a re-read cannot change. Retries are counted by
      `decider.command.conflict_retries`; exhaustion still surfaces as `CommandError::Append`.
- [x] `P2` **Chunked replay fold.** Replay materializes the full `Vec<StreamEvent>` in
      `ReadStreamResponse`. Folding in batches of N bounds peak memory on both paths and, at L2,
      makes guest fuel budgeting per-batch instead of per-stream. This is a `StreamRead` contract
      change, hence L1 and hence before the L2 adapters.

      **Done.** No `StreamRead` change was needed after all: `read_stream_bounded` already exists,
      so chunking is a cursor loop over advancing `ReadFrom` positions. `ReplayChunkSize` is a
      `NonZeroU64` value object next to `ReplayLimit`, and `ReplayBounds` / `ReplayCursor` are the
      shared piece both paths drive: `read_bound` takes whichever of the two is tighter, and the
      walk is pinned to the tail the first read observed, so a stream that grows mid-walk cannot be
      folded past the position the append is guarded on. `with_replay_chunk_size` on both
      `CommandExecution` and `WasmCommandExecution`, off by default. On the guest path each chunk
      is folded by its own `evolve` call, which is what makes the fuel and epoch budget per chunk
      rather than per stream.
- [x] `P3` **[ADR#0026](./docs/adr/0026-command-authorization-principal.md) - authorization
      principal** *(lands in the skeleton)*. Documented honestly in the architecture doc today:
      whoever can construct a `CommandExecution` can apply any command the decider accepts. No
      authorization phase, no principal input.

      **Done, for the seam the ADR scopes; the ingress half is still not built.**
      `trogon-decider-runtime/src/authorization.rs` owns `CommandPrincipal` (a `PrincipalKind`, a
      `PrincipalId`, a set of `PrincipalClaim`s, and an optional `DirectedPrincipal`), the
      `CommandAuthorizer` trait, the `AuthorizationDeniedError` and `UnauthorizedError` rejections,
      and `WithoutAuthorization`. Both `CommandExecution` and `WasmCommandExecution` gained a
      defaulted `Auth = WithoutAuthorization` parameter plus `with_principal` and
      `with_authorizer`; both call the authorizer immediately after the admission permit and
      before the first read, and both do it outside the conflict-retry loop, so a denied command
      costs one call and a command that conflicts three times still answers to one decision.
      `decision_outcome` gained a `denied` value, which per
      [ADR#0057](./docs/adr/0057-decider-command-nats-binding.md)'s own rule that the wire and
      trace vocabularies are one vocabulary forced a fifth `CommandOutcome` arm, `CommandDenied`,
      rather than folding a denial into `faulted.internal` and reporting it as a fault.

      The crate ships no rule and no claim vocabulary. `WithoutAuthorization` is named for the
      absence of a decision rather than for a permissive one, so an unconfigured execution
      carries that absence in its own type. The trait has two methods and implementations write
      only one: the runtime calls `authorize_execution`, whose default refuses an absent
      principal before `authorize` is consulted, so fail-closed is what an implementation gets by
      writing nothing.

      **Two deviations from the ADR draft**, both in the ADR now:
      1. The trait is `CommandAuthorizer<C: ?Sized>`, not `<C: Decider>`. The WASM path's command
      is a `CommandEnvelope`, which is not a `Decider`, so the drafted bound would have made the
      both-paths decision unimplementable and split the trait in two.
      2. The hook runs before the WASM path's stream-id resolution rather than after it, as the
      draft had it. Authorizing after would pay guest instantiation and fuel for a command about
      to be refused. The price is that an authorizer sees the principal and the command and never
      the target stream or its state, which the ADR now records as deliberate: a policy that must
      read stream state to decide is a domain rule, and a domain rule belongs in `decide`.

      **Three ADR questions had to be resolved before any of this could ship**, per the same
      [ADR#0000](./docs/adr/0000-adr-process.md) constraint that applied to [ADR#0028](./docs/adr/0028-decider-admission-control-and-backpressure.md) and #0029.
      All three resolutions are written into the ADR and need review rather than being treated as
      settled:
      1. *Placement.* The seam lives in the crate rather than in an application-owned wrapper. A
      wrapper's guarantee holds only for callers who go through it, is unverifiable by the
      runtime, and is unavailable to consumers outside the application that defines it. In-crate,
      "was this execution authorized" is answerable from the execution's own type.
      2. *Error surface.* `Unauthorized` goes on the shared error enums, the same cost
      `Overloaded` already imposed on every match.
      3. *AAuth coupling.* Confined to the ingress mapper, because `CommandPrincipal` is
      deliberately not an AAuth type; the unverified directed-principal hint sits in its own field
      so it cannot be mistaken for a verified claim.
- [x] `P3` **[ADR#0028](./docs/adr/0028-decider-admission-control-and-backpressure.md) - the
      storage-neutral half.** Define the admission traits here; the wasmtime-aware implementation
      belongs at L2.

      **Done.** `trogon-decider-runtime/src/admission.rs` owns the `CommandAdmission` trait, the
      `AdmissionLimit` value object, the `Overloaded` rejection, `WithoutAdmission` (the default,
      admits everything), and `ConcurrencyAdmission` (semaphore-backed, clones share one budget).
      `CommandExecution` gained a defaulted `A = WithoutAdmission` parameter and `with_admission`;
      the permit is taken at the top of the execute skeleton and released on drop, so a failing
      execution cannot leak a slot. `decision_outcome` gained a `shed` value in
      `otel/semconv/registry/decider.yaml`.

      **Two ADR questions had to be resolved before any of this could ship**, since [ADR#0000](./docs/adr/0000-adr-process.md)
      forbids implementing a draft. Both resolutions are written into the ADR and need review
      rather than being treated as settled:
      1. *Placement.* The seam lives in the shared crates, not only at the consumer boundary. The
         draft's own objections to the consumer-only placement were that it gives no per-host
         budget and no way for the shared crates to assert admission at all. The shape chosen is
         structurally identical to the `SnapshotPolicy` / `SnapshotFailurePolicy` /
         `SnapshotTaskScheduler` injections already in the crate.
      2. *Default.* Opt-in and no-op. The crates own a seam and one implementation but never a
         bound, because the number worth choosing is `admission_limit x max_memory_bytes` against
         a specific host's memory budget, which is a deployment fact. A host that configures
         nothing is exactly as unprotected as it was before.

      `admit` is synchronous on purpose: an admission decision that can await is a queue, and an
      unbounded queue relocates the pressure the limiter exists to bound while hiding it behind
      latency. A host wanting a bounded wait before shedding owns that wait in its dispatch loop.

## L1 - `trogon-decider-guest-macros`

No defects found. `export_decider!` has `trybuild` compile-fail coverage for the bundle-mismatch
cases (`tests/ui/bundle_mismatched_*.rs`).

---

## L2 - `trogon-decider-guest-sdk`

- [x] `P1` **`encode_current` unconditionally returns `Some`.** *(landed)* Resolved from the host
      side, which is where the decision belongs: the host now reads the declared cadence and only
      calls `session.snapshot` when one is due, so a guest that always has a frame to give is no
      longer a guest that forces a write. `encode_current` returns `Vec<u8>` accordingly and the
      generated `snapshot()` wraps it.

## L2 - `trogon-decider-nats`

Where the real correctness bugs live.

- [x] `P2` **Command-level idempotency. Must land before the L5 host.** *(landed)* `append_stream`
      sets `message_id(event.id.to_string())`
      (`crates/decider/trogon-decider-nats/src/stream_store.rs:346`), and event ids were freshly
      generated per execution by `UuidV7Generator`, so the JetStream dedup window could not recognize
      a redelivered command and a `Processor` retry after an ack timeout appended the events twice.

      `CommandId` (`trogon-decider-runtime/src/command_id.rs`) now names the delivery that carries a
      command and derives each decided event's id as `uuid_v5(command_id, index_be)`. Both execution
      paths take it through `with_command_id`, opt-in because an in-process command has no redelivery
      and because the id has to come from the transport: one generated inside the execution would be
      as fresh on the retry as the UUIDv7 it replaces. Deliveries that carry no id of their own use
      `CommandId::derive(namespace, key)` over a business key the sender cannot vary between
      attempts.

      Both real at-least-once entrypoints are wired. The worker processor names its command after the
      schedule event driving it, the same durable id it already uses as a stable `Nats-Msg-Id` for
      every other action. The RRULE wakeup consumer derives one from `(schedule_id, occurrence_at)`
      under a namespace of its own; its two attempts disagree on `recorded_at`, and that is the point,
      deduplication keeps whichever attempt won rather than recording the occurrence at two times.

      The window itself was the other half: `SCHEDULER_EVENTS` left `duplicate_window` at the server's
      implicit default of 120s, exactly the consumers' `ack_wait`, so the entry expired as the first
      redelivery arrived. It is now declared at 600s as a floor (an operator who widened it keeps
      theirs), reconciled onto existing streams, and refused at startup if too short.
- [x] `P3` **[ADR#0029](./docs/adr/0029-decider-retention-and-truncation-watermark.md) - retention
      watermark.** Nothing computes the minimum stream sequence still needed for a logical stream, so
      streams grow forever and replay cost grows with them. This is what makes snapshots actually pay
      off, and it is why the missing `ReplayLimit` at L2 matters more than it looks.

      **Done, for the read-only half the ADR actually scopes.** `trogon-decider-nats/src/retention.rs`
      owns `RetentionWatermark` (`RetainAll` or `DiscardBelow(StreamPosition)`, ordered so
      `RetainAll` is the least element and combining two constraints is `min`),
      `RetentionWatermarksBuilder`, the `RetentionWatermarks` report, and
      `read_retention_watermarks` for the single-snapshot-type case. Snapshot types are folded
      one at a time because each carries its own payload type; checkpoints fold as a constraint
      shared by every stream on the physical stream. Everything the fold cannot justify a
      boundary for resolves to `RetainAll`, so an incomplete picture over-retains rather than
      over-deletes.

      Nothing purges, and nothing in the store calls a purge: the ADR makes that an
      operator/job action and makes the job itself a Non-Goal. The residual sharp edge is
      recorded in both the ADR and the module docs: the watermark is only as safe as the set of
      markers folded into it, and a stream nobody declared does not appear in the report at all,
      so the aggregate is sound only over a complete set of stream ids.

      **Two ADR questions had to be resolved before any of this could ship**, per the same
      [ADR#0000](./docs/adr/0000-adr-process.md) constraint that applied to [ADR#0028](./docs/adr/0028-decider-admission-control-and-backpressure.md). Both
      resolutions are written into the ADR and need review rather than being treated as settled:
      1. *Placement.* The read-only query lives in `trogon-decider-nats`, because the watermark
      is derivable only from that crate's own KV key grammar, snapshot envelope, and the fact
      that its `StreamPosition` **is** the physical JetStream sequence. A layer above would have
      to reimplement all three. The purge stays out of the crate, which is where the operational
      decision actually lives.
      2. *Relationship to [ADR#0035](./docs/adr/0035-session-store-decider-aggregate.md).*
      Resolved by scope rather than precedence: what shipped is the read-only watermark, which
      #0035 explicitly retains as a diagnostic even for keep-forever session streams, so the two
      documents do not conflict on anything either currently specifies. #0035's supersession
      claim only bites once a purge job exists. Its cross-references were updated to match.
- [x] `P3` **[ADR#0027](./docs/adr/0027-decider-multi-tenancy-primitive.md) - declared subject scope.**
      Isolation across tenants depended entirely on the caller's `StreamSubjectResolver`, with nothing
      on the store side able to tell a correctly scoped subject from an escaped one.

      **Done, and not as the ADR drafted it.** The draft proposed a `Tenant` value object required on
      `StreamSubjectResolver`; what shipped is `SubjectScope` and a defaulted
      `StreamSubjectResolver::subject_scope()` returning `Option<&SubjectScope>`. A resolver declares
      the subtree it promises every subject it returns falls inside, and `JetStreamStore` refuses a
      subject outside it (`JetStreamStoreError::SubjectOutsideScope`) before the read or the append.
      `ModuleEventSubjects` in the L5 host declares `<module>.events.` and became fallible at
      construction, so a module name that cannot form a subtree fails at startup rather than on a
      command.

      The check is real rather than circular because the two halves have different origins: the scope
      is fixed when the resolver is constructed, the subject is derived per call from a stream id the
      resolver did not choose. That is also why snapshots are out of scope: `snapshot_key` is a type
      prefix plus a bare caller-supplied string with no resolver trait behind it, so there is no
      construction-time fact to check against. A shared multi-tenant deployment still owes snapshot-key
      scoping itself.

      **Both of the ADR's open questions were resolved here rather than signed off**, per the same
      [ADR#0000](./docs/adr/0000-adr-process.md) constraint that applied to
      [ADR#0028](./docs/adr/0028-decider-admission-control-and-backpressure.md) and
      [ADR#0029](./docs/adr/0029-decider-retention-and-truncation-watermark.md), and both want a
      reviewer:
      1. *Placement.* The crates own no `Tenant`. Tenancy vocabulary belongs to the consumer, which
      projects its own tenant value onto a scope; the only in-repo resolver with a real isolation
      boundary draws a *module* boundary, not a tenant one, which is the argument against naming the
      primitive after tenants at all.
      2. *The breaking resolver change.* The draft asked for sign-off on breaking every existing
      resolver. Dissolved rather than granted: `subject_scope()` is defaulted, so no resolver breaks.
      The only source-breaking change left is the new `SubjectOutsideScope` error variant, for
      exhaustive matchers.

      The ADR was retitled and flipped `draft` -> `accepted`, and its downstream references were
      updated across #0028, #0029, #0035, #0037, #0046, the decider architecture doc, the glossary
      (`tenant` rewritten, `subject-scope` added), the session events proto, and the ADK research
      comparison.
- [x] `P4` **Template-driven `trogon.error.v1alpha1` annotations** (deferred out of the #0026
      authorization work). The premise the deferral rested on was wrong twice over: the package
      already ships in the `trogon-proto` commit `buf.lock` pins, and `buffa` does emit `extend`
      blocks as `::buffa::Extension` constants, as `elixirpb.__ext.rs` had been showing all along.

      Shipped as `proto/trogonai/decider/v1/faults.proto`: one annotated message per host-owned
      reason, eleven in all, each declaring its domain, reason, code, and visibility. They are
      schema-only. Nothing references them, nothing encodes them, and the wire still carries the
      plain `google.rpc.Status` the arm always did, which is the whole point: the templates document
      the contract the host already honors instead of becoming a second way to transport it. The
      prose on `CommandOutcome`'s arms was cut back to what the templates cannot carry, and the
      [ADR#0057](./docs/adr/0057-decider-command-nats-binding.md) mapping table now points at them.

      `rejected` is deliberately untemplated: its domain is the module owning the code space and its
      reason is that module's own code, so neither is the host's to declare. Two things the template
      shape cannot express, left as prose: the typed details (`shed`'s `QuotaFailure`, `faulted`'s
      `DebugInfo`), and the fact that `Template.message` is the invariant description while the host
      substitutes live error text on emission.
- [ ] `P4` **Revisit where `Projector` / `Processor` live** (`src/projector.rs`, `src/processor.rs`).
      Generic JetStream read-side primitives with no decider dependency; possibly `trogon-nats`
      instead, per the [ADR#0002](./docs/adr/0002-rust-crate-boundaries.md) boundary argument. Cosmetic against the rest of this file.

## L2 - `trogon-decider-wasm-runtime`

Three capabilities that exist natively at L1 and are missing here. All three get much cheaper after
the L1 skeleton lands.

- [x] `P1` **Add `ReplayLimit`.** *(landed: `with_replay_limit`, shared
      `ensure_replay_within_limit`, `WasmCommandError::ReplayLimitExceeded`.)* Zero hits for `replay_limit` / `ReplayLimit` in this crate. Native
      fails with a typed `CommandError::ReplayLimitExceeded`
      (`trogon-decider-runtime/src/execution.rs:1051`, `:1246`) before folding anything. Worse than a
      missing guard: this crate hands every replayed event to a single fuel-bounded `evolve(events)`
      call, so a long stream fails as an `OutOfFuel` trap counted in `decider.wasm.traps` -
      indistinguishable from a buggy guest.
- [x] `P1` **Add a host-side snapshot policy.** *(landed: descriptor cadence, plus
      `with_snapshot_cadence` as the host override. Skipping the snapshot also skips the decided-event
      fold, which exists only to make that snapshot correct.)* L1 has `NoSnapshot`/`FrequencySnapshot`; this path
      writes a snapshot on *every* command. Every command on a hot stream pays a full state proto
      encode plus a KV round trip. Depends on the L0 descriptor decision and pairs with the
      guest-sdk item above.
- [x] `P1` **Call `read_stream_bounded`.** *(landed via the shared `read_stream_for_execution`, but
      only on the store-less configuration until the L1 skeleton refactor caught the snapshot path
      still calling raw `read_stream`. Both are bounded now and both are pinned by tests.)* Always reads `ReadFrom::Beginning`
      (`trogon-decider-wasm-runtime/src/execution.rs:435`, `:693`) even though `JetStreamStore`
      overrides the bounded read (`trogon-decider-nats/src/store.rs:262`). Pairs with `ReplayLimit`
      above - the limit only bounds wire traffic when the store's override is actually called.
- [x] `P3` **[ADR#0028](./docs/adr/0028-decider-admission-control-and-backpressure.md) - the
      wasmtime-aware half.** Defaults today are `max_concurrent_sessions` 256 x `max_memory_bytes`
      64 MiB, a 16 GiB worst case. **Prerequisite for the L5 host, not a follow-up.**

      **Done.** `WasmCommandExecution` mirrors the native shape exactly: the same defaulted
      `A = WithoutAdmission` parameter, the same `with_admission` builder, and a
      `WasmCommandError::Overloaded` reusing the runtime's `Overloaded` type. The permit is taken
      before the first `spawn_guest`, so a shed command never creates a wasm store and never
      touches the event store, which is what makes `limit x max_memory_bytes` a real bound rather
      than an estimate. Pinned by tests that assert a shed command reads and appends nothing, and
      that the slot comes back on both a successful and a rejected command.

      The 16 GiB worst case named above is unchanged for a host that configures no limiter, by
      design: the crates deliberately choose no default bound (see the L1 half). What ships is the
      means to bound it, and the L5 host is the first consumer expected to use it.

---

## L3 - `trogon-decider-sim`

- [ ] `P4` **Make parity a CI gate.** `assert_parity` covers the schedules decider and the act-chain
      fixture, but nothing requires it for a new component. Rule to enforce: every component in
      `wasm-components/` ships a YAML conformance suite *and* a parity test. Natural pairing with the
      L4/L5 conformance-gate work.

---

## L4 - `cli/trogon-decider-test` and `wasm-components/*`

- [x] `P4` **Issue #467 - `Decision::Act` support in the WASM path.** Already implemented:
      `trogon-decider-wasm-runtime/tests/act_chain_execution.rs` proves a multi-step chain crosses the
      real WIT/wasmtime boundary with later steps observing earlier steps' evolved state, and
      `wasm-components/trogon-act-chain-decider` is the fixture. **Action: close the stale issue.**
- [x] `P0` **Issue #470 - conformance gate, CLI half.** *(landed)* A suite failure was already a hard
      gate; the hole was coverage. `github-actions:decider-test-suites` iterated built artifacts and
      printed `no decider-test fixture for <alias>; skipping`, so a component added with no suite
      kept CI green. The task now iterates `wasm-components/*` and fails unless each one either has a
      passing `cli/trogon-decider-test/<alias>.yaml` or declares
      `[package.metadata.decider-test] exempt = "..."` in its own manifest, and it also fails on a
      suite that matches no component and on an exemption that contradicts an existing suite. The
      declaration lives in the component's manifest so deleting the component deletes its exemption.
      `trogon-act-chain-decider` is the one exemption: its command and events are hand-rolled buffa
      messages with no generated descriptors, so `codec::type_registry` has nothing to encode a suite
      against, and `trogon-decider-wasm-runtime/tests/act_chain_execution.rs` covers it instead.
      The registry half is at L5.

---

## L5 - The host that does not exist yet

*As found:* `trogon-decider-wasm-runtime` was depended on by exactly two things,
`trogon-decider-sim` and its own tests. Nothing bound `DeciderRegistry` + `JetStreamStore` + a NATS
subscription into a running service, and `WasmDeciderModule::load(engine, bytes)`
(`crates/decider/trogon-decider-wasm-runtime/src/module.rs:87`) had no byte source. The sandboxed
path was a well-tested library that could not serve a command; `trogon-scheduler` still uses only
the native path.

This layer is last **by dependency**, not by importance - it is the single change that turns the
WASM path from a library into a product. Everything below it exists to make it correct on the first
try.

- [x] `P0` **Add a `trogon-decider-nats-server` host crate.** *(landed)* One `{prefix}.>` core
      request/reply subscription, routed through `DeciderRegistryHandle`, executed against a
      per-module `JetStreamStore`, answered with a single `trogonai.decider.v1.CommandOutcome`.
      The binding is written down as [ADR#0057](./docs/adr/0057-decider-command-nats-binding.md),
      which is **`draft` and authored by the implementer**: per [ADR#0000](./docs/adr/0000-adr-process.md) it needs signoff from
      someone else, and its deliberate departure from
      [ADR#0016](./docs/adr/0016-protobuf-rpc-over-nats-micro-binding.md) (not a NATS micro service,
      because `activate`/`retire` break the static-endpoint invariant) is the decision most in need
      of review. Two known limits: there is no control plane for runtime activation yet, so the
      per-module store map is built once at startup from the configured modules; and startup refuses
      if an existing events stream does not already cover a newly configured module's subtree.
- [x] `P0` **Issue #465 - give `WasmDeciderModule::load` a byte source.** *(landed)* Written down as
      [ADR#0058](./docs/adr/0058-decider-module-distribution.md), also **`draft` and authored by the
      implementer**. A module is now named by a `ModuleReference` (`{name}@{version}`), never by a
      path: `--module scheduler.schedules@0.1.0` plus one `--module-store`, resolved through a
      `ModuleSource` trait the host is generic over so it cannot branch on which store it has. Two
      implementations ship: `ObjectStoreModuleSource` (a JetStream object store, the store the ADR
      picks, because every host already has a `jetstream::Context` and the bucket is replicated and
      access-controlled like any stream) and `FileModuleSource` for development. OCI is deferred
      until modules cross an organizational boundary. A fetched component is checked against its own
      descriptor before it is registered, so a component published under the wrong key fails startup
      naming both sides rather than serving one module's commands under another module's name.
- [x] `P0` **Issue #470 - conformance gate, registry half.** *(landed)* Per
      [ADR#0058](./docs/adr/0058-decider-module-distribution.md) the gate is at store entry, not at
      host start: a host that re-ran the suite would pay for it on every replica and every restart
      to learn something the publisher already established, and would still have no answer for a
      module that fails at three in the morning. `cli/trogon-decider-publish` is the only supported
      way a component enters the bucket. It runs the suite through the same
      `trogon_decider_test::conformance::run_suite` the CI gate calls (extracted out of
      `decider-test`'s `main` so there is one definition of "conformant" rather than two that
      drift), then loads the component exactly as a host will, then publishes it under the reference
      read from the component's own descriptor rather than one supplied on the command line.
      Nothing is written unless every step passes, down to not provisioning the bucket, so a refused
      publish and a bucket nobody published into are the same observable state.
      `cli/trogon-decider-publish/tests/gate.rs` proves that against a live JetStream.

---

## Suggested execution order

Bottom-up by layer, with the two genuine cross-layer bundles called out:

1. ~~**L0 `WritePrecondition` total enum.**~~ *(done)* Self-contained, one production call site, no dependencies.
   Do it first: it is the cheapest correctness win on the list and it settles half the contract bump.
2. ~~**L0 contract decision**~~ *(done)* - snapshot cadence in `module-descriptor`, yes or no. Bundle the
   resolved answer with step 1's WIT mirror into a single `0.3.0`. Blocks step 4.
3. ~~**L1 skeleton** (+ its wasm half at L2).~~ *(done)* One skeleton per crate rather than one
   across both, so OCC retry, admission, and authorization are a two-place change instead of four.
   See the L1 item for why the cross-crate session abstraction was not worth its cost.
4. ~~**L2 parity bundle**~~ *(done)* - `ReplayLimit`, snapshot policy (L0 + guest-sdk + wasm-runtime together),
   `read_stream_bounded`.
5. ~~**L2 idempotency**~~ *(done)* - the `message_id` fix. Hard prerequisite for step 7.
6. ~~**L2 admission** ([ADR#0028](./docs/adr/0028-decider-admission-control-and-backpressure.md)).~~ *(done)* Hard prerequisite for step 7. The ADR is ratified and
   both halves ship, but the two resolutions it needed (seam in the shared layer, opt-in no-op
   default) are the author's own and want a reviewer's signoff per [ADR#0000](./docs/adr/0000-adr-process.md).
7. ~~**L5 host**~~ *(done)* - the payoff. The crate serves; [ADR#0057](./docs/adr/0057-decider-command-nats-binding.md) that describes it is still
   `draft` and needs an approver other than its author.
8. ~~**L5 module distribution** (#465 + the registry half of #470).~~ *(done)* The host fetches by
   reference from a store, and `cli/trogon-decider-publish` is the gate that fills it.
   [ADR#0058](./docs/adr/0058-decider-module-distribution.md) is also `draft` and also authored by
   the implementer, so it needs the same outside signoff.

9. ~~**Retention ([ADR#0029](./docs/adr/0029-decider-retention-and-truncation-watermark.md)), subject
   scope ([ADR#0027](./docs/adr/0027-decider-multi-tenancy-primitive.md)), and authorization
   ([ADR#0026](./docs/adr/0026-command-authorization-principal.md))**~~ *(done)* - none of them blocked
   the host, and all three needed a resolution the author supplied and a reviewer has not.

---

## Related open issues not yet placed

- #466 - browser jco host for unit testing and AI-sandbox simulation (would be a new L3 sibling)
- #469 - expose the sim host as an MCP tool over `mcp-nats` (L3/L4)
