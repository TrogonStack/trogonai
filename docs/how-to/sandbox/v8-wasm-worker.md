# Run the V8 and Wasm worker

`trogon-sandbox-v8-wasm-worker` runs a lease's guest in process. A JavaScript
guest (`index.js`) runs in a V8 isolate and a Wasm guest (`module.wasm`) runs
in wasmtime. By default a lease runs its guest once and reclaims the runtime.
Per-lease verbs such as `run_command` reply with a typed unsupported error.

## Configure the worker

| Variable / flag | Purpose |
|-----------------|---------|
| `SANDBOX_NATS_URL` / `--nats-url` | NATS connection URL. Default `nats://127.0.0.1:4222`. |
| `SANDBOX_MAX_CONCURRENT_LEASES` / `--max-concurrent-leases` | Capacity announced in the heartbeat. Default 8. |
| `SANDBOX_DISPATCH_FENCE_FILE` / `--dispatch-fence-file` | Dispatch fence file. See [Sign worker dispatches](./sign-worker-dispatches.md). |

## Keep a JavaScript runtime alive

Set `V8_WASM_PERSISTENT_RUNTIME=true` (or `1`) in the lease `spec.env` to keep
the JavaScript runtime alive after `index.js` finishes. `false` and `0` mean
the same as leaving it unset; any other value refuses the lease. The broker reads that
key as the `needs_persistent_runtime` requirement, so only a worker that
declares `supports_persistent_runtime` is given such a lease. Every other
worker is refused before it is asked.

- `run_command` with `command: ["eval", "<js source>"]` evaluates the source in
  the same runtime, so globals set by `index.js` and by earlier commands persist
  until the lease is released.
- stdout and stderr are separate in the result. A thrown error exits 1 with the
  error text on stderr and the runtime stays usable. A timeout exits -1 with
  `timed_out` set and the runtime stays usable.
- The heap is bounded by `spec.memory_mib`. Hitting the limit ends the running
  script and retires the runtime. Later commands then report a failure instead
  of crashing the worker.
- Releasing the lease drops the runtime. One worker holds at most 32
  persistent runtimes, and further opt-in leases fail.
- Only JavaScript guests support this. `cwd`, `env`, `run_as_user`, detached,
  streamed and lease-deadline commands are refused with an invalid-spec reply.
- Persistent runtime is not a declared profile flag for admission, so clients
  opt in through the environment variable.
