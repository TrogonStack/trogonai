# Measure sandbox latency

This guide measures how long the sandbox platform takes to serve a lease. Three tools cover different questions. Two are in `trogon-sandbox-conformance`:

- The `memory_worker` bench measures the platform's own overhead: NATS round trips, JSON encoding and chunked transfer, with no container or VM behind the lease.
- The `latency` example times each step of a lease's life against a real worker: create, first exec, file write, file read, snapshot and delete.

The third is the `admission` bench in `trogon-sandbox-broker`, which measures broker admission alone, with no NATS server behind it. The conformance tools report p50 and p95. The admission bench is a criterion bench and reports a time range per measurement.

## When to use this

- Before and after a change to the worker protocol, lease handling or chunked transfer, to see whether it moved the numbers.
- When comparing providers on the same machine.
- When you need measured create times for a provider's capability profile instead of an estimate.

The figures compare two builds or two providers on the same machine. They are not CI gates.

## Prerequisites

- Docker, for the NATS server the `memory_worker` bench and the default example start in a container. The `admission` bench needs no Docker.
- A Rust toolchain with `cargo`.
- A clone of the repository. The commands below run from `rsworkspace`.

## Measure the platform overhead

```sh
cargo bench -j 4 -p trogon-sandbox-conformance --bench memory_worker
```

The bench starts NATS in a container and the in-process memory worker on it. Each measurement runs 5 untimed warmups and then 50 timed samples. Command output is measured at 1 KiB, 64 KiB and 512 KiB. Each line prints the p50 and p95 in milliseconds, plus a throughput figure where the measurement moves bytes:

```text
lease create and release round trip      p50 <ms> ms  p95 <ms> ms
command output: cat 1024 bytes           p50 <ms> ms  p95 <ms> ms <rate> MiB/s
```

Run it once on the base commit and once on your change, on the same machine with nothing else heavy running, and compare the lines.

## Measure broker admission

```sh
cargo bench -p trogon-sandbox-broker --bench admission
```

The bench drives the broker's lease handler with in-memory stores and a worker bus that answers at once, so it times only what the broker does between receiving a request and asking a worker for the lease. It has two measurements:

- `grant and delete a lease` admits a lease with a fresh idempotency key and then deletes it. The in-memory quota caps live leases, so the figure covers admission plus release.
- `replay a lease request by idempotency key` repeats a request whose key the broker has already seen, which is the cost of the replay path.

Criterion prints a time range for each measurement and, on a repeat run, how it changed since the previous run.

## Save a baseline and compare against it

Criterion keeps results under `target/criterion` and can name a run so later runs compare against it. This works for the `admission` bench. The `memory_worker` bench prints its own figures and takes no baseline flags, so compare its printed lines by hand.

1. On the base commit, save a baseline named `main`:

   ```sh
   cargo bench -p trogon-sandbox-broker --bench admission -- --save-baseline main
   ```

2. Switch to your change and run against it:

   ```sh
   cargo bench -p trogon-sandbox-broker --bench admission -- --baseline main
   ```

   Each measurement reports `change:` with a percentage range and whether criterion found a significant regression or improvement. A range that spans zero, or the words `No change in performance detected`, means the run does not distinguish the two builds.

3. To keep the new figures as the next reference, save them under their own name:

   ```sh
   cargo bench -p trogon-sandbox-broker --bench admission -- --save-baseline my-change
   ```

Use the same machine for both runs, with nothing else heavy running. Baselines live in `target`, so a `cargo clean` removes them.

## Check that the benches still build

CI does not run the benches, since their figures depend on the machine. It builds them with `cargo bench --no-run` in the `Sandbox benches build` job of the `CI Sandbox Conformance` workflow, so a change that breaks a bench fails the pull request. To run the same check locally from `rsworkspace`:

```sh
cargo bench --locked --no-run --package trogon-sandbox-broker --bench admission --package trogon-sandbox-conformance --bench memory_worker
```

## Time a lease against a worker

With no flags, the example runs the in-process memory worker on a NATS server in a container:

```sh
cargo run -j 4 --release -p trogon-sandbox-conformance --example latency
```

To time a real provider, start its worker against a NATS server first, then point the example at that server:

```sh
cargo run -j 4 --release -p trogon-sandbox-conformance --example latency -- \
  --nats-url nats://127.0.0.1:4222 --provider local_docker --image <image>
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `--nats-url URL` | NATS server the worker is attached to. Without it, the example runs the memory worker in process. | none |
| `--provider NAME` | Provider whose worker serves the leases, as a snake_case name such as `local_docker` or `daytona`. | the memory worker |
| `--image IMAGE` | Image each lease starts from. | none |
| `--iterations N` | Leases to create and time. At least 1. | `10` |
| `--snapshot-registry HOST` | Registry the worker pushes the snapshot to. The snapshot step runs only with this flag. | none |
| `--snapshot-repository REPO` | Repository within that registry. | `latency/snapshots` |

The example prints one table with a row per step:

```text
provider      step        n  p50 ms  p95 ms  note
local_docker  create      10   <ms>    <ms>
local_docker  first exec  10   <ms>    <ms>
```

The steps are `create`, `first exec`, `file write`, `file read`, `snapshot` and `delete`.

## Read the results

- **Create** includes the first lease, which is often cold: a provider pulling or caching an image makes it much slower than the rest. Use enough iterations that the p50 reflects warm creates, and read the p95 for the cold cost.
- **Delete** has no reply, so the example cancels the lease and probes it until the worker stops answering. The figure is when the first unanswered probe was sent, accurate to one probe round trip. Each probe waits up to 2 seconds. A lease the worker still answers after 120 seconds counts as a failed delete.
- **Skipped steps** show `-` and a note. A worker with no exec or file verbs skips those steps and cannot have its delete timed. The snapshot step is skipped without `--snapshot-registry`, or when the worker reports snapshots as unsupported.
- **Capability profiles**: each worker declares a `cold_start_p50_ms` in its capability profile (`src/profile.rs` in the worker crate), and `trogon-sandbox-capability-matrix` lists them side by side. A measured create p50 from this example can replace the declared figure.
