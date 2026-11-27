# Sandbox memory worker

`trogon-sandbox-memory-worker` is an in-memory sandbox provider worker for tests. It
answers the real provider subjects over NATS (`sandbox.provider.mock.*`), so a
broker, dispatcher, or client can be exercised end to end without containers or
virtual machines.

Each lease owns a map-backed filesystem, a scripted command table, an echo
terminal hub, and its own authority scope. Nothing is persisted and no process is
started on the host.

## Main types

- `MemoryWorker` starts the worker on a NATS client, holds the leases it grants,
  exposes its authority surface (`authority_held_leases`, `authority_freeze`,
  `authority_purge`), builds heartbeats through `heartbeat_source`, and closes every
  lease on `shutdown`.
- `MemoryWorkerConfig` selects the provider kind and the `CommandTable`.
- `CommandTable` maps a command name to a `CommandHandler`. The default table
  provides `echo`, `true`, `false`, `pwd`, `printenv`, `cat`, `sleep`, and `serve`;
  tests register their own handlers with `register`. A handler receives a
  `CommandCtx` with the arguments, working directory, environment, file map,
  a cancellation token, and a `LiveOutput` for output that readiness probes can observe.
- `MemoryFs` is the in-memory file map, with path resolution through `resolve`.
- `TerminalHub` and `FrameSink` carry terminal frames; `FrameSink` is implemented
  for the NATS client so frames can be published or captured.
- `MemoryWorkerArgs` and `run` are the command line and the entry point behind the
  `trogon-sandbox-memory-worker` binary. `RunError` reports a failed connection or
  start.
- `build_heartbeat` and `PROFILE` describe the capabilities the worker advertises.

## Behavior notes

- Command output is redacted with the request's redaction values.
- Sleeps are clamped to 86400 seconds.
- Verbs the worker does not model (stdin writes, users and groups, running
  checkpoints, network policy updates, domains) reply with a typed unsupported or
  denied response.
- File watches buffer 4096 events and publish overflow evidence when they exceed
  that.

## Binary

`trogon-sandbox-memory-worker` reads `SANDBOX_NATS_URL`,
`SANDBOX_MAX_CONCURRENT_LEASES`, `SANDBOX_MEMORY_REGIONS`, and
`SANDBOX_DISPATCH_FENCE_FILE`, serves until interrupted, and then closes its leases.

## Fit with the other sandbox crates

The worker speaks the wire types of `trogon-sandbox-contract` and
`trogon-sandbox-protocol`, serves file operations through
`trogon-sandbox-file-ops`, and uses `trogon-nats` for the NATS transfer helpers.
Tests start a throwaway NATS server in a container.
