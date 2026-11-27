# Sandbox volumes

`trogon-sandbox-volumes` moves workspace trees between a sandbox lease and an
object store (S3, Azure Blob Storage, Google Cloud Storage, or a local directory).
It pulls a prefix into a directory, pushes only the files that changed, and
supports copy-on-write workspaces and FUSE mounts of a bucket prefix.

## Types

- `VolumeClient` pulls a `VolumePrefix` into a directory (`pull_to_dir`), pushes
  changed files (`push_changed`), checks for a prefix (`head_exists`), and runs the
  copy-on-write flow (`cow_setup` returns a `CowHandle`, `cow_finalize` returns the
  tree `Digest`). It is built from a `VolumeClientConfig` and a `VolumeMode`.
- `VolumeBackend` selects the store: S3, Azure, GCS, or a local directory. Bucket,
  region, endpoint, and resource names are parsed into `VolumePrefix`,
  `VolumeRegion`, `VolumeEndpoint`, `StoreResourceName`, and `KmsKeyId`; each has a
  typed error and construction fails on invalid input.
- `ManifestKey` is a validated relative object key. `ChangedManifest` and
  `ChangedManifestEntry` describe the files that differ between a baseline and a
  directory; `build_changed_manifest` and `compute_tree_digest` produce them.
  `PullSummary` and `PushSummary` report file and byte counts.
- `VolumeMetrics` and `FuseMetrics` register Prometheus collectors. Metric names
  use the `trogon_sandbox_volumes_` prefix: `bytes_in_total`, `bytes_out_total`,
  `pull_duration_seconds`, `push_duration_seconds`, `cache_hit_total`,
  `fuse_mount_total`, and `fuse_mount_duration_seconds`.
- `FuseMountManager` mounts a bucket prefix through an external FUSE binary and
  returns a `FuseMountHandle`. `FuseMountConfig`, `FuseOtherUsers`, and
  `FusePlatform` configure it. FUSE mounts are supported on Linux only; other
  platforms return `VolumeError::FuseUnsupported`.

## Volume modes

This crate re-exports `VolumeMode` from `trogon-sandbox-contract`. The client records the
mode as a metric label and uses it to choose the flow: `ReadOnly` and `ReadWrite`
pull and push a prefix directly, while `CopyOnWrite` pulls a base prefix with
`cow_setup`, collects changes in a delta directory, and uploads only that delta
under a lease prefix on `cow_finalize`.

## Tenancy

Each tenant uses a dedicated bucket. A `KmsKeyId` can be recorded on the client
config and read back with `kms_key_id`; the client does not apply it to requests.
`VolumeClient::with_options` accepts an optional byte quota that limits a single
push and fails with `VolumeError::QuotaExceeded`.

## Features

`volume-cache` enables `clone_cache_prefix`, `try_pull_cached_clone`, and the
related helpers, which store and restore clone workspaces under a prefix derived
from the repository URL and commit. `CloneCacheConfig` selects the store and
`CloneCacheOutcome` reports a `Hit` or a `Miss`. Hits are counted through
`VolumeClient::record_cache_hit`.

## Use with other sandbox crates

The crate depends on `trogon-sandbox-contract` for `Digest`, `VolumeMode`, and the
mount and workspace source types. Workers use it to materialize the drives and
workspace sources a lease declares.
