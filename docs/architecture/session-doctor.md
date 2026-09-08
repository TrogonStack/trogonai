# Session Doctor

The doctor inspects a session and reports what disagrees with what. Repairs are
a separate operation that can only act on findings from a specific report, and
only on derived state. This page documents the protobuf contract that exists
today. There is no Rust implementation yet.

See [Session Aggregate](./session-aggregate.md) for the write side and
[Session Projection Freshness](./session-projection-freshness.md) for the
watermarks several checks compare against.

## Why diagnosis and repair are different operations

The obvious design is one operation with a `dry_run` flag. It is also the design
that eventually deletes something, because report-only then lives in a field,
and a field can be forgotten, defaulted, or lost in a refactor.

So `DiagnoseSession` has no mode. It mutates nothing and has no field that could
make it mutate. Reaching a mutation requires calling a different operation with
a different message, which is a thing a caller does on purpose.

Where a flag is unavoidable, on the repair itself, the numbering carries the
default: `REPAIR_MODE_UNSPECIFIED = 0` is dry run. A caller that forgets the
field, decodes an older message, or misses a branch when building a request gets
a preview. Any other ordering makes the destructive path the default, and a
default that destroys is a defect documentation cannot fix.

## No repair touches the event stream

Every value in `RepairAction` operates on derived state: rebuild a projection,
reset a checkpoint, quarantine a generation, discard a snapshot, re-verify an
artifact, release an orphaned claim.

That is a boundary, not a starting set. The event stream is the source of truth
and it is append-only. A repair that edited or deleted events would make the log
depend on the tooling built to inspect it, and every guarantee resting on replay
determinism would rest instead on nobody having run the wrong repair.

The corollary catches what operators most expect to find here. A tool call that
started and never recorded an outcome is exactly the kind of thing a doctor
surfaces, and reconciling it is **not** a repair. It is `CompleteToolCall` or
`FailToolCall`, an ordinary command through the ordinary decider, subject to the
ordinary invariants. Routing it through the doctor would let an operator write
history without passing the rules that make the history mean anything.

This is also why severity is the enum it is. `DEGRADED` means derived state
disagrees with an intact stream, so rebuilding should fix it. `CORRUPT` means
the stream cannot be fully interpreted, so nothing can, and those findings
usually carry no available repairs. That is the honest answer rather than a gap
in the list.

A `CORRUPT` finding leaves exactly one path forward, and it is not here.
Salvage copies what can still be read into a new session identity, which works
precisely because it does not pretend to repair anything: the damaged source is
left untouched and the copy is marked as a copy. See
[Session Maintenance](./session-maintenance.md).

## An empty report is not health

`DiagnoseSessionResponse` carries `checks` alongside `findings`, one entry per
check the server considered, including ones it skipped.

A doctor that exhausted its budget after two checks returns the same empty
`findings` list as a clean session. An operator reading only `findings` cannot
tell those apart, and the failure mode is the worst available: a diagnostic
that reports health it did not verify, to someone who ran it precisely because
they were worried.

So an empty `findings` list means healthy only when every entry in `checks`
reports `CHECK_STATUS_COMPLETED`. `BUDGET_EXHAUSTED` says findings so far are
real and their absence proves nothing. `FAILED` says the check itself broke,
reported rather than swallowed.

The budget exists because inspection is unbounded by nature: verifying every
artifact digest on a long session means reading every artifact. Without a bound
the doctor becomes a load generator operators learn not to run, and a diagnostic
nobody dares use is worth nothing. `CHECK_KIND_ARTIFACT_DIGEST` and
`CHECK_KIND_ORPHAN` are outside the default set for that reason, one because its
cost scales with storage rather than the stream, the other because deciding an
orphan means tracing durable ownership and projector watermarks.

## Repairing safely

Three properties, each closing a failure operator tooling reaches for eventually.

**Targeted.** There is no "repair everything" shape. A caller names exact
findings and the exact action for each. A bulk repair is a repair whose blast
radius nobody reviewed. The action must also be one the finding offered in
`available_repairs`, so what is legal is decided by the code that understands
the finding rather than by the caller.

**Provenanced.** `diagnosis_id` binds every mutation to the report that
justified it. A repair naming an unknown diagnosis is refused rather than
treated as an unprovenanced repair, and one naming a diagnosis too old to
authorize a mutation fails with `DIAGNOSIS_EXPIRED`.

**Re-verified.** The server re-runs each finding's check before acting. The
interval between running a doctor and running a repair is where state changes,
and a repair that trusts a ten-minute-old report will discard a projection that
already rebuilt itself. When re-verification finds the problem gone, the result
is `NOT_REPRODUCIBLE`, which is a success: doing nothing was correct.

`finding_id` is a digest over the finding's kind and subject, not over its
observed values. A projection that fell further behind between diagnosis and
repair is still the same finding, and an id that changed with every observed
byte would make repairs impossible on a busy session. Identity is stable;
staleness is caught by re-verification.

Results are per target, because a repair call is not atomic and pretending
otherwise would hide which half of it landed. `REFUSED_UNSAFE` is the interesting
status: the finding still holds and the server declined anyway. An unreadable
artifact is the standard case. Unreadable is not the same as absent, it may be a
permissions or transport problem, and nothing that treats it as data loss may
run on that evidence.

## Orphans, and why finding one is not enough to delete it

An artifact upload succeeds and the append that would reference it fails. The
bytes are durable, nothing points at them, and nothing ever will. Multiply that
by every crash and every timeout and it becomes a storage bill.

Cleanup is the dangerous half. The obvious implementation searches for
references, finds none, and deletes. It is wrong for two reasons, and neither
one shows up in testing.

