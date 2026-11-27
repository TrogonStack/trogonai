# Sandbox broker

`trogon-sandbox-broker` is the control-plane service of the sandbox stack. It
accepts lease, command, file, drive, snapshot, terminal, and entity requests from
clients over NATS request/reply, admits them against tenant policy and quota,
signs a dispatch for the worker that will execute it, and records the outcome in
JetStream and KV state.

The crate ships a library (`trogon_sandbox_broker`) and a binary
(`trogon-sandbox-broker`). The binary is a thin entry point that calls
`serve::run`; the library exposes the pieces so other sandbox crates (the admin
tooling, the snapshot builder, and workers) can reuse the same stores and
policies.

## What it does

- **Lease lifecycle.** `core` handles lease requests, extension, pause and resume,
  release, and cancellation, with `active_lease`, `named_lease`, `pending_release`,
  `pending_purge`, `idle`, and `sweeper` carrying the durable bookkeeping.
- **Admission.** `quota`, `limits`, `placement_gate`, and the `*_policy` modules
  decide whether a request may proceed. Lease requests are matched against the
  worker capability profiles registered through `worker_registry`.
- **Dispatch.** `signing_bus` and `worker_authority` sign operations with the
  protocol crate's dispatch signer, and `worker_fence` rejects stale worker epochs.
- **Commands and files.** `command_launch`, `command_logs`, and the `core`
  command, file, and terminal handlers.
- **Drives and snapshots.** `drive_store`, `drive_purge`, `snapshot_catalog`,
  `snapshot_tag`, `snapshot_tracker`, `snapshot_accounting`, `retention_sweep`,
  and `artifact_lister`.
- **Entity state.** `entity_store` and `entity_shadow` mirror lease state to the
  entity stream.
- **Operations.** `auth` and `auth_setup` verify callers, `metrics` exposes the
  `sandbox_broker_*` Prometheus series on the metrics endpoint, and `cli` parses
  flags and environment variables.

## How it fits the other sandbox crates

| Crate | Role for the broker |
| --- | --- |
| `trogon-sandbox-contract` | Identifiers, specs, capability profiles, lease and operation rules |
| `trogon-sandbox-protocol` | NATS subjects, KV keys, dispatch signing and verification, authority types |
| `trogon-sandbox-auth` | Caller authentication and authorization |
| `trogon-sandbox-credential-broker` | Credential issuance for leases |
| `trogon-sandbox-observability` | Shared tracing and metric conventions |
| `trogon-sandbox-oci` | Image reference and digest handling |

## Testing

Tests that need NATS start a JetStream server in a container through
`trogon_nats::test_support::JetStreamTestServer`, so they require a working
container runtime. The Bazel targets carry the `requires-network` tag for this
reason.
