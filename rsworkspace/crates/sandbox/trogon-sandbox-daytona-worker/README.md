# Sandbox Daytona worker

`trogon-sandbox-daytona-worker` is a sandbox worker for Daytona. It leases one
Daytona sandbox per lease, answers lease operations over NATS, and deletes what it
created when a lease ends. The `trogon-sandbox-daytona-worker` binary runs it.

## Types

- `DaytonaConfig` holds where the worker reaches Daytona and as whom, built from
  `DaytonaEndpoint`, `DaytonaApiKey` and an optional `DaytonaOrganizationId`. Each
  one is checked when it is parsed. An endpoint must use HTTPS unless its host is
  loopback, and the key is never part of `Debug`.
- `DaytonaApi` is the REST client for the control plane and the toolbox. It never
  follows redirects, so the key cannot leave the configured endpoint.
  `SandboxRecord`, `SandboxState`, `NewSandbox`, `PortPreview`, `ExecuteRequest`,
  `ExecuteReply` and `GitCloneRequest` are the shapes it sends and returns.
- `DaytonaExec` runs commands and moves files in a lease's sandbox. `LeaseSandbox`
  names the sandbox a lease runs in.
- `DaytonaWorkerProvider` implements `trogon_sandbox_protocol::Provider`. It
  creates sandboxes, waits for them to settle, runs commands, reads and writes
  files, pauses and resumes sandboxes and deletes them when a lease is cancelled.
  `Wait` bounds how long it waits for a sandbox state.
- `run` serves a provider on NATS: lease dispatch on a queue group, cancellation,
  the per-lease verbs, the authority check and the snapshot export subject.
  `launch` wires the pieces the binary needs, including heartbeats.

## Behavior

- A lease starts from a named Daytona snapshot, the worker's default unless the
  lease names an image. A lease with a git workspace is cloned into `/workspace`;
  an empty workspace creates that folder. Remote mounts, privileged execution and
  explicit vCPU or disk sizes are refused, because a snapshot fixes its own size.
- A command runs inside a capture script, so each stream keeps its head and tail
  when a cap is reached, and a deadline stops the command. A detached command
  keeps running until it finishes or is killed, and its result is kept for a
  while after it is read. A detached command does not survive a pause.
- The native options are `auto_stop_interval`, `auto_archive_interval`,
  `auto_delete_interval` and `public`. Any other key is refused.
- Pause and disk-level suspend stop the sandbox, and resume and wake start it
  again. A stopped sandbox keeps its files but not its processes. Interactive
  stdin, native snapshots and exporting a snapshot as an OCI artifact are not
  offered.
- Network policies are translated to Daytona's block-all and CIDR allow list.
  Policies Daytona cannot express are refused, and a policy cannot be changed
  after the sandbox exists.

## Fit

The wire types come from `trogon-sandbox-contract`. Dispatch verification, the
redispatch guard, the authority guard, heartbeats, large-payload transfer and the
guest capture protocol come from `trogon-sandbox-protocol`. Tracing and metrics
come from `trogon-sandbox-observability`.

## Running

The binary reads its settings from flags or environment variables. The API key is
read from `DAYTONA_API_KEY`, the organization from `DAYTONA_ORGANIZATION_ID` and
the endpoint from `DAYTONA_BASE_URL`. It stops on interrupt.
