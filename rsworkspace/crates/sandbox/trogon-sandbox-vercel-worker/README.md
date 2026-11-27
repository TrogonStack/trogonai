# Sandbox Vercel worker

`trogon-sandbox-vercel-worker` is a sandbox worker for the Vercel Sandbox platform.
It leases one Vercel Sandbox microVM per lease, answers lease operations over NATS,
and stops what it created when a lease ends. The `trogon-sandbox-vercel-worker`
binary runs it.

## Types

- `VercelConfig` holds where the worker reaches Vercel and as whom, built from
  `VercelEndpoint`, `VercelTeamId`, `VercelProjectId` and `VercelToken`. Each one is
  checked when it is parsed. An endpoint must use HTTPS unless its host is
  loopback, and the token is never part of `Debug`.
- `VercelApi` is the REST client. It never follows redirects, so the bearer token
  cannot leave the configured endpoint. `SandboxRecord`, `SessionRecord`,
  `SnapshotRecord`, `SandboxAndSession` and `NewSandbox` are the shapes it sends and
  returns.
- `VercelExec` runs commands and moves files in a lease's sandbox session.
  `LeaseSandbox` names the sandbox and session a lease runs in.
- `VercelWorkerProvider` implements `trogon_sandbox_protocol::Provider`. It creates
  sandboxes, runs commands, reads and writes files, pauses and resumes sandboxes,
  takes native snapshots and stops sandboxes when a lease is cancelled.
- `VercelPlan` is the Vercel plan the team is on. It caps how long a lease may run
  and how many vCPUs it may ask for.
- `run` serves a provider on NATS: lease dispatch on a queue group, cancellation,
  the per-lease verbs, heartbeats and the authority check. `launch` wires the
  pieces the binary needs.

## Behavior

- A lease may name a runtime (`node26`, `node24`, `node22` or `python3.13`), an
  image reference that names one of them, or a Vercel native snapshot. Remote
  mounts and privileged execution are refused.
- A command runs in the sandbox's own capture script, so its output keeps the head
  and tail of each stream when a cap is reached, and a deadline kills the command.
- The `persistent` native option keeps the sandbox files across pause and resume.
- Pause and disk-level suspend stop a persistent sandbox, and resume and wake start it again. Memory-level suspend
  and freeze are not offered. Exporting a snapshot as an OCI artifact is not offered either, because Vercel owns its
  snapshots.
- Network policies are translated to the Vercel network policy shape, and ones
  Vercel cannot express are refused.

## Fit

The wire types come from `trogon-sandbox-contract`. Dispatch verification, the
redispatch guard, the authority guard, heartbeats, large-payload transfer and the
guest capture protocol come from `trogon-sandbox-protocol`. Tracing and metrics come
from `trogon-sandbox-observability`.

## Running

The binary reads its settings from flags or environment variables. The access
token is read from `VERCEL_TOKEN`, and the team and project from `VERCEL_TEAM_ID` and
`VERCEL_PROJECT_ID`. It stops on interrupt.
