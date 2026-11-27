# Restore from a snapshot

This guide explains how to start a lease from a snapshot, how a named tag resolves to a digest, how the worker-side digest cache affects latency, and what to do when no worker can restore.

## Prerequisites

- A NATS URL and a tenant credential.
- A known digest recorded in the `sandbox-broker-kv-snapshot` bucket, or a named snapshot tag that points at one. See [Publish a snapshot](./publish-a-snapshot.md).
- A `provider_pref` list whose head provider can restore. Check with `trogon-sandbox-admin capability show <provider>`.

## Lease from a digest

Reference the artifact with a digest-pinned `OciArtifactRef`:

```rust
SandboxImage::Snapshot {
    reference: OciArtifactRef {
        registry: "registry.example.com".to_string(),
        repository: "snapshots/acme".to_string(),
        digest: Digest::new("sha256:abc123...")?,
        artifact_type: ArtifactType::SandboxRootfsV1,
    },
    parent: None, // or Some(OciImageRef) for the base image the snapshot came from
}
```

On the wire:

```json
{
  "spec": {
    "image": {
      "kind": "snapshot",
      "reference": {
        "registry": "registry.example.com",
        "repository": "snapshots/acme",
        "digest": "sha256:abc123...",
        "artifact_type": "sandbox_rootfs_v1"
      }
    },
    "workspace": { "empty": {} }
  },
  "provider_pref": ["local_docker"]
}
```

The broker selects a worker whose capability profile can restore the artifact type and dispatches the lease to it.

## What the worker does

On lease start the worker:

1. Pulls the manifest and layers from the registry through `trogon-sandbox-oci`.
2. Verifies that the manifest digest matches `OciArtifactRef.digest`.
3. Cross-checks the digest against the broker's KV record and refuses an unrecorded digest.
4. Restores the artifact. The Docker worker imports the rootfs tar, the Hyperlight worker decodes its snapshot and the KubeVirt worker attaches the disk.

The Firecracker worker also checks the Cosign signature and the environment digest. See [Snapshot store](../../architecture/sandbox/snapshot-store.md).

## Lease from a named tag

A digest is exact but awkward to pass around. A named snapshot tag is a tenant-scoped pointer the broker resolves to a digest at lease time. Point a tag at a digest with the admin tool:

```bash
trogon-sandbox-admin tag set acme prod sha256:abc123... --artifact-type sandbox_rootfs_v1
trogon-sandbox-admin tag list acme
trogon-sandbox-admin tag history acme prod
trogon-sandbox-admin tag get acme prod
trogon-sandbox-admin tag clear acme prod
```

`tag set` accepts `--expected-revision` to refuse the change when someone moved the tag since you read it. Then lease with the tag:

```json
{
  "spec": {
    "image": {
      "kind": "snapshot_tag",
      "tag": { "tenant": "acme", "name": "prod" },
      "artifact_type": "sandbox_rootfs_v1"
    },
    "workspace": { "empty": {} }
  }
}
```

An optional `create_from` image names what to boot while the tag has no snapshot yet. Tag mutations are recorded in the `sandbox.audit.snapshot-tag` audit stream, and `tag history` reads them. Workers always pull the resolved digest, so a tag is a broker-side convenience. The retention passes never evict a digest a tag points at. See [Snapshot catalog](../../architecture/sandbox/snapshot-catalog.md).

## Cache hits and latency

Workers keep a local cache of pulled snapshot blobs keyed by digest, and report the cached digests in `WorkerHeartbeat.cached_digests`. The first restore of a digest on a worker pays the full registry pull. Later leases on the same worker skip the pull, and the broker prefers a worker that already holds the digest. A tenant prewarm policy (`SnapshotPolicy.prewarm`) keeps idle restored sandboxes ready to hide the cold start.

## Troubleshooting

### No worker can restore

The lease is denied with `no_worker_can_restore_snapshot`, or the broker reports no available provider.

1. Run `trogon-sandbox-admin capability show <provider>` for each entry in `provider_pref`. Confirm the profile supports restore and lists the artifact type.
2. Confirm the artifact type matches what the worker exports. Do not request `kubevirt_disk_v1` on a Docker worker.
3. Add a provider that can restore to `provider_pref`.

The subprocess provider never supports restore.

### Digest not in KV

The worker refuses the pull. Publish the snapshot first, or check the digest string: it must be `sha256:` followed by 64 lowercase hex digits.

### Registry pull fails with 401 or 403

Workers pull the snapshot repository with platform credentials. A base image named in `parent` may need a pull secret. See [Add a pull secret](./add-a-pull-secret.md).

## See also

- [Snapshot store](../../architecture/sandbox/snapshot-store.md) for the trust boundary.
- [Volumes](../../architecture/sandbox/volumes.md) to mount volumes alongside.
- [Provider capability profiles](../../architecture/sandbox/capability-profiles.md) for the restore flags.
- [Publish a snapshot](./publish-a-snapshot.md) to create digests.
- [Back up snapshots](./snapshot-backup.md) for disaster recovery.
- [Media types](../../reference/sandbox/media-types.md) for the artifact types.
