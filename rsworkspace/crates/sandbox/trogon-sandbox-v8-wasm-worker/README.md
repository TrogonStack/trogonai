# trogon-sandbox-v8-wasm-worker

A sandbox provider worker that runs untrusted JavaScript in a V8 isolate or WebAssembly components in Wasmtime, in
process and without hardware virtualization.

## What it does

- Serves lease dispatches for the `v8_wasm` provider over NATS: runs the guest named by the lease, reports its
  lifecycle on JetStream, and replies with the grant stamped with the worker id.
- Runs a JS guest (`index.js`) or a Wasm guest (`module.wasm`) from a read-only host mount, with CPU time and memory
  limits and no ambient authority.
- Optionally keeps a JS runtime alive per lease (`V8_WASM_PERSISTENT_RUNTIME`), so later `run_command` calls with
  `["eval", "<source>"]` share its globals until the lease is released.
- Answers every other routed verb of a held lease with its typed "not supported" reply instead of timing out.
- Publishes heartbeats with its capacity and execution profile.

## Main items

| Item | Role |
| --- | --- |
| `V8WasmProvider` | The `Provider` implementation: allocates, runs and cancels leases. |
| `run_worker` | Subscribes to dispatch, cancel and routed verb subjects and serves them until cancelled. |
| `serve` | Connects to NATS, starts heartbeats and calls `run_worker`. |
| `WorkerArgs` | Command line and environment configuration of the binary. |
| `run_js`, `run_wasm`, `JsSession` | The guest runtimes, usable on their own. |
| `RoutedVerb` | The per-lease verbs the worker answers. |

## Running

```sh
SANDBOX_NATS_URL=nats://127.0.0.1:4222 cargo run -p trogon-sandbox-v8-wasm-worker
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `SANDBOX_NATS_URL` | `nats://127.0.0.1:4222` | NATS server to serve from. |
| `SANDBOX_MAX_CONCURRENT_LEASES` | `8` | Capacity advertised in heartbeats. |
| `SANDBOX_DISPATCH_FENCE_FILE` | unset | Journal that keeps the dispatch fence across restarts. |

## Tests

```sh
cargo test -p trogon-sandbox-v8-wasm-worker
```

The NATS tests start an ephemeral JetStream server through `trogon-nats` with the `test-support` feature.
