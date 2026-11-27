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
- for Hyperlight, snapshot and restore-to-clean.

The Hyperlight sandbox uses the `trogon-sandbox-hyperlight-worker` defaults (256 KiB input buffer,
1.25 MiB heap) and the same pinned `hyperlight-wasm` revision.

## Running

`./run.sh` on x86_64 Linux. With a readable and writable `/dev/kvm` it also builds the AOT guest and
the Hyperlight rows; without one it prints the two wasmtime hosts only. Hyperlight's guest target is
`x86_64-unknown-none`, so arm64 hosts get no Hyperlight rows.

CI: `.github/workflows/experiment-sandbox-host-call-bench.yml` runs it on `ubuntu-latest` (which
exposes KVM) on every push to the `experiments/sandbox-host-call-bench` branch and writes the table
to the job summary. Shared runners are noisy: read orders of magnitude, not single digits.
