# Sandbox Fly worker

`trogon-sandbox-fly-worker` is a sandbox worker for the Fly.io Machines platform. It
leases one Fly Machine per sandbox, answers lease operations over NATS, and keeps
what it created recoverable across restarts. The `trogon-sandbox-fly-worker` binary
runs it.

## Types

- `FlyConfig` holds the worker's settings, built from `FlyConfigParts` and checked
  for consistency (lease limits, warm pool size, operator volume and workspace
  volume combinations). `FlyRegion`, `FlyAppName`, `FlyApiToken`, `FlyEndpoint`,
  `FlyVolumeId`, `FlyVolumeBinding` and `FlyWorkspaceVolume` are the parsed
  pieces. An endpoint must use HTTPS unless its host is loopback.
- `FlyApi` is the Machines REST client. It never follows redirects, so the bearer
  token cannot leave the configured endpoint. `Machine`, `MachineState`,
  `FlyVolume`, `FlySnapshot` and `ExecOutput` are the shapes it returns.
- `MachineSize` rounds a requested size up to a guest Fly can run.
- `FlyProvider` implements `trogon_sandbox_protocol::Provider`. It validates each
  lease, creates and destroys Machines and workspace volumes, runs commands,
  writes and reads files, publishes ports, pauses and resumes Machines, and takes
  native volume snapshots. Machines carry an owner label, so the worker only ever
  destroys what it created.
- `FlySnapshotRef` encodes a volume snapshot together with the region and image it
  can be restored into.
- `FlyWorker` serves a provider on NATS: targeted lease dispatch through the
  durable attempt ledger, cleanup requests, lease operations, heartbeats and the
  authority check.

## Behavior

- A lease must be ephemeral, not prewarmed, unprivileged, and use the worker's
  image or a Fly native snapshot. Workspace sources, remote mounts, network
  policies, filesystem policies, secret bindings, platform options and users are
  refused.
- With a workspace volume configured, each lease gets its own volume mounted at the
  working directory, deleted with the lease, and restorable from a native
  snapshot. With an operator volume, one Machine at a time mounts the configured
  drive.
- A warm pool of created but not started Machines can be kept to shorten lease start.
- On start and on shutdown the worker destroys every Machine and workspace volume
  it owns.
- Memory suspend is offered as a Machine suspend; disk-level suspend and
  filesystem export are not available on Fly.

## Fit

The wire types come from `trogon-sandbox-contract`. Dispatch verification, the
attempt ledger, the authority guard, heartbeats, large-payload transfer and the
guest capture protocol come from `trogon-sandbox-protocol`.

## Running

The binary reads its settings from flags or `TROGON_SANDBOX_*` environment
variables, and the API token from `FLY_API_TOKEN`. It stops on interrupt after
in-flight requests finish.
