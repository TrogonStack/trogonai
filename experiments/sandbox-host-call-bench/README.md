# sandbox-host-call-bench

What does crossing the sandbox boundary cost when every guest is Wasm? The same guest runs on three
hosts:

| Host | Boundary |
|---|---|
| wasmtime in-process | Wasm linear memory only |
| wasmtime subprocess | Wasm + an OS process; every host import is a pipe round trip to the parent |
| hyperlight-wasm | Wasm + a KVM/MSHV micro-VM; every host import is a VM exit |

The guest (`guest/`) imports one host function, `HostNoop(i32) -> i32`, and exports `ping(i32)` and
`noop_loop(n)`, which calls the host `n` times. Everything is `i32`, so the numbers measure the
boundary, not marshalling. The harness (`host/`) reports p50/p99/mean for:

- creating a ready guest (instantiate, spawn, or micro-VM build and module load),
- one host→guest call,
- one guest→host call, amortized over 10,000 calls in a loop,
- one host→guest and one guest→host call carrying 1 KiB and 64 KiB (in-process and Hyperlight
  only), the copy a protocol wrapper pays per message,
- for Hyperlight, snapshot and restore-to-clean.

`hluk.sh` and `firecracker.sh` are a second, separate bench: whether stateless Linux tool calls
could leave the microVM tier. They share `lib.sh` (row format, timing loops, a failure becomes a
"failed" row) and run in one CI job, so their rows come from the same runner.

`hluk.sh` runs Linux binaries in a Hyperlight micro-VM through hyperlight-unikraft's `hluk` (first
run, which boots and saves the warm snapshot, then restored from it) against native and `docker run`,
one table per workload. Each hluk sample is a whole `hluk run` process: CLI start, snapshot restore,
running the script, exit.

- Python `print("ok")`, as a `hluk init` project and through `--runtime python` (the two share one
  snapshot, so the latter's "first run" is already a restore).
- 21 standard-library imports (one list in `hluk.sh`): imported per run, and imported once by `warm_exec` before the
  snapshot.
- `import numpy, pandas` on the `agent` image, whose driver imports them before the snapshot;
  native uses a venv. No Docker row: the slim image has no numpy.
- Node.js `console.log("ok")`.
- 8 concurrent Python hello worlds, 5 batches: per-run latency under that load and throughput
  (barrier waits between batches included).

`firecracker.sh` is the tier hluk would replace: Firecracker v1.17.0 with the kernel (6.1.155) and
Ubuntu 24.04 rootfs that `trogon-sandbox-firecracker-worker/scripts/build-guest-rootfs.sh` pins,
1 vCPU and 256 MiB (hluk's python size). Instead of systemd and `sandbox-actor-agent`, the guest's
init is `firecracker/agent.py`, a Python vsock listener, so "ready" means Python is up; the worker's
own boot is slower. The host reaches it over hybrid vsock (`CONNECT 5000`) as the worker does.
`firecracker/drive.py` times, per sample with a new `firecracker` process, a total and its parts:

- cold boot: process start to API socket, five configuration calls and `InstanceStart`, kernel boot
  until the agent accepts on vsock, then a round trip in which the guest runs `python3 /hello.py`;
- restore from a Full snapshot taken after both modes ran once (so the interpreter and python3's
  pages are in guest memory): process start, `PUT /snapshot/load` with resume (memory file mapped,
  not copied), vsock connect, then either `python3 hello.py` as a new guest process or the same code
  `exec`'d in the warm agent interpreter, the closer match to hluk's restored driver.

The Hyperlight sandbox uses the `trogon-sandbox-hyperlight-worker` defaults (256 KiB input buffer,
1.25 MiB heap) and the same pinned `hyperlight-wasm` revision. The guest links with a 64 KiB stack and 128 KiB initial
memory (`guest/build.rs`) so its linear memory fits that heap; a payload row that fails may be
hitting these sizes rather than a Hyperlight limit.

## Running

`./run.sh` on x86_64 Linux. With a readable and writable `/dev/kvm` it also builds the AOT guest and
the Hyperlight rows; without one it prints the two wasmtime hosts only, or fails when
`REQUIRE_HYPERLIGHT=1` (CI sets it). The toolchain comes from `rsworkspace/rust-toolchain.toml` and the
AOT compiler from the revision `host/Cargo.toml` pins. `host/Cargo.lock` was seeded from
`rsworkspace/Cargo.lock`, so shared dependencies resolve as the worker's do. Hyperlight's guest target is
`x86_64-unknown-none`, so arm64 hosts get no Hyperlight rows.

CI: `.github/workflows/experiment-sandbox-host-call-bench.yml` runs it on `ubuntu-latest` (which
exposes KVM) on every push to the `experiments/sandbox-host-call-bench` branch and writes the table
to the job summary. Shared runners are noisy: read orders of magnitude, not single digits.
