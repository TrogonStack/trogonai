# Measure sandbox latency

This guide measures how long the sandbox platform takes to serve a lease. Two tools in `trogon-sandbox-conformance` cover different questions:

- The `memory_worker` bench measures the platform's own overhead: NATS round trips, JSON encoding and chunked transfer, with no container or VM behind the lease.
- The `latency` example times each step of a lease's life against a real worker: create, first exec, file write, file read, snapshot and delete.

Neither includes broker admission. Both report p50 and p95.

## When to use this

- Before and after a change to the worker protocol, lease handling or chunked transfer, to see whether it moved the numbers.
- When comparing providers on the same machine.
- When you need measured create times for a provider's capability profile instead of an estimate.

The figures compare two builds or two providers on the same machine. They are not CI gates.

## Prerequisites

- Docker, for the NATS server the bench and the default example start in a container.
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
