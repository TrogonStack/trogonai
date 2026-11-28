# Run the Vercel Sandbox worker

`trogon-sandbox-vercel-worker` subscribes to `sandbox.provider.vercel.lease`
and the per-lease verbs under `sandbox.provider.vercel.*`. It provisions each
lease as a Vercel Sandbox microVM.

Broker clients request this provider with `vercel` in `provider_pref`.
`vercel-sandbox` is accepted as an alias.

The worker has no vendor SDK. Every call is plain REST against
`https://vercel.com/api`, authenticated with a bearer token and scoped to a
team by the `teamId` query parameter. Sandboxes are addressed by name and run
in sessions. Commands and files are session endpoints, so the worker keeps the
current session id per lease and replaces it on resume. The token never
appears in `Debug` output or logs.

## Configure the worker

| Variable / flag | Purpose |
|-----------------|---------|
| `SANDBOX_NATS_URL` / `--nats-url` | NATS connection URL. Default `nats://127.0.0.1:4222`. |
| `VERCEL_TOKEN` / `--token` | Vercel access token. Required. Hidden from `--help` and logs. |
| `VERCEL_TEAM_ID` / `--team-id` | Team the sandboxes belong to. Required. |
| `VERCEL_PROJECT_ID` / `--project-id` | Project sandboxes are created under. Required. |
| `SANDBOX_VERCEL_API_URL` / `--api-url` | API base URL. Default `https://vercel.com/api`. |
| `SANDBOX_VERCEL_RUNTIME` / `--runtime` | Runtime used when a lease names no image: `node26`, `node24`, `node22` or `python3.13`. Default `node24`. The worker refuses to start with any other name. |
| `SANDBOX_VERCEL_PLAN` / `--plan` | The team's plan: `hobby` (45 minute sessions, 4 vCPUs), `pro` (24 hours, 8 vCPUs) or `enterprise` (24 hours, 32 vCPUs). Default `pro`. |
| `SANDBOX_VERCEL_MAX_VCPUS` / `--max-vcpus` | Most vCPUs a lease may ask for, lowered to the plan's limit. Default 8. |
| `SANDBOX_VERCEL_HTTP_TIMEOUT_SECS` / `--http-timeout-secs` | Timeout for every request. Default 300. Keep it above foreground command deadlines. An HTTP timeout alone does not confirm that a command stopped. |
| `SANDBOX_VERCEL_REGIONS` / `--regions` | Regions to advertise, comma separated. Default none, so a lease that names a region never reaches this worker. |
| `SANDBOX_MAX_CONCURRENT_LEASES` / `--max-concurrent-leases` | Capacity announced in the heartbeat. Default 4. |
| `SANDBOX_DISPATCH_FENCE_FILE` / `--dispatch-fence-file` | Dispatch fence file. See [Sign worker dispatches](./sign-worker-dispatches.md). |
| `SANDBOX_VERCEL_DISPATCH_LEDGER_FILE` / `--dispatch-ledger-file` | Dispatch ledger. Set it to have the worker take dispatches and cleanups addressed to it alone. See [Targeted dispatch](#targeted-dispatch). |

## Targeted dispatch

With `SANDBOX_VERCEL_DISPATCH_LEDGER_FILE` set, the worker takes its id from the
ledger and announces the `TargetedV1` dispatch protocol. The broker then sends a
Vercel lease to a worker that announces it when one has capacity, and to legacy
workers otherwise, so a fleet can move over one worker at a time.

- Each dispatch attempt is recorded in the ledger. A repeat of an attempt gets
  its first receipt, and an attempt that was cleaned up is refused.
- A cancel becomes a cleanup the worker must confirm. The broker keeps the lease
  as cancelling and retries the cleanup until the worker reports the sandbox
  deleted, so a delete that fails is retried instead of leaking the sandbox.
- A cleanup finds the sandbox by the attempt's receipt, so it still works after
  the worker restarts.
- The worker keeps serving the shared lease subject, so legacy dispatches still
  reach it during a rollout.

Keep the ledger file on storage that survives a restart, and give each worker
its own file.

## How a lease maps to Vercel

| Wire field | Vercel |
|------------|--------|
| `spec.image` | An empty repository uses `SANDBOX_VERCEL_RUNTIME`. An unqualified runtime name without a tag or digest becomes `runtime`. Other references become a registry `image`, preserving the repository, scope, tag and digest, with a 255 byte limit. A Vercel native snapshot becomes `source: {"type": "snapshot", "snapshotId": "..."}`. Native receipts from other providers and OCI snapshot variants are refused. |
| `ttl_seconds` | `timeout` in milliseconds. A TTL past the plan's session limit, or a `no_ttl` lease, is refused with `would_exceed_max_lifetime` before any create call. The heartbeat advertises the limit as `lifecycle.max_lease_lifetime_seconds`, so the broker routes `no_ttl` leases elsewhere. |
| `spec.exposed_ports` | `ports`, at most 15 |
| `spec.env` | `env` |
| `tags` | `tags`. Values longer than 256 characters are refused before create. |
| `spec.vcpu_millis`, `spec.memory_mib` | `resources.vcpus`. See [Machine size](#machine-size). |
| `spec.network_policy` | `networkPolicy`. See [Network policy](#network-policy). |
| `req.region` | `region` |
| `native.vercel.persistent` | `persistent`. Default true. |

Any other key under `native.vercel` is refused before a sandbox is created.
The requested sandbox name remains a broker identity and is not sent as a
vendor name override. The worker adopts the name Vercel returns.

## Images and creation endpoints

Runtime creation calls `POST /v2/sandboxes`. Image creation and native
snapshot restore call `POST /v3/sandboxes`, with `image` or `source`
respectively.

A bare repository such as `my-repository:stable` or
`my-repository@sha256:...` belongs to the authenticated project.
Team-scoped references use `team/project/repository:tag`, optionally prefixed
with `vcr.vercel.com/`. Custom images must already be ready in the Vercel
container registry as optimized `linux/amd64` images. Command capture needs
compatible `sh`, `env`, `timeout -k`, `base64 -w 0`, `head`, `tail -c`, `wc`,
`cat`, `mkdir`, `mv`, `rm`, `sleep` and `kill`, plus a writable absolute
`TMPDIR` or `/tmp`.

The untagged, unqualified names `node26`, `node24`, `node22` and `python3.13`
keep runtime behavior. To select a registry repository whose name matches a
runtime, supply its tag or digest, for example `node24:stable`.

## Machine size

Vercel sizes a sandbox by whole vCPUs and fixes memory at 2048 MiB per vCPU.
The worker translates a request instead of refusing it.

- `vcpu_millis` rounds up to whole vCPUs. 1500 becomes 2.
- `memory_mib` above what the vCPU count carries lifts the count. 4097 MiB
  needs 3 vCPUs.
- When both are named, the larger count wins.
- A request needing more than `SANDBOX_VERCEL_MAX_VCPUS` is refused. The
  profile advertises `vcpu_millis` from 1000 to that ceiling, so admission
  refuses it first.
- `disk_mib` is refused. Vercel has no disk option and the profile advertises
  no disk range.

A lease that names no size gets Vercel's default.

## Pause and resume

Vercel has no memory-preserving pause. The worker maps pause to stopping the
current session and resume to fetching the sandbox with resume enabled, which
starts a new session. With a persistent sandbox (the default) files survive
and processes do not.

A lease created with `native.vercel.persistent: false` refuses pause, because
a stop would lose its files. Pausing an already stopped sandbox is treated as
done. Routes change when a new session starts, and the worker adopts the new
ones.

## Native snapshots

A checkpoint that accepts provider-native receipts calls
`POST /v3/sandboxes/sessions/{id}/snapshot` and returns the Vercel snapshot id,
size and expiry when the API supplies them. Restore calls `POST /v3/sandboxes`
with that id in `source.snapshotId`. OCI snapshot export and OCI restore are
unsupported. The profile advertises `supports_native_snapshots: true` while
`can_snapshot`, `can_checkpoint_running` and `can_restore` stay false.

Taking a snapshot ends the current Vercel session. The worker serializes
capture and session replacement per lease, then resumes the sandbox and adopts
its new session and routes. A successful snapshot receipt is returned even if
resume fails, and the next operation or wake retries resume. If that retry also
fails, operations can receive a vendor error against the old session.

## Run the hosted tests

Three test binaries run against a real Vercel team. They are ignored by default,
cost money, and never run in CI. Start them with `--ignored` and the variables below.

| Test binary | Variables it needs |
|-------------|--------------------|
| `hosted_conformance` | `SANDBOX_VERCEL_HOSTED_TEST=1`, `VERCEL_TOKEN`, `VERCEL_TEAM_ID`, `VERCEL_PROJECT_ID`, `SANDBOX_CONFORMANCE_NATS_URL` and a running worker of the same team on that NATS server. Optional: `SANDBOX_CONFORMANCE_WORKER_ID`, `SANDBOX_CONFORMANCE_IMAGE`, `SANDBOX_VERCEL_API_URL`. |
| `hosted_egress_diagnostic` | `SANDBOX_VERCEL_HOSTED_TEST=1`, `VERCEL_TOKEN`, `VERCEL_TEAM_ID`, `VERCEL_PROJECT_ID`. It reports what the allow-all and deny-all policies do and does not judge conformance. |
| `hosted_image_contract` | The same as the diagnostic, plus `SANDBOX_VERCEL_PAID_TEST=1` because a snapshot is stored until the test retires it. The custom image test also needs `SANDBOX_VERCEL_CUSTOM_IMAGE`, a Ready image in the Vercel registry. |

```sh
cd rsworkspace
SANDBOX_VERCEL_HOSTED_TEST=1 cargo test -p trogon-sandbox-vercel-worker --test hosted_conformance -- --ignored
```

Each lane removes what it created and fails if it cannot prove a sandbox or
snapshot is gone. `hosted_snapshot_retirement` tests the retirement check itself
against a loopback server, so it runs with the normal suite.

## Network policy

| Policy | Vercel |
|--------|--------|
| unset, allow all | Vercel default (open egress) |
| deny all, or a user defined policy with nothing allowed | `{"mode": "deny-all"}` |
| user defined with hosts or ranges | `allow` lists the domains, `subnets.allow` and `subnets.deny` list the ranges |

The worker refuses these with `InvalidNetworkPolicy`: denied domains, request
rules, a policy that refuses opaque tunnels, any rule that is not pass-through,
and any rule that names a port. A user defined policy refuses opaque tunnels
unless it sets `"opaque_tunnels": "allow"`, so one sent to this worker has to
say so. Changing the policy of a running lease is refused. The worker applies it
at create time only.

## Commands, files and ports

- Commands retain binary stdout and stderr in an exclusive guest capture
  directory and transport bounded head and tail bytes through base64 frames.
  The original `max_output_bytes` budget applies when decoding, with
  `truncated` set when bytes are discarded. `stream_output` is unsupported.
- Completed detached commands retain a bounded provider log receipt rather than
  their full output in worker memory. Polls decode the same source under the
  original cap and exit cause. Recovery does not rerun the user command.
- Finite commands send a native timeout in milliseconds, including detached
  commands. A foreground local deadline also attempts SIGKILL when no terminal
  status has arrived. The worker reports `timed_out` only after an accepted
  deadline kill and a native exit 137 receipt. An unknown command id, a failed
  kill or a missing terminal receipt is a backend error with an uncertain
  outcome. An ordinary native exit such as 124 or 137 alone is not marked timed
  out.
- A detached command starts without waiting and is polled and killed by its
  command id. Signals are sent by number. An explicit lease deadline is
  supported only for detached commands and omits the finite native timeout.
- A command runs as the sandbox user. `run_as_user` of `root` uses sudo. Any
  other user is refused.
- Relative `cwd` values resolve against `/vercel/sandbox`.
- Files are written as a gzip compressed tar and read as raw bytes. Writes and
  reads as another user are refused.
- Ports must be declared when the sandbox is created. A port that was not
  declared answers `PortNotExposed`. Each declared port has a public route that
  the worker reports as its URL.

## Limits

- No OCI snapshot export or restore. Use the native snapshot receipts above.
- No idle timeout, terminals, file watching or change tracking.
- No interactive stdin, named processes, sandbox users or groups, or file
  operations beyond read and write. Each returns a typed unsupported or denied
  outcome.
- No remote mounts. Vercel cannot bind a host path into a microVM.

## Test the worker

```sh
cargo test -p trogon-sandbox-vercel-worker
```

The default suite uses HTTP fixtures and local command processes. Hosted test
cases are ignored by default.

## Related

- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md)
  for the declared capabilities next to every other worker
- [Run the E2B worker](./e2b-worker.md)
