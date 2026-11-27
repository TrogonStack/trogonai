# Sandbox SPACE worker

`trogon-sandbox-space-worker` serves sandbox leases as Firecracker microVMs through the
SPACE native API. It is both a library and the `trogon-sandbox-space-worker` binary.

The worker holds a lease for each sandbox it creates, answers lease dispatches, cleanups and
lease operations on NATS, and publishes a heartbeat. It implements `trogon_sandbox_protocol::Provider`
with `SpaceProvider`, and `SpaceWorker` connects that provider to the subjects defined in
`trogon_sandbox_protocol::subjects`.

## Endpoints and tokens

`SpaceApi` talks to two origins, each a `SpaceOrigin` that is HTTPS or plain HTTP to a
loopback host with no credentials, query or path.

- The control plane serves `/v1/...` and is called with the management `SpaceToken`.
- The data plane serves command event streams. It is called with a per-command attach token,
  and event URLs SPACE returns must resolve to the data plane origin. Streams resume with
  `Last-Event-ID`.

Tokens never appear in `Debug` output.

## Leases

`SpaceConfig` holds the SPACE template, the state directory, the boot and operation timeouts,
the poll interval, the maximum number of leases (`MaxLeases`), the command grace period and the
file read timeout. Requests the worker cannot honour, such as privileged sandboxes, host mounts,
port exposure, egress policies other than the default, user overrides or volumes, are refused
with a violation before anything is created.

A cancel that arrives while a create is pending races it through the pending record, so a sandbox
created after the cancel is destroyed. Sandboxes carry the worker, lease and dispatch attempt as
metadata. `SpaceProvider::recover` destroys sandboxes an earlier run created, and shutdown does
the same for the sandboxes the worker still holds.

The state directory holds the worker lock and `dispatch.db`, the dispatch ledger from
`trogon_sandbox_protocol::AttemptLedger`. The ledger allows one attempt per lease and gives the
worker its durable identity.

## Running

| Setting | Flag | Environment variable | Default |
| --- | --- | --- | --- |
| NATS server | `--nats-url` | `SANDBOX_NATS_URL` | `nats://127.0.0.1:4222` |
| Control plane | `--endpoint` | `SANDBOX_SPACE_API_URL` | required |
| Data plane | `--data-plane-url` | `SANDBOX_SPACE_DATA_PLANE_URL` | required |
| Management token | `--token` | `SPACE_API_TOKEN` | required |
| Template | `--template` | `SANDBOX_SPACE_TEMPLATE` | `seed:latest` |
| State directory | `--state-dir` | `SANDBOX_SPACE_STATE_DIR` | `/var/lib/sandbox-space` |
| Worker ID | `--worker-id` | `SANDBOX_WORKER_ID` | the durable one |
| Authority mode | none | `SANDBOX_WORKER_AUTHORITY` | `warn` (`off`, `warn` or `enforce`) |

`--max-leases`, `--boot-timeout-seconds` and `--operation-timeout-seconds` tune capacity and timeouts.
An interrupt stops the worker, lets in-flight requests finish and reclaims the sandboxes it owns.

## Related crates

- `trogon-sandbox-contract` defines the wire types.
- `trogon-sandbox-protocol` defines the provider trait, subjects, the dispatch ledger, request
  verification and the authority guard.