The first is that a reference search is not an ownership trace. Content-addressed
storage means the same bytes can be reachable through a stream nobody thought to
scan, so "no reference found" is only meaningful alongside where the search ran
and where it stopped. A check that scanned four of a tenant's streams produces
the same empty result as one that scanned all of them. `OwnershipTrace` is what
separates them: streams read, streams that could not be read, the position the
scan covered through, the head at the time, and how far the projectors it
consulted had applied. Any unreadable stream makes the trace non-exhaustive no
matter how many others were scanned, because the reference could be in exactly
that one.

The second is timing. An append whose write state is indeterminate may still
land, and if it lands it carries a reference to something a cleanup pass has
already decided nothing points at. See
[Session Crash Boundaries](./session-crash-boundaries.md) for how that
indeterminacy arises. So `ReleaseGate` carries the indeterminacy window
alongside the minimum age, and the minimum age has to exceed it. A resource
inside that window is held no matter how thoroughly it was traced: the trace was
accurate, and the history had not finished happening.

`GateOutcome` is a single value, its zero holds, and `RELEASABLE` is last, so an
unrecognized variant keeps the resource rather than deleting it. The outcomes
that hold say which condition is holding, and `releasable_after` is unset for
the ones that will not resolve by waiting.

`OrphanClass` enumerates five kinds rather than one, because the cost of getting
it wrong is not uniform. Releasing an abandoned projection generation costs a
rebuild. Releasing a claim check that was about to be referenced destroys the
only copy of a command's output. Releasing an expired reconciliation lease is
not a deletion at all: the work still needs settling, and reclaiming the lease
is what lets someone else settle it.

That split runs through the repair actions too.
`REPAIR_ACTION_RELEASE_ORPHANED_CLAIM` destroys content the stream cannot
regenerate. `REPAIR_ACTION_DISCARD_ORPHANED_DERIVED_STATE` destroys something it
can. `REPAIR_ACTION_RECLAIM_EXPIRED_LEASE` destroys nothing. An operator
deciding how carefully to look before running one is deciding on exactly that
difference.

`CHECK_KIND_ORPHAN` stays out of the default check set. Tracing ownership is
expensive, and a check that is expensive enough to be tempting to approximate is
one that should be asked for explicitly.

And the gate is re-evaluated at repair time, not trusted from the report.
Everything it asserts can stop being true in the interval, which is the same
reason no other repair trusts a finding it did not re-verify.

## The example, end to end

A projection checkpoint claims to have processed an event the materialized view
does not contain.

1. `DiagnoseSession` with the default checks. `CHECK_KIND_PROJECTION` runs.
2. It emits a `FINDING_KIND_PROJECTION_CHECKPOINT_INCONSISTENT` at severity
   `DEGRADED`, with a `ProjectionCheckpointDetail` carrying all three positions:
   what the checkpoint claims, what the view reflects, and where the source head
   is. One or two of those numbers is not a diagnosis. Three is.
3. `available_repairs` offers `RESET_PROJECTION_CHECKPOINT` and
   `REBUILD_PROJECTION`.
4. The operator runs `RepairSession` in dry run. The finding re-verifies, and
   the result is `WOULD_APPLY` with `affected` naming the generation.
5. The operator re-runs with `APPLY`.

Nothing between steps 1 and 4 could have mutated anything, and step 4 could not
have either.

The artifact half of the example diverges usefully.
`FINDING_KIND_ARTIFACT_DIGEST_MISMATCH` carries an
`ArtifactDigestObservation` distinguishing `MISMATCH`, `ABSENT`, and
`UNREADABLE`, because the digests alone cannot. Only `MISMATCH` is evidence of
corruption. `UNREADABLE` is evidence of nothing yet, and the doctor says so
rather than letting an operator infer loss from a failed read.

## Layout

`proto/trogonai/session/sessions/doctor/v1alpha1/`:

| File | Contents |
| --- | --- |
| `finding.proto` | `Finding`, `FindingKind`, `FindingSeverity`, `SubjectRef`, typed details |
| `repair_action.proto` | `RepairAction`, and the rule that bounds it |
| `diagnose_session.proto` | request/response, check selection, budget, per-check outcome |
| `repair_session.proto` | request/response, modes, per-target results |
| `orphan.proto` | `OrphanClass`, `OrphanDetail`, `OwnershipTrace`, `ReleaseGate` |
| `doctor_error.proto` | `DoctorError`, `DoctorErrorCode` |

A sibling subtree rather than part of `queries`, and its own error type rather
than `QueryError`. The doctor is an operator surface with its own audience and
release cadence; pinning it to the client contract would make every new operator
diagnostic a client-visible change. That is the reasoning
[ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 3 applies to
projection value types, one subtree further out.

`FindingKind` is treated as open. A reader meeting an unrecognized kind must
surface it as an unknown finding rather than drop it, and `kind` sits outside
the detail `oneof` so it stays readable when the detail variant does not. A
dropped finding reads as health.

An expired supervision lease is reachable as an orphan class because the lease
policy is on the log even though the heartbeat never was; see
[Session Detached Work](./session-detached-work.md).

## Status

Shipped: the six protos above, lint-clean, formatted, building, and generating
Rust bindings reachable at `trogonai_proto::session::sessions::doctor_v1alpha1`.

Not shipped: every check, the ownership tracer the orphan check depends on, the
repair executor, diagnosis storage and expiry, the transport binding, and the numeric JSON-RPC error-code reservation, which is
made by decision rather than invented in a proto file.

The finding taxonomy is ahead of the system it describes. Several kinds name
subsystems that do not exist yet, including projections, snapshots, and the
recovery checkpoint attestations. They are defined now because the taxonomy is
the part that has to be right before anything reports against it, and because a
check added later against an existing kind is not a contract change.
