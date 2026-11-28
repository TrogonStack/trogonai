# Run the E2B worker

`trogon-sandbox-e2b-worker` subscribes to `sandbox.provider.e2b.lease` and the
per-lease verbs under `sandbox.provider.e2b.*`. It provisions each lease as an
E2B microVM sandbox.

Broker clients request this provider with `e2b` in `provider_pref`. `e2b-dev`
is accepted as an alias.

The worker implements commands, general file operations, terminals, file
watches, change tracking and provider-native memory snapshots. See the
[E2B runtime reference](../../reference/sandbox/e2b-runtime.md) for snapshot
authority, storage permissions, file limits and verification boundaries.

## Prepare a worker

1. Provide `E2B_API_KEY` through the worker's secret configuration and select an
   E2B template that has the required guest tools.
1. Set `SANDBOX_NATS_URL` to the broker's NATS account. Native snapshots and
   lease cleanup need durable JetStream.
1. Start the worker binary with the settings below.
1. Request an `e2b` lease through the broker. Verify the account's create,
   command, file, preview and native-memory flows before treating the deployment
   as accepted.

When upgrading a replicated fleet, drain every worker that still uses a
broadcast lease subscription before dispatching new leases. Updated workers
share the `sandbox-provider-e2b` queue group for create intake, so one dispatch
reaches one updated replica. An older broadcast subscriber receives the same
dispatch independently of the queue and can allocate a duplicate sandbox.
Per-lease commands stay broadcast so the worker holding the lease can answer.

## How the worker talks to E2B

There is no vendor SDK. The control plane is plain REST (create, snapshot,
delete, pause and connect) authenticated with the `X-API-Key` header. Commands
and files go to the `envd` daemon inside each sandbox over its Connect-protocol
HTTP API. Credential headers are marked sensitive and configuration debug
output is redacted.

## Configure the worker

| Variable / flag | Purpose |
|-----------------|---------|
| `SANDBOX_NATS_URL` / `--nats-url` | NATS connection URL. Default `nats://127.0.0.1:4222`. |
| `E2B_API_KEY` / `--api-key` | E2B API key. Required. Hidden from `--help` and logs. |
| `E2B_API_URL` / `--api-url` | Control plane URL. |
| `E2B_DOMAIN` / `--domain` | Domain sandboxes are served under. |
| `SANDBOX_E2B_ENVD_URL` / `--envd-url` | Overrides the sandbox `envd` address for a self-hosted endpoint. |
| `SANDBOX_E2B_TEMPLATE` / `--template` | Template used when a lease names no image. Default `base`. |
| `SANDBOX_E2B_HTTP_TIMEOUT_SECS` / `--http-timeout-secs` | Default timeout for E2B transport requests. Command execution and bounded helper requests set their own deadlines. Default 300. |
| `SANDBOX_E2B_REGIONS` / `--regions` | Regions to advertise, comma separated. Default none, so a lease that names a region never reaches this worker. |
| `SANDBOX_MAX_CONCURRENT_LEASES` / `--max-concurrent-leases` | Capacity announced in the heartbeat. Default 4. |
| `SANDBOX_E2B_DISPATCH_LEDGER_FILE` / `--dispatch-ledger-file` | Retains the worker identity and exact assignment receipts across restarts. |
| `SANDBOX_DISPATCH_FENCE_FILE` / `--dispatch-fence-file` | Journal that keeps dispatch ownership fences across worker restarts. Unset keeps them in memory. See [Sign worker dispatches](./sign-worker-dispatches.md). |

For native snapshots, enable durable JetStream storage on `SANDBOX_NATS_URL`
and grant the worker access to these KV buckets:

| Bucket | Holds |
|--------|-------|
| `sandbox-e2b-snapshot-executions` | Native snapshot manifests |
| `sandbox-e2b-snapshot-custody` | Cleanup custody for captured snapshots |
| `sandbox-e2b-lease-cleanup` | The lease and sandbox cleanup journal |
| `sandbox-e2b-assignments` | Assignment receipts |
| `sandbox-e2b-command-custody` | Command execution custody |

