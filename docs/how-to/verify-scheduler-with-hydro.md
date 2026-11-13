# Verify scheduler decisions with Hydro

Use this check before changing scheduler command behavior. Hydro enumerates
interleavings of ordered client command streams. For every resulting order, the
test calls the production `trogon-scheduler-domain` deciders and folds their
actual events.

## Run the checks

From the repository root:

```sh
mise run verification:hydro
```

The experiment also has an explicit local Bazel launcher:

```sh
mise exec bazel -- bazel run //projects/hydro-verification:verify
```

The task selects the verification project’s Rust 1.98.1 compiler and runs its
locked integration tests. Install the compiler with `mise install rust@1.98.1`
if needed. The first run compiles Hydro and its generated simulation libraries.
No broker, container, service, or `cargo sim` installation is required.

To select a test, pass its name:

```sh
mise run verification:hydro removal_stays_terminal_across_client_interleavings
```

The Bazel launcher forwards the same filter after `--`:

```sh
mise exec bazel -- bazel run //projects/hydro-verification:verify -- removal_stays_terminal_across_client_interleavings
```

## Keep the experiment local

Hydro verification is experimental and excluded from CI. The Bazel target is
tagged `manual`, which omits it from `bazel build //...` and `bazel test //...`.
Both supported entrypoints reject a nonempty `CI` or `GITHUB_ACTIONS` variable
before starting mise or Cargo for the experiment. Standard repository PR checks
continue independently.

Bazel stages and runs a local launcher. The launcher uses the pinned mise/Cargo
workflow because Hydro generates and compiles simulation libraries through
Cargo at runtime. These are not native Bazel Rust actions or Bazel-cached test
results. Invoke the explicit target whenever you want fresh verification.

## Check the launcher

The launcher tests use a fake mise executable to check CI refusal, workspace
selection, and argument forwarding without running Hydro:

```sh
mise exec python -- python -m unittest discover -s projects/hydro-verification/tests -p test_launcher.py
```

## Check formatting and lints

Run formatting and lint checks from `projects/hydro-verification`:

```sh
mise exec -- cargo fmt --all -- --check
mise exec -- cargo clippy --locked --all-targets -- -D warnings
```

## What runs

The Hydro graph merges client command streams with `merge_ordered`, preserving
each client’s order while exploring their relative order. The graph outputs
command orderings. Inside each simulator execution, the host test calls the
public `CreateSchedule`, `PauseSchedule`, `ResumeSchedule`,
and `RemoveSchedule` implementations from `trogon-scheduler-domain` through
`trogon-decider`. These are the implementations re-exported by the production
scheduler and used by the scheduler WASM component.

The tests check that removal remains terminal despite interleaved lifecycle and
creation commands, duplicate creation does not create another history, missing
schedules reject lifecycle commands with the correct typed errors, accepted
commands emit the expected event kind and schedule identity, and replay of
accepted events reproduces the resulting state. A rejected command must produce
the appropriate typed rejection.

The output reports Hydro’s exhaustive execution count for each finite scenario.
Changing production decider logic changes the behavior these tests execute.

## Scope of the evidence

The scenarios operate on one synthetic schedule identity. They cover finite
serialized command orderings: the removal scenario interleaves client streams
of four commands each; duplicate creation interleaves streams of three commands
each; missing-schedule rejection interleaves streams of two and three commands.
Created schedules use an Every schedule with a 30-second interval. Each execution
starts from fresh state. Hydro’s schedule enumeration happens in
its generated library, and the production deciders execute in the host assertion
closure for each enumerated order.

A command decision and its event fold are treated as one serialized operation.
These checks do not exercise optimistic-concurrency retries, broker persistence,
publish/checkpoint/acknowledgment ordering, process crashes, or distributed
processor failover. They do not establish exactly-once delivery or an unbounded
proof. Existing processor fault-injection tests remain separate evidence for
those implementation paths.

## Extend the harness

Add a finite client command sequence and an independently observable property.
Keep calls to production code in the assertion closure. When adding a new
command, check its typed rejection and emitted event contract as well as the
final state.

Hydro is pinned by Git revision because upstream reuses a prerelease package
version across simulator API changes. The project has a separate Cargo lock and
compiler pin so this development dependency does not change production builds.
Hydro derives a generated dynamic-compilation lockfile from the outer lock; the
locks are not claimed to be byte-identical.
