# Session Maintenance

Two operator workflows rewrite or duplicate a session's durable history:
**migration**, which rewrites a session's stored events under a newer storage
schema and keeps its identity, and **salvage**, which copies what can be read
from a damaged session into a new identity. Both can be interrupted between
doing the work and learning that the work landed. This page documents the
protobuf contract that makes that interruption recoverable. There is no Rust
implementation yet.

See [Session Doctor](./session-doctor.md) for diagnosis, which reports these
problems but is not allowed to fix them, and
[Session Aggregate](./session-aggregate.md) for `SessionOrdinal` and the
creation-batch preconditions both workflows rely on.

## The failure this exists for

A migration writes the replacement stream, commits, and then loses the
acknowledgment. The process crashes, or the network drops, or the pod is
evicted. On restart the workflow knows it started and does not know whether it
finished.

Both available reflexes are wrong. Running the migration again copies a session
that may already have been copied, and the second copy overwrites the first
while looking like progress. Giving up leaves the session in a state nobody will
revisit, because there is no record saying it needs revisiting.

The workflow has to be able to **look at durable state and decide**, which means
there must be something recorded on the near side of the crash to compare
against.

## Intent before work, always

Both workflows write an intent record before touching anything:

1. Record the intent.
2. Stage the target.
3. Commit.
4. Validate.

The intent is the whole mechanism. `MigrationIntent.expected_target` carries the
boundary the target will have if the commit lands, computed before the commit is
attempted. After a crash, `ReconcileMigration` reads whatever is at the target
location and compares it to that. The comparison answers a question a bare retry
cannot: not "did some write land" but "did **my** write land."

`INDETERMINATE_REASON_INTENT_MISSING` exists so that skipping this step is a
reportable condition rather than a mystery.

### Which means the transformation must be deterministic

Predicting a target digest before writing it only works if the transformation is
byte-deterministic: the same source bytes and the same `implementation_version`
produce the same target bytes. That is an admissibility requirement, not an
aspiration, and it is why the encoder is scoped to the version rather than
floating.

`ContentDigest` commits to the event bytes exactly as the log stores them, never
to a re-serialization. Protobuf encoding is not canonical, so a digest taken
over a decode-then-re-encode round trip would not reproduce, and a workflow
whose central check is "is this the same content" cannot rest on a comparison
that fails against bytes the system itself wrote. The
[page cursor envelope](./session-pagination.md) keeps its payload as raw bytes
for exactly the same reason.

A transformation that cannot reproduce its own output is not a migration. It is
a salvage, and it must mint a new identity.

## The four verdicts

`ReconcileMigration` reads and returns one of four answers. It has no mode, no
force, and no field that could make it write, on the same reasoning
`DiagnoseSession` has none: a reconciler that can also commit is a reconciler
that can blindly commit.

| Verdict | Evidence | Correct action |
| --- | --- | --- |
| `NOT_COMMITTED` | No target exists | Retry under the same identity |
| `COMMITTED` | Target matches the intent exactly | Record success; do not migrate again |
| `DIVERGED` | Target exists and does not match | Stop. No automated action is safe |
| `UNRESOLVED` | The comparison could not be made | Stop. An operator decides |

`DIVERGED` is the one that matters. A truncated copy written by a prior attempt
and an intact copy written by something else look identical from here, and they
call for opposite actions. Guessing is how a session gets destroyed by the tool
that was supposed to preserve it.

The user's lost acknowledgment lands on `COMMITTED`, and the workflow reports
`MIGRATION_RESULT_ALREADY_MIGRATED` rather than `MIGRATED`. A retry that claimed
credit for work it did not do would make a fleet run's completion count a
fiction.

### Unknown and indeterminate are different states

`MAINTENANCE_STATE_COMMIT_UNKNOWN` says nobody has looked yet, and looking is a
mechanical step the workflow performs on its own.
`MAINTENANCE_STATE_INDETERMINATE` says the workflow looked and the evidence does
not decide.

Indeterminate is a conclusion, not the absence of one. Collapsing the two would
make an unattended retry loop indistinguishable from a problem that needs a
human, and the second would sit in a queue behind the first forever.

## Migration keeps ordinals; salvage keeps nothing

| | Migration | Salvage |
| --- | --- | --- |
| Identity | Preserved | New, derived |
| Ordinals | Preserved exactly | Not preserved |
| Source | Must be intact | Is damaged |
| Source afterwards | Replaced | Untouched |
| Loss | Inadmissible | Expected and enumerated |

Ordinal preservation is a hard rule, and the reason is that `SessionOrdinal` is
referenced from **outside** the stream it belongs to. A child session's fork
context boundary, checkpoint evidence, a consistency token, a page cursor: each
names a position on someone else's stream. Insert or drop one event during a
migration and every one of those references silently comes to mean a different
event. Nothing fails. Nothing logs. The fork simply inherits a slightly
different prefix than it did yesterday.

So a migration is one transformed payload per source position, or it is not
admissible. `AdmissibilityReason` enumerates the ways a plan fails that rule:
an event type removed from the target schema, a value the target cannot
represent, source bytes that will not decode. Every one of them is a reason to
reach for salvage instead. **A migration is total or it is not a migration.**

## Salvage mints a session, and says so

Salvage has the same lost-acknowledgment problem and solves it more cheaply,
because the write it performs is a creation batch.

`SalvageIntent.target_session_id` is derived from the salvage identity before
any write is attempted. A retry derives the same id, and the
`[SessionStarted, SessionRecovered]` batch appends under a `NoStream`
precondition, so the duplicate attempt is rejected by the store rather than
resolved by a later comparison. Reconciliation is not needed because the write
itself cannot happen twice.

