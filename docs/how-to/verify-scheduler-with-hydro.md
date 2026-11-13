# Verify scheduler recovery with Hydro

Use this harness to explore recovery-phase interleavings before changing the
scheduler's processing protocol. It tests a bounded abstract protocol, not the
live scheduler, NATS server, or a process killed during I/O.

## Run the checks

From the repository root:

```sh
mise run verification:hydro
```

The task enters `projects/hydro-verification`, selects its Rust 1.98.1 compiler
with mise, and runs the locked integration tests. If the compiler is missing,
install it with `mise install rust@1.98.1`. No broker, container, deployment, or
`cargo sim` installation is required.

To run a named test, pass its filter:

```sh
mise run verification:hydro negative_control_exposes_cross_instance_pause_resume_race
```

Run formatting and lint checks from `projects/hydro-verification`:

```sh
mise exec -- cargo fmt --all -- --check
mise exec -- cargo clippy --locked --all-targets -- -D warnings
```

## Read the results

The recovery test models one schedule's create, pause, and resume events in
order. Hydro's `merge_ordered` chooses where an injected fault occurs among
the phase advances; a stateful `scan` executes each transition. A run supplies
28 advances and either no fault or one fault: crash, publish rejection, accepted
publish with lost response, checkpoint rejection, or lost acknowledgment.
The test reports Hydro's completed exhaustive execution count for each scenario.
That count is relative to these finite inputs and modeled choices.
Rejection and response-loss faults affect the next matching operation; a crash
resets the current attempt immediately. A fault injected after its final
applicable operation may remain unused in that execution.

The assertions inspect simulator-produced observations independently of the
state machine. They require acknowledgments to be covered by checkpoints,
checkpoint advancement to follow the corresponding scheduling effect, and
successful bounded retry to finish at the expected resumed schedule. The tests
also require exploration to encounter publish rejection and acceptance with a
lost response as distinct outcomes. They do not treat a missing publish response
as proof that no effect happened.

Negative controls change the modeled protocol to acknowledge before saving a
checkpoint or save a checkpoint before its scheduling effect. The suite passes
only when the assertions detect the invalid behavior. These controls keep the
normal command green while checking that the oracle rejects meaningful mistakes.

The cross-instance control starts from a created schedule, then lets an older
pause and newer resume independently read, purge, publish, CAS-save, and
acknowledge. Hydro interleaves their ordered phase streams. The test requires an
observed trace in which both acknowledge and the checkpoint says resumed while
the execution subject is empty. That is the limitation already documented in
the scheduler's `worker/consumer.rs`; it is not a newly reproduced production bug.
The output includes the actual model input trace for a counterexample.

## Preserve the model's assumptions

The positive recovery model assumes one active processor and serial delivery
for each schedule key. It excludes schedule firing, time progression,
permanent failures, checkpoint corruption, poison records, concurrent keys,
multiple injected faults, and unbounded crash/retry sequences. Convergence is
conditional on the remaining successful advances. A pause may still allow a
previously armed occurrence to record progress under the production contract.

No assertion establishes exactly-once downstream message delivery, JetStream
durability, failover readiness, or an unbounded theorem. Hydro Lean is not used.
The model's boolean execution subject describes schedule replacement, not the
delivery history of an external recipient.

## Keep correspondence reviewable

The model abstracts the scheduler's orchestration rather than importing its
crate-private reconciliation implementation and entire production dependency
graph. `projects/hydro-verification/build.rs` binds the inspected source files
with SHA-256. A source change fails the build and asks for correspondence review;
refreshing a hash alone does not prove equivalence.

| Model transition | Production source under `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/` |
| --- | --- |
| Read checkpoint and ignore covered event | `reconciliation/reconcile.rs`, `reconcile` |
| Purge, then accepted publish and response | `execution_schedules/mod.rs`, `ExecutionScheduleWriter::upsert` |
| Apply effect, save checkpoint, return acknowledgment | `worker/processor.rs`, `apply_reconciliation` |
| Resolve CAS conflict as covered or retry | `worker/processor.rs`, `resolve_save_conflict` |
| Create or revision-guarded checkpoint update | `checkpoints/store.rs`, `ScheduleCheckpointStore::save` |
| Serial lanes within one active instance | `worker/dispatcher.rs` and `worker/consumer.rs` |

The recovery and replica cases were designed against the existing selected fault-injection
tests `crash_after_execution_schedule_write_before_checkpoint_write_retries_and_converges`,
`transient_execution_schedule_failure_does_not_advance_checkpoint`, and
`checkpoint_update_cas_loss_retries_and_converges` in `worker/processor/tests.rs`.
Those tests remain separate Rust implementation evidence. Running this command
does not run them or establish process-crash acceptance.

Hydro is pinned by Git revision in the project's manifest because upstream uses
the same prerelease package version across changes to simulator APIs. The outer
Cargo dependency graph is locked. Hydro creates a separate generated manifest
for dynamic compilation and derives its lockfile from the workspace lock; its
generated lockfile is not claimed to be byte-identical to the checked-in lock.
The selected Rust compiler and Cargo environment also apply to those subprocesses.

When extending the model, retain the source bindings, separate accepted effects
from lost responses, expose new failure boundaries as Hydro choices, and add an
independent assertion plus a bad variant that demonstrates detection.
