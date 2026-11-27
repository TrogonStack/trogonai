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
  `ExecuteReply`, `GitCloneRequest`, `SnapshotRecord` and `SnapshotState` are the
  shapes it sends and returns.
- `DaytonaExec` runs commands and moves files in a lease's sandbox. `LeaseSandbox`
  names the sandbox a lease runs in. `OrdinaryCommandRuntime` is the pinned command
  supervisor that `DaytonaExec::with_runtime` installs for commands that take
  signals.
- `DaytonaWorkerProvider` implements `trogon_sandbox_protocol::Provider`. It
  creates sandboxes, waits for them to settle, runs commands, reads and writes
  files, pauses and resumes sandboxes, takes native snapshots and deletes what it
  created when a lease is cancelled. `Wait` bounds how long it waits for a sandbox
  state and `SnapshotWait` how long it waits for a snapshot.
- `SnapshotCustody` records every native snapshot the worker creates in a
  JetStream key-value bucket, so a snapshot left behind by a crash or a timeout is
  found and removed later.
- `DaytonaSshGrant` is a short-lived SSH token for a sandbox. `DaytonaGatewayAccess`
  mints one, and `RusshGatewayShell` opens a shell with it against a pinned host
  key. `DaytonaTerminals` serves interactive terminals over that shell.
- `S3ExportTarget` names a bucket, prefix, region, endpoint and optional KMS key.
  `export_workspace_to_s3` packs a sandbox workspace and uploads it there.
- `NatsLifecycleEmitter` publishes lease lifecycle events to JetStream.
- `run` serves a provider on NATS: lease dispatch on a queue group, cancellation,
  the per-lease verbs, the authority check and the snapshot export subject.
  `launch` wires the pieces the binary needs, including heartbeats, terminals and
  lifecycle events.

## Behavior

- A lease starts from a named Daytona snapshot, the worker's default unless the
  lease names an image. A lease with a git workspace is cloned into `/workspace`;
  an empty workspace creates that folder. Remote mounts, privileged execution and
  explicit vCPU or disk sizes are refused, because a snapshot fixes its own size.
  Tags are stored as `trogon.tag.{key}` labels next to `trogon.lease_id`, and the
  native handle is `{api_base_url}/sandbox/{id}`.
- A command runs in a Daytona session inside a capture script, so each stream
  keeps its head and tail when a cap is reached. A deadline stops the command with
  `timeout`, and a receipt tells a deadline apart from the command's own exit.
  Stdin is fed through a pipe in order, one write at a time, until it is closed. A
  detached command keeps running until it finishes or is killed, and its result is
  kept for ten minutes after it ends, up to 256 results. A detached command does
  not survive a pause.
- Killing or signalling a detached command needs the command supervisor. When the
  worker is given a supervisor binary and its SHA-256, it checks the pin and the
  binary format, installs it in the sandbox and runs detached commands through it.
  Without it, a kill is denied.
- The native options are `auto_stop_interval`, `auto_archive_interval`,
  `auto_delete_interval`, `public`, `class` (`small`, `medium` or `large`) and
  `gpu` (1 to 8). Any other key is refused.
- Pause and disk-level suspend stop the sandbox, and resume and wake start it
  again. A stopped sandbox keeps its files but not its processes. A running
  checkpoint with a native name takes a Daytona snapshot; without one it is not
  offered. Exporting a snapshot as an OCI artifact is not offered.
- Cancel removes the lease's native snapshots that were not kept before it deletes
  the sandbox. Purge also removes the kept ones. A snapshot is treated as gone only
  when Daytona answers that it does not exist.
- The `ssh_access` verb returns an SSH token for the lease's sandbox, and the SSH
  stream bridge and terminals use the same access.
- Only an open network policy, or none, is admitted. A policy that blocks or
  limits egress is refused before any sandbox exists, because the worker cannot
  verify that Daytona enforces it. A policy cannot be changed after the sandbox
  exists.

## Fit

The wire types come from `trogon-sandbox-contract`. Dispatch verification, the
redispatch guard, the authority guard, heartbeats, large-payload transfer and the
guest capture protocol come from `trogon-sandbox-protocol`. The terminal hub, the
SSH stream bridge and the pinned host key come from `trogon-sandbox-ssh-gateway`.
Uploads to S3 go through `trogon-sandbox-volumes`. Tracing and metrics come from
`trogon-sandbox-observability`.

## Running

The binary reads its settings from flags or environment variables. The API key is
read from `DAYTONA_API_KEY`, the organization from `DAYTONA_ORGANIZATION_ID` and
the endpoint from `DAYTONA_BASE_URL`. `--regions` (`SANDBOX_DAYTONA_REGIONS`)
lists the regions the heartbeat advertises.
`--snapshot-poll-interval-secs` (`SANDBOX_DAYTONA_SNAPSHOT_POLL_INTERVAL_SECS`,
default 2) and `--snapshot-timeout-secs` (`SANDBOX_DAYTONA_SNAPSHOT_TIMEOUT_SECS`,
default 300) bound the wait for a native snapshot. `--command-supervisor`
(`SANDBOX_DAYTONA_COMMAND_SUPERVISOR`) and `--command-supervisor-sha256`
(`SANDBOX_DAYTONA_COMMAND_SUPERVISOR_SHA256`) are given together or not at all.
An S3 export reads its KMS key from `SANDBOX_S3_KMS_KEY_ID` when one is asked for.
The worker stops on interrupt.

## Hosted tests

The `hosted_conformance`, `hosted_contract` and `hosted_network_policy` tests run
against a real Daytona account. They are skipped by default and run with
`SANDBOX_DAYTONA_HOSTED_TEST=1` and `--ignored`, for example
`cargo test -p trogon-sandbox-daytona-worker --test hosted_contract -- --ignored`.
They read `DAYTONA_API_KEY`, `DAYTONA_ORGANIZATION_ID`, `SANDBOX_DAYTONA_SNAPSHOT`
and `SANDBOX_CONFORMANCE_NATS_URL`, and optionally `DAYTONA_BASE_URL` and
`SANDBOX_CONFORMANCE_WORKER_ID`. The snapshot timeout lane needs a JetStream of
its own.
