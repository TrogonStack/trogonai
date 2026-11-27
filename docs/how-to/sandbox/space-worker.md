# Run the SPACE worker

`trogon-sandbox-space-worker` subscribes to `sandbox.provider.space.*.*`, plus
its own targeted dispatch and cleanup subjects. It provisions each lease as a
microVM on a SPACE install through SPACE's native HTTP API.

Broker clients request this provider with `space` in `provider_pref`.
`perplexity-space` is accepted as an alias.

Validate the first deployment against a non-production SPACE install. The
worker's tests run against an in-process fake of the control plane and the data
plane.

## How the worker talks to SPACE

SPACE has two planes with two kinds of token, and the worker keeps them apart.

- **Control plane.** Plain REST under `/v1`, authenticated with a bearer
  management token. Creating, pausing, suspending, waking and deleting a
  sandbox are asynchronous. SPACE answers `202` with an operation id, and the
  worker polls `GET /v1/operations/{id}` until it is terminal. An accepted
  request is never treated as readiness.
- **Data plane.** A command's output is a Server-Sent Events stream on the
  data-plane origin. The create-command reply carries a session scoped attach
  token and an `events_url`. The worker opens that URL with the attach token
  only. Attaching is what starts the process. The management token is never
  sent to the data plane, and the attach token is never sent to the control
  plane.

The worker refuses an `events_url` that does not resolve to the configured
data-plane origin, does not follow redirects, ignores proxy settings and
requires HTTPS except for loopback. Tokens are marked sensitive and never
appear in `Debug` output or logs.

## Configure the worker

| Variable / flag | Purpose |
|-----------------|---------|
| `SANDBOX_NATS_URL` / `--nats-url` | NATS connection URL. Default `nats://127.0.0.1:4222`. |
| `SANDBOX_SPACE_API_URL` / `--endpoint` | Control-plane origin. Required. |
| `SANDBOX_SPACE_DATA_PLANE_URL` / `--data-plane-url` | Data-plane origin. Required. |
| `SPACE_API_TOKEN` / `--token` | Management token. Required. Hidden from `--help` and logs. |
| `SANDBOX_SPACE_TEMPLATE` / `--template` | The one template every lease is created from. Default `seed:latest`. |
| `SANDBOX_SPACE_STATE_DIR` / `--state-dir` | Worker identity, dispatch ledger and lock. Default `/var/lib/sandbox-space`. |
| `--max-leases` | Capacity announced in the heartbeat. Default 4. |
| `--boot-timeout-seconds` | How long a create may take until the guest answers. Default 180. |
| `--operation-timeout-seconds` | How long pause, suspend, wake and delete may take. Default 120. |
| `SANDBOX_WORKER_ID` / `--worker-id` | Must match the durable identity in the state directory when set. |

## Lease flow

1. `POST /v1/sandboxes` with an `Idempotency-Key` derived from the lease and the
   dispatch attempt, and metadata marking the owner, lease and attempt.
2. The dispatch ledger records the sandbox id as soon as SPACE returns it, so
   cleanup can find it.
3. Poll the create operation until it succeeds.
4. Poll the sandbox until it is `running`.
5. Probe `GET /v1/sandboxes/{id}/processes?limit=1` until the guest agent
   answers.

Only then is the lease handed out. If any step fails, or the lease is cancelled
meanwhile, the worker deletes the sandbox and waits for the deletion. A create
whose outcome is ambiguous is reconciled by listing sandboxes and deleting the
one carrying that attempt's marker.

## Lease translation

| Wire field | SPACE |
|------------|-------|
| `spec.image` | Must name the worker's configured template. Anything else is refused with `invalid_spec`. |
| `spec.env` | `env` at create time and on every command |
| `spec.vcpu_millis` | `resources.vcpu`, rounded up to whole vCPUs |
| `spec.memory_mib`, `spec.disk_mib` | `resources.memory_mib`, `resources.disk_mib`, only when set |
| `spec.working_dir` | `cwd` on commands |

The worker refuses these with a typed error before any SPACE call: privileged
guests, workspace imports, mounts and drives, exposed ports, any network policy
other than unset or allow all, filesystem policy, secret bindings, platform
options, user overrides, and persistent and prewarm leases.

## Pause, suspend and wake

| Verb | SPACE |
|------|-------|
| pause | `POST /v1/sandboxes/{id}:pause`. VM memory stays resident. |
| suspend (memory) | `POST /v1/sandboxes/{id}:suspend`. State is saved durably and the VM is released. |
| resume, wake (memory) | `POST /v1/sandboxes/{id}:wake` |
| suspend or wake at disk level | Unsupported |

Each call waits for its operation and for the sandbox to reach the target
state. A request for the state the sandbox is already in succeeds without a
SPACE call. After a wake the worker probes the guest again.

## Commands and files

- `POST /v1/sandboxes/{id}/commands` is sent exactly once. The request's launch
  key, when present, is its `Idempotency-Key`.
- An ambiguous start is never retried. If creating the command fails without a
  definite refusal, or the event stream cannot be opened after the command
  exists, the worker answers `LeaseError::Backend` with "command start outcome
  is unknown" and sends nothing again. Only a refused connection, which sent
  nothing, is retried when opening the stream.
- A stream that drops is resumed with `Last-Event-ID`. A `resync_required` or
  `error` event ends the command as unknown, because output was lost.
- The worker enforces the request timeout plus a short grace period. If SPACE
  has not reported an exit by then, the worker asks SPACE to kill the process
  and returns the partial output with `timed_out` set.
- Output is buffered and capped. It is not forwarded to the bus while the
  command runs.
- Files are written and read through guest shell commands (`mkdir`, `mktemp`,
  `base64`, `dd`, `mv`), at most 16 MiB and 256 files per write. Guest `base64`
  and `dd` must exist in the template. `as_user` is refused.

## Delete

Delete is accepted in any sandbox state and wins every race. Cancelling a
lease whose create is still pending flags the create to stop, deletes the
sandbox as soon as its id is known, and returns once the create path has wound
down. Cancelling twice is safe.

At startup and shutdown the worker deletes every sandbox it owns, found by the
owner marker in the sandbox metadata.

## Capability profile

The worker advertises a `MicroVm` isolation tier, pause and resume, and
dormancy of freeze plus memory suspend. It advertises no egress enforcement,
output streaming, file operations, ports, snapshots, named processes,
interactive stdin or warm pool.

## Limits

- No egress enforcement. The worker does not translate `NetworkPolicy` into a
  SPACE network policy, so it refuses restrictive policies instead of claiming
  them.
- No bus output streaming, interactive stdin, detached or named processes,
  readiness probes or user overrides.
- No snapshots or checkpoints, no warm pool, no exposed ports or preview URLs,
  and no idle policy.
- No native files API. Files go through the guest shell.

## Test the worker

```sh
cargo test -p trogon-sandbox-space-worker
```

The tests run against an in-process fake that serves both planes on separate
listeners.

## Related

- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md)
- [Run the Vercel Sandbox worker](./vercel-worker.md) and
  [Run the E2B worker](./e2b-worker.md)
