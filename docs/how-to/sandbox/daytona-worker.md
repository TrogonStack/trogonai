# Run the Daytona worker

`trogon-sandbox-daytona-worker` subscribes to `sandbox.provider.daytona.lease`
and to the per-lease verbs under `sandbox.provider.daytona.*`, including
`sandbox.provider.daytona.cancel.>`. It provisions each lease as a Daytona
sandbox through the Daytona HTTP API.

Broker clients request this provider with `daytona` in `provider_pref`.
`daytona-io` is accepted as an alias.

## Configure the worker

| Variable / flag | Purpose |
|-----------------|---------|
| `DAYTONA_API_KEY` / `--api-key` | Daytona API key. Required. Hidden from `--help` and logs. |
| `DAYTONA_BASE_URL` / `--api-url` | API base URL. Default `https://app.daytona.io/api`. |
| `DAYTONA_ORGANIZATION_ID` / `--organization-id` | Organization the key acts for. Unset uses the key's own. |
| `SANDBOX_NATS_URL` / `--nats-url` | NATS connection URL. Default `nats://127.0.0.1:4222`. |
| `SANDBOX_DISPATCH_FENCE_FILE` / `--dispatch-fence-file` | Journal that keeps dispatch fencing across worker restarts. Unset keeps it in memory. See [Sign worker dispatches](./sign-worker-dispatches.md). |
| `SANDBOX_DAYTONA_SNAPSHOT` / `--snapshot` | Snapshot a sandbox starts from when a lease names no image. Default `sandbox-agent:dev`. |
| `SANDBOX_DAYTONA_HTTP_TIMEOUT_SECS` / `--http-timeout-secs` | Timeout for every Daytona request, in seconds. Default 300, minimum 1. A foreground command holds one request open, so this also bounds how long a command can run. Use a detached command for longer work. |
| `SANDBOX_DAYTONA_START_TIMEOUT_SECS` / `--start-timeout-secs` | Seconds a sandbox may take to start or stop before the lease fails. Default 300, range 1 to 3600. |
| `SANDBOX_DAYTONA_COMMAND_SUPERVISOR` / `--command-supervisor` | Worker-host path to the static Linux command supervisor that ordinary detached commands run through. Give it together with its SHA-256 pin. See [Control detached commands](#control-detached-commands). |
| `SANDBOX_DAYTONA_COMMAND_SUPERVISOR_SHA256` / `--command-supervisor-sha256` | Lowercase hex SHA-256 of that exact artifact. Each flag requires the other, and a missing or invalid artifact or a pin that does not match refuses worker startup. |
| `SANDBOX_MAX_CONCURRENT_LEASES` / `--max-concurrent-leases` | Capacity announced in the heartbeat. Default 4. |

Observability flags such as `--metrics-addr` and `--otlp-endpoint` are shared
by all workers. See [Observe the sandbox stack](./observability.md).

## Choose an image

A lease image must name a Daytona snapshot. The image reference names the
snapshot, so `my-snapshot` launches the snapshot called `my-snapshot`. A
reference that is not a snapshot in the organization fails with Daytona's
`Snapshot ... not found` error. An empty repository uses
`SANDBOX_DAYTONA_SNAPSHOT`.

A native snapshot receipt from this provider restores by its snapshot name. A
native receipt from another provider, and any other image kind, is refused.

The snapshot fixes the sandbox's vCPUs, memory and disk. A lease that sets
`vcpu_millis` or `disk_mib` is refused with `invalid_spec`.

## How a lease maps to Daytona

| Wire field | Daytona |
|------------|---------|
| `spec.image` | Snapshot name, see above |
| `spec.env` | `env` |
| `tags` | Labels |
| `req.region` | `target` |
| `spec.network_policy` | `networkBlockAll` and `networkAllowList`, see [Network policy](#network-policy) |
| `spec.idle_timeout_seconds` | `autoStopInterval`, rounded up to whole minutes |
| `spec.workspace` | An empty workspace creates `/workspace`. A clone workspace runs a git clone of the pinned commit into `/workspace`. |

The clone URL must use `https`, must not carry credentials, and must have no
query or fragment. Pass credentials as the clone auth. A host mount or an S3
volume workspace is refused, and so is any mount, because Daytona cannot bind
a worker host path into the sandbox.

The worker accepts `auto_stop_interval`, `auto_archive_interval`,
`auto_delete_interval` and `public` under `native.daytona`. Unknown keys and
invalid values are refused before create. The interval values are in minutes,
at most 525600, and override the defaults described below.

## Network policy

Daytona restricts egress by CIDR block only, and at most ten blocks. The worker
refuses anything it cannot carry with `InvalidNetworkPolicy` instead of
approximating it.

- Unset or allow all leaves Daytona's default, which is unrestricted egress.
- Deny all, or a policy that allows nothing, sets `networkBlockAll`.
- A user defined policy becomes an allow list. A host rule is accepted only
  when it names an IP literal, which becomes a single address block.
- Denied domains, request rules and a policy that refuses opaque tunnels are
  refused. A user defined policy refuses opaque tunnels unless it sets
  `"opaque_tunnels": "allow"`, so one sent to this worker has to say so.

The profile declares best-effort egress enforcement. Verify enforcement on your
own account before relying on it for isolation. Changing the policy of a running
lease is refused, because the worker applies network settings at create time
only.

## Idle limit, pause and resume

- **Auto-stop.** Unless overridden under `native.daytona`,
  `spec.idle_timeout_seconds` becomes `autoStopInterval`, rounded up to whole
  minutes. With no idle limit the worker sends `0`, which turns off Daytona's
  own default, so only the broker's TTL ends the lease. The archive and delete
  intervals stay at Daytona's defaults unless overridden.
- **Pause and resume.** Pause stops the Daytona sandbox and resume starts it
  again, waiting for the new state each time. Files survive and running
  processes do not. A repeated request is a no-op.

## Snapshots

The profile declares no snapshot support. A checkpoint or snapshot export
request gets a typed unsupported reply, because Daytona owns its snapshots.

## Commands

- Commands run through the toolbox inside a guest capture script. The guest
  needs `sh`, `env` and `timeout`. Finite commands use the guest `timeout`
  utility. An explicit lease deadline requires a detached command and installs
  no finite timer.
- A detached command starts in the background and is polled by its command id.
  The capture directory is removed when the command is collected.
- `run_as_user` wraps the command in `sudo -n -u <user> env`, so the user needs
  passwordless sudo, as Daytona's default user has.
- Live output streaming, interactive stdin, terminals, named processes, sandbox
  users and groups, and command cancellation are not declared in the profile.
  Each returns a typed unsupported or denied outcome.
- Ports are previewable, including after create (`supports_port_preview` and
  `supports_expose_after_create`).

## Control detached commands

Signalling an unfinished detached command needs the command supervisor.
Without `--command-supervisor` and `--command-supervisor-sha256`, a signal
request for such a command is denied.

1. Build a static Linux ELF supervisor for the guest architecture: x86-64 for a
   Node `x64` guest, AArch64 for `arm64`. The target is the guest, even when the
   worker runs on another operating system.
2. Compute its SHA-256 as 64 lowercase hex digits and start the worker with the
   artifact path and the digest.
3. Use a snapshot that runs Linux with Node.js 22.7 or newer. Before releasing
   a detached command, the worker probes the guest platform and architecture,
   uploads the supervisor, and refuses the command when the guest does not
   qualify.

## Test the worker

```sh
cargo test -p trogon-sandbox-daytona-worker
```

## Related

- [Native passthrough and regions](./native-passthrough-and-regions.md)
- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md)