Keep these buckets without TTL or manual record deletion, back them up, and
keep their records and the NATS credentials outside guest filesystems.

Use a Linux template with `bash`, `setsid`, GNU coreutils and `tar`, which the
derived file operations and detached process helpers need. The worker returns an
error when an image lacks a required tool.

## How a lease maps to E2B

| Wire field | E2B |
|------------|-----|
| `spec.image` | Template name from a registry reference, or an owned native snapshot reference. An empty repository uses `SANDBOX_E2B_TEMPLATE`. OCI snapshot and tag inputs are unsupported. |
| `ttl_seconds` | `timeout`, clamped to 24 hours. E2B enforces the plan limit itself. |
| `spec.env` | `envVars` |
| `tags` | `metadata` |
| `spec.network_policy` | `allow_internet_access` and `network.allowOut` / `network.denyOut`. See [Network policy](#network-policy). |
| `native.e2b.auto_pause` | `autoPause` |

Any other key under `native.e2b` is refused before a sandbox is created.

## Machine size

E2B fixes CPU, memory and disk in the template. A lease cannot choose them, so
the profile advertises no size range and a request that names `vcpu_millis` or
`disk_mib` is refused at admission. To get a different size, build a template
with that size and name it as the image.

## Pause and resume

Pause maps to E2B's memory-preserving pause, so running processes and files both
survive. Resume maps to the connect call, which also refreshes the sandbox's
access token. Pausing an already paused sandbox is treated as done.

## Capture and restore a native snapshot

Request a provider-native capture through the broker with `accept_native`
(`acceptNative` in the TypeScript client). Restore with the returned native
snapshot reference as the lease image, using the same tenant and the E2B
provider.

Every capture requests a fresh, unnamed provider snapshot with memory. The
`native_name` in the worker protocol is a logical label stored in the host
manifest. It does not assign or reuse an E2B template name. Retain the returned opaque reference
together with its host metadata.

The worker reconnects the source sandbox and refreshes its credentials before
returning a successful receipt. A capture serializes detached launches on its
source lease while other leases keep running commands. Restore validates the
captured process identities before adopting them, and the original execution
ids are retained under the new lease, so source and fork handles stay
independent.

Retire a returned native reference through the client's native snapshot delete
operation. The broker records tenant-owned deletion intent before dispatch,
which blocks new restores while cleanup retries. Existing forks continue
independently. Exact absence, not a successful delete response alone, clears
custody.

Deploy the worker, broker and gateway together before using native retirement.

## Network policy

| Policy | E2B |
|--------|-----|
| unset, allow all | E2B default (open egress) |
| deny all, or a user defined policy with nothing allowed | `allow_internet_access: false` |
| user defined with hosts or ranges | `allowOut` lists them and `denyOut` is `0.0.0.0/0` plus the deny ranges |

The worker refuses these with `InvalidNetworkPolicy`: denied domains, request
rules, a policy that refuses opaque tunnels, any rule that is not pass-through,
and any rule that names a port. A user defined policy refuses opaque tunnels
unless it sets `"opaque_tunnels": "allow"`, so one sent to this worker has to
say so. Changing the policy of a running lease is refused, because E2B takes it
at create time.

Blocked TCP connections can appear open without delivering application traffic.
A successful `connect()` or `nc -z` alone does not show a deny-all violation.
Probe with an authenticated TLS handshake to a literal destination instead.

## Commands, files and ports

- A foreground command uses the same owned finite execution as a detached
  command, then waits for capped output and confirmed native group completion.
  A request cancellation or a deadline trailer alone cannot become a completed
  timeout result.
- A detached launcher records the wrapper's PID and Linux start ticks before
  releasing the command, then records output and exit status under an execution
  directory. Finite runs install a guest watchdog. An explicit
  `deadline: "lease"` omits the finite timer. The template must provide `bash`,
  `setsid`, `nohup`, `sleep`, `mkdir`, `mv`, `rm` and readable Linux `/proc`
  process metadata. Missing prerequisites fail before the target command is
  released.
- Kill checks the host-recorded PID and start pair before signalling the
  process group. Signal delivery is separate from completion, so a trapped or
  ignored signal leaves the command running. Deleting a sandbox ends its
  commands.
- Commands run as the sandbox's `user` account unless the request names another
  user.
- General file operations include metadata, recursive listings, ranged binary
  reads, chunk writes, exact edits, search, path mutations, plain tar transfers
  and ranged SHA-256 checksums.
- Any port of a running sandbox is served at `https://<port>-<sandbox>.<domain>`,
  so ports need not be declared at create.

The worker advertises `command_cancellation`. It does not advertise
`command_launch_cancellation`, so a client that needs launch cancellation is
not routed to it.

## Terminals

The worker serves terminals and the SSH stream bridge on the shared terminal
hub. Each terminal is one `process.Process/Start` call on
`envd` with a PTY. Input is sent with `SendInput`, a resize with `Update` and
close with `SendSignal`.

## File watching

The profile declares `supports_file_watch`. The worker answers `watch_start`,
`watch_poll` and `watch_stop` with the same contract as the Docker worker: a
sequence cursor, a parked poll and an `overflowed` flag. Watch cursors stay in
worker memory, so start a new watch on a restored fork.

Change tracking (`checkpoint`, `changes`) uses a bounded host-owned baseline and
content fingerprints.

## Limits

- No OCI snapshot export or restore and no in-place rollback. Native memory
  snapshots use provider references.
- No interactive command stdin, named processes, or sandbox users and groups.
  Each returns a typed unsupported or denied outcome. PTY terminals remain
  available.
- No remote mounts. E2B cannot bind a host path into a microVM.

## Test the worker

```sh
cargo test -p trogon-sandbox-e2b-worker
```

The default suite uses local provider protocol fixtures, owned filesystem
helpers and throwaway NATS servers. Hosted tests need a real `E2B_API_KEY` and
`nats-server` on `PATH`, and run only when explicitly enabled.

### Run the live suites

The live suites create real sandboxes, so each is ignored and opts in with its own
variable. Each deletes the sandboxes it creates and proves they are gone.

| Test binary | Opt-in variable | What it needs |
| --- | --- | --- |
| `hosted_conformance` | `SANDBOX_E2B_HOSTED_TEST=1` | `E2B_API_KEY`, a same-account worker on `SANDBOX_CONFORMANCE_NATS_URL`, and `SANDBOX_CONFORMANCE_WORKER_ID`. `SANDBOX_CONFORMANCE_IMAGE` defaults to `base`. |
| `hosted_live_provider` | `SANDBOX_E2B_HOSTED_TEST=1` | `E2B_API_KEY`. |
| `hosted_file_events` | `SANDBOX_E2B_HOSTED_TEST=1` | `E2B_API_KEY`. |
| `hosted_native_snapshot` | `SANDBOX_E2B_HOSTED_SNAPSHOT_TEST=1` | `E2B_API_KEY` and `nats-server` on `PATH`. It serves its own worker and creates one persistent snapshot. |
| `hosted_egress_diagnostic` | `SANDBOX_E2B_EGRESS_DIAGNOSTIC_TEST=1` | `E2B_API_KEY`. |

`SANDBOX_E2B_TEMPLATE` selects the template the suites boot, `base` by default, and
`E2B_API_URL` and `E2B_DOMAIN` point them at a self-hosted endpoint. Run one suite with:

```sh
SANDBOX_E2B_HOSTED_TEST=1 cargo test -p trogon-sandbox-e2b-worker --test hosted_file_events -- --ignored
```

## Related

- [E2B runtime reference](../../reference/sandbox/e2b-runtime.md)
- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md)
- [Run the Vercel Sandbox worker](./vercel-worker.md)
