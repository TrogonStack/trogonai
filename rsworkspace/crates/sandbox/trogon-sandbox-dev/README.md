# Sandbox dev

`trogon-sandbox-dev` starts a complete local sandbox stack with one command and keeps it
running under one supervisor. It is for development and tests, not for production.

The stack is four processes:

- `nats-server` with JetStream, on `--nats-port`.
- The broker, `trogon-sandbox-broker`, with in-memory quota and idempotency backends.
- One worker: `trogon-sandbox-docker-worker`, `trogon-sandbox-subprocess-worker`, or
  `trogon-sandbox-memory-worker`. `--worker auto` picks docker when the daemon answers,
  then the subprocess worker when `--agent-binary` is given on Linux or macOS, then the
  memory worker.
- The websocket gateway, `trogon-sandbox-ws-gateway`, on `--gateway-listen`.

Binaries are searched in `--bin-dir`, then next to this executable, then on `PATH`.

## Types

- `DevArgs` holds the command-line flags, already parsed: the tenant is a `TenantId`, the
  API key is secret and is checked as a `BearerToken` before anything starts.
  `DevArgs::plan` turns the flags into a `DevPlan` without starting anything, so the
  wiring is testable. Each plan mints a fresh dispatch signing key: the broker signs, and
  the worker accepts only the matching public key.
- `DevPlan` and `ProcessSpec` describe what to spawn, with which arguments and
  environment. `PlanError` says which binary is missing or which flag is required.
- `WorkerKind` selects the worker. `WorkerKind::resolve` is pure; the caller probes docker
  with `docker_available`.
- `run` starts the plan, waits until nats-server and the gateway accept connections, and
  until the broker answers a tenant limits request. It then calls the readiness callback
  with a `DevReady` and supervises until the shutdown future resolves or one process exits.
  Every process is stopped before it returns. `DevError` covers each startup step.

## API key handling

The key is never printed. A supplied `--api-key` (or `SANDBOX_DEV_API_KEY`) is hidden from
`--help`; a generated one is `sbx-dev-` followed by 32 hexadecimal characters. Either is
registered for the tenant in the key-value bucket the gateway reads, and written to a new
owner-only file (`--api-key-file`, default a private temporary file). An existing file or
symlink at that path is refused, never overwritten. The banner names the file only.

## Platform notes

Children run in their own process group and are killed when the supervisor drops them. On
Linux each child also receives SIGTERM when the supervisor dies, so a killed supervisor does
not leave the stack behind; that path is only built and tested on Linux.

## Related crates

The broker request and response types come from `trogon-sandbox-contract`, the dispatch
signing key and subjects from `trogon-sandbox-protocol`, and the key store from
`trogon-sandbox-auth`.