A randomly minted id would break this completely: a retry could not find its own
prior work, and every interrupted salvage would leave an orphan session nobody
could attribute.

### Why `SessionRecovered` is an event

It would have been simpler to keep salvage provenance in the operator-side
record. It is an event because a projection is rebuilt from the stream, and
provenance that lived only in a journal would vanish on the next rebuild. The
salvaged session would then read as an ordinary session that happens to have a
short history, which is exactly the substitution FX-13 forbids.

For the same reason the event carries `omitted_count` and not just a pointer to
the record: a fold of this stream alone must be able to conclude the session is
incomplete, without an operator-side lookup that may not be available to it.

`SessionRecovered` is not `SessionForked`. A fork inherits a prefix it assumes
is valid and references it in place, which requires the source to stay readable
forever. A recovery copies, because the source's readability is the thing that
failed. The copied events are the new session's own events at its own ordinals;
`source_boundary` is provenance, not an index into this stream.

### What was lost is enumerated, not counted

`OmittedItem` names each thing the salvage could not carry, with a kind and a
reason. An operator told that three things were lost cannot tell whether they
were three tool results or three user messages, and that difference decides
whether the salvaged session is worth keeping.

Two omission reasons are worth calling out:

- `DIGEST_MISMATCH` content is never carried. A salvage that copied content
  failing its own integrity check would launder corruption into a session that
  looks healthy.
- `ERASED` content was not lost. It was destroyed under a privacy obligation,
  and a salvage must not resurrect it. It is recorded so the gap in the salvaged
  history is explained rather than read as further damage.

There is no `recovered_with_unverified_artifacts` result, which is a deliberate
departure from the shape Fx uses. History completeness and artifact
verification are independent axes, and one flat enum forces the server to choose
which fact to report. The omissions carry both, and a second signal that could
disagree with them would only invite a caller to pick.

## The read side has to show it

A salvaged session that renders identically to an intact one defeats the whole
workflow. `RecoveryProvenanceView` is therefore on `SessionSummary`, the list
row, and not only on the detail view: a picker is where the substitution
actually happens, because by the time a user opens a session they have already
decided it is theirs.

The view carries the source id, a completeness value, and a count. It does not
carry the enumerated omissions. A client needs to know the session is
incomplete; rendering a damage report is an operator's job.

## Modes default to writing nothing

`MigrationMode` and `SalvageMode` both treat the unset zero value as
`PLAN_ONLY`, matching `REPAIR_MODE_UNSPECIFIED` on the doctor. A caller that
forgets the field gets a plan, and `WOULD_MIGRATE` or `WOULD_SALVAGE` says
plainly that nothing was written. `SALVAGE_RESULT_WOULD_SALVAGE` still carries
the planned omissions, so an operator can see what a salvage would cost before
authorizing it.

An operation that ran and could not be classified is **not** a
`MaintenanceError`. It is a successful call reporting an indeterminate state,
because it has a durable record and a defined next step, and turning it into an
error would discard both.

## Layout

`proto/trogonai/session/sessions/maintenance/v1alpha1/`:

| File | Contents |
| --- | --- |
| `stream_boundary.proto` | `StreamBoundary`, `ContentDigest`, `OrderProof` |
| `outcome.proto` | `MaintenanceState`, `IndeterminateDetail`, `IndeterminateReason` |
| `migration.proto` | identity, intent, record, result, admissibility |
| `migrate_session.proto` | request/response pair |
| `reconcile_migration.proto` | request/response pair, `ReconciliationVerdict` |
| `salvage.proto` | identity, intent, record, omissions, result, refusal |
| `salvage_session.proto` | request/response pair |
| `maintenance_error.proto` | `MaintenanceError`, `MaintenanceErrorCode` |

On the write side, `RecoverSession` and `SessionRecovered` join the session
command and event catalogs, and `RecoveryOrigin` joins the aggregate state.
On the read side, `RecoveryProvenanceView` joins `SessionSummary`.

There are no `service` definitions, matching the rest of this repo. Transport
binding is JSON-RPC over NATS
([ADR#0055](../adr/0055-nats-subject-design-jsonrpc-bindings.md),
[ADR#0056](../adr/0056-canonical-jsonrpc-bodies-over-nats.md)).

## Status

Shipped: the eight protos above, plus `recover_session.proto`,
`session_recovered.proto`, the `SessionRecovered` arm on `SessionEvent`,
`RecoveryOrigin` on `State`, and `RecoveryProvenanceView` on the query contract.
Lint-clean, formatted, building, and generating Rust bindings reachable at
`trogonai_proto::session::sessions::maintenance_v1alpha1`. The event validator
covers `SessionRecovered`, including the cross-field rule that a complete
recovery cannot report omissions and a partial one cannot report none.

Not shipped: the transformations themselves, intent storage, the reconciler, the
salvage reader, and the target-id derivation function.

The dependency that was open when these protos landed is now settled.
[ADR#0059](../adr/0059-session-stream-incarnation-fencing.md) defines a stream
incarnation as a subject token isolating one rebuild of the physical stream from
the next, so `StreamBoundary.incarnation` is required rather than optional: an
unset incarnation proves nothing, and a comparison that can never fail is not a
comparison. `INDETERMINATE_REASON_SOURCE_INCARNATION_CHANGED` now names a
condition the runtime can actually observe, because a boundary read under one
incarnation and a boundary read under another are read through different
subjects.

That decision also relocates the migration this contract describes. A schema
migration is a rebuild of the physical stream into the next incarnation, which is
what lets it replace a session's stored bytes without any event leaving the log:
the retiring incarnation is sealed and kept, not edited. `MigrateSession` is one
session's participation in that rebuild, which is why ordinal preservation is an
admissibility rule rather than a nicety. Ordinals are the only positions that
survive an incarnation change.
