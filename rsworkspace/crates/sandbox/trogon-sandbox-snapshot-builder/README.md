# trogon-sandbox-snapshot-builder

A NATS service that turns a released lease into a published snapshot.

For every lease release that asks for a snapshot, the builder:

1. Reserves the capture with the broker, so a snapshot is counted once.
2. Asks the worker that held the lease to export its snapshot to the registry, retrying only when no worker was
   listening. The request carries a signed dispatch header when a signing key is configured.
3. Pulls the artifact back, gates it on a vulnerability scan, attaches a software bill of materials, and signs it
   with a KMS key.
4. Records the digest in JetStream KV and publishes `SnapshotPublished`.
5. Confirms the accounting with the broker and moves the snapshot tag, newest release winning.

A tag that could not be moved leaves the snapshot published. A durable consumer on the publication stream moves it
later, so the move never rebuilds or republishes anything.

Firecracker snapshots are unpacked for scanning by running this binary again with a hidden flag, under byte, entry,
depth, time, memory, and file descriptor limits.

## Running

```sh
trogon-sandbox-snapshot-builder --nats-url nats://127.0.0.1:4222 --registry-url 127.0.0.1:5000
```

Every flag has an environment variable, shown by `--help`. Scanning needs `trivy` and `syft` on the path, and
signing needs `--cosign-kms-uri`. Metrics are served on the address from `--metrics-addr`:

- `snapshot_builder_pushed_total` by `artifact_type`
- `snapshot_builder_failed_total` by `reason`
- `snapshot_builder_inflight`
- `snapshot_builder_queue_depth`
