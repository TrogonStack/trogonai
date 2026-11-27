# Volumes

This page explains mutable workspace storage in the sandbox stack: how `WorkspaceSource::S3Volume` differs from
snapshots, what the three `VolumeMode` values mean, how tenants are isolated, and how streaming FUSE mounts differ from
copy-based volumes. Read it before implementing volume sync in a worker or configuring tenant storage.

## Snapshots and volumes

Snapshots (see [the snapshot store](./snapshot-store.md)) are read-only templates. They capture a filesystem or VM disk
at a point in time, are content-addressed by `Digest`, and are stored immutably in the platform registry. Restoring one
gives a clean starting state.

Volumes are mutable workspace data held in an object store. They hold clone artifacts, build caches, test evidence and
any state that changes across leases and outlives a single sandbox run.

| | Snapshots | Volumes |
|---|---|---|
| Mutability | Immutable (digest-pinned) | Mutable (`ReadWrite`, `CopyOnWrite`) |
| Store | Platform OCI registry | Object store (S3 and compatible backends) |
| Wire type | `SandboxImage::Snapshot` | `WorkspaceSource::S3Volume` |
| Restore on lease | Yes (warm start) | Mounted or synced alongside the snapshot |
| Post-run capture | `SandboxSpec.snapshot_out` | Changed files uploaded on release |

Restoring a snapshot does not restore a volume. On lease start the worker handles both independently: the snapshot
becomes the root (or guest disk) and the volume becomes the workspace.

The implementation crate is `rsworkspace/crates/sandbox/trogon-sandbox-volumes`. It supports S3, Azure Blob Storage,
Google Cloud Storage and a local directory as backends.

## `WorkspaceSource::S3Volume` wire shape

```rust
S3Volume {
    bucket: String,
    prefix: String,
    mode: VolumeMode,
    snapshot: Option<Digest>,
}
```

- `bucket` and `prefix` locate the object-store prefix for this volume.
- `mode` selects sync semantics.
- `snapshot` optionally names a base digest for a copy-on-write volume.

## `VolumeMode` semantics

### `ReadOnly`

The worker downloads the prefix into a local scratch directory at boot. Nothing is uploaded on release. Use it for
shared assets that many leases consume without mutation, such as cached dependencies or frozen datasets.

### `ReadWrite`

The worker downloads the prefix at boot and uploads only the files that changed at release. Use it when a lease
produces output that must persist, such as test artifacts or build output.

### `CopyOnWrite`

The worker reads from a base prefix but collects writes in a delta directory. On release it uploads only that delta under
a per-lease prefix. This suits workspaces that accumulate state across many leases but should not pay a full upload on
every release.

Which modes a worker accepts is part of its capability profile (`VolumeCapability.supported_modes`). Use
`trogon-sandbox-admin capability show` to read it for a given worker, and see
[provider capability profiles](./capability-profiles.md).

## Per-tenant isolation

Each tenant uses a dedicated bucket. A worker never holds long-lived object-store root credentials: the credential
broker issues credentials scoped to the bucket and prefix (read-only for `ReadOnly`, read-write for the other modes) at
lease start. See [the per-tenant onboarding tutorial](../../get-started/sandbox/onboard-tenant.md) for provisioning.

## Streaming FUSE mounts

The copy-based modes download objects into a local directory before the sandbox sees them, so the sandbox consumes
space proportional to the volume and the first read waits on the initial sync.

A streaming mount works differently. The worker runs the `mount-s3` binary on the host, which presents a bucket prefix
as a filesystem through the Linux FUSE driver. Objects are fetched on demand as the sandbox reads them, and nothing is
copied before the sandbox starts.

| Characteristic | Copy-based (`S3Volume`) | Streaming (`RemoteMount`) |
|---|---|---|
| Wire type | `WorkspaceSource::S3Volume` | `SandboxSpec.mounts[].source` |
| Data locality | Fully local at sandbox start | Fetched on demand |
| Write semantics | `ReadOnly`, `ReadWrite`, `CopyOnWrite` | `readonly` true or false |
| Storage on worker | Proportional to prefix size | Minimal |
| Cold start latency | High for large prefixes | Near zero regardless of prefix size |
| Suitable for | Build caches, test artifacts, stateful workspaces | Large datasets, model weights, read-heavy data |
| Multiple mounts | One workspace | Several entries in `mounts` |

Use copy-based modes when the sandbox mutates data and the changes must persist. Use streaming mounts when data is
read-heavy, too large to copy up front, or when several independent prefixes must appear as separate directories.

### The worker mounts, not the sandbox

The worker mounts the FUSE filesystem on the host before starting the sandbox, then bind-mounts the already-mounted
directory into it. The sandbox never sees object-store credentials and never calls `mount`, so its seccomp policy does
not need to allow mount syscalls. If the FUSE process dies, the sandbox sees ordinary I/O errors rather than a
credential exposure. Revoking the worker's credentials stops new I/O to mounted prefixes without rotating anything per
sandbox.

### Capability flag and broker inference

Each worker advertises FUSE support through `VolumeCapability.supports_fuse_mounts`, which maps to the
`WorkerCapabilities::SUPPORTS_FUSE_MOUNTS` bit the broker inspects when choosing a worker. You do not set
`needs_fuse_mounts` yourself: when a lease carries a non-empty `spec.mounts`, the broker marks it as required before
checking worker profiles. If no registered worker satisfies it, the lease is denied with
`NoWorkerSatisfiesCapabilities`.

A mount's `mount_path` must be an absolute path, must not resolve to `/`, and must not contain `..`. Duplicate
`mount_path` values in one request are rejected.

### Worker support

| Worker | FUSE support | Notes |
|---|---|---|
| `trogon-sandbox-docker-worker` | Yes | Bind-mounts the host directory into the container, read-only when requested |
| `trogon-sandbox-subprocess-worker` | Linux only | Enabled with `SANDBOX_FUSE_ENABLED`; the binary path is `SANDBOX_FUSE_MOUNT_S3_BINARY` (default `mount-s3`) |
| `trogon-sandbox-modal-worker` | Never | The mount lives on the worker host, which a remote Modal sandbox cannot see; the worker rejects non-empty `spec.mounts` |
| All other workers | No | They advertise `supports_fuse_mounts` as unsupported, so the broker does not dispatch mount leases to them |

## Limitations

- Workers that reject `S3Volume` workspaces return an error at lease time: the subprocess, Firecracker, KubeVirt, Hyperlight and Daytona workers. The Firecracker guest has no network to sync over. Use a worker that supports volumes, or a `Clone` workspace.
- Only the Docker and subprocess workers list `CopyOnWrite` in their capability profile. Other workers accept `ReadOnly` and `ReadWrite` at most (Daytona and E2B accept only `ReadWrite`). Check with `trogon-sandbox-admin capability show`.
- Copy-on-write uploads the delta under the lease's own prefix. Nothing merges that delta back into the base prefix, so a later lease reads the base unless the caller points it at the delta prefix.
- A `KmsKeyId` can be recorded on the volume client configuration, but the client does not apply it to requests. Configure encryption at rest on the bucket itself.
- FUSE streaming mounts work on Linux only. On other platforms the volumes crate returns `VolumeError::FuseUnsupported`.
- A single push can be limited by a byte quota, and a push over the quota fails with `VolumeError::QuotaExceeded`. Size the quota for the largest expected change set.

## See also

- [Snapshot store](./snapshot-store.md)
- [Provider capability profiles](./capability-profiles.md)
- [Mount remote storage](../../how-to/sandbox/mount-remote-storage.md)
