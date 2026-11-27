# Publish a snapshot

This guide walks through requesting a post-run snapshot on a lease, how the snapshot builder picks it up, how to verify the resulting digest and how to inspect the artifact with the `oras` CLI. For a guided first run with a local registry, signing and scanning, follow [Publish a signed, scanned snapshot](../../get-started/sandbox/publish-signed-snapshot.md) instead.

## Prerequisites

- A NATS URL and a tenant credential, the same as for any lease.
- A running `trogon-sandbox-snapshot-builder` and a worker whose provider can export snapshots. Check the worker profile with `trogon-sandbox-admin capability show <provider>`.
- A snapshot registry reachable from the builder and the workers. The builder reads it from `SANDBOX_REGISTRY_URL`.
- For operator inspection, the `oras` CLI logged in to the registry.

## Mark the lease for a post-run snapshot

Set `snapshot_out` in the lease spec. The field is an optional `SnapshotPolicy`:

```rust
SnapshotPolicy {
    artifact_type: ArtifactType::SandboxRootfsV1,
    target_repository: "snapshots/acme".to_string(),
    retention: None, // or Some(RetentionPolicy { max_tags, max_age_days })
    prewarm: None,
    tag_name: None, // or Some("prod".to_string()) to move a named tag
}
```

On the wire:

```json
{
  "spec": {
    "image": { "kind": "registry", "reference": { "repository": "alpine", "tag": "3.20" } },
    "workspace": { "empty": {} },
    "snapshot_out": {
      "artifact_type": "sandbox_rootfs_v1",
      "target_repository": "snapshots/acme"
    }
  }
}
```

The worker runs the lease normally. The export happens after release, not on the hot path, so release stays fast. A running lease can also be captured without releasing it; see [Snapshot a running sandbox](./snapshot-a-running-sandbox.md).

## Release the lease

When the lease releases, the broker publishes `LeaseReleased`. The snapshot builder reacts to released leases whose spec carries `snapshot_out`:

1. The builder reserves the capture with the broker so the snapshot is counted once.
2. It asks the worker that held the lease to export the snapshot to the registry.
3. It scans the artifact with Trivy, attaches an SBOM from Syft and signs the manifest with Cosign, depending on `SANDBOX_REQUIRE_SCAN_PASS` and `SANDBOX_COSIGN_KMS_URI`.
4. It writes the digest record to the JetStream KV bucket `sandbox-broker-kv-snapshot` and publishes `SnapshotPublished` on `sandbox.snapshot.published`.
5. It confirms the accounting with the broker and moves the tag, if the policy named one.

## Verify the digest

The authoritative record is the KV entry for the digest in `sandbox-broker-kv-snapshot`. It carries the source lease, tenant, artifact type, registry location, parent digest and creation time. Workers refuse to restore a digest the broker has no record of.

The simplest way to learn the digest is to subscribe to the publication event:

```bash
nats sub 'sandbox.snapshot.published'
```

To check whether a lease's capture succeeded or failed, ask the admin tool:

```bash
trogon-sandbox-admin snapshot status <lease-id>
```

Tags on the repository exist for human navigation only. The identity is always the digest.

## Inspect the artifact with the `oras` CLI

Operators use the upstream `oras` CLI against the registry. The workers and the builder talk to it through `trogon-sandbox-oci` and never shell out to it.

```bash
oras manifest fetch registry.example.com/snapshots/acme@sha256:abc123...
oras discover registry.example.com/snapshots/acme@sha256:abc123...
```

`discover` lists the Cosign signature, the SBOM and the scan report as referrers of the snapshot.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| No KV record after release | The builder is not running, the worker export failed, or the push retries ran out. Check the builder logs. |
| The worker answers `unsupported` for the export | The provider profile does not advertise snapshot export. |
| A scan failure and no published snapshot | `SANDBOX_REQUIRE_SCAN_PASS` is on and the scan did not pass `SANDBOX_SCAN_CVSS_THRESHOLD`. Check the builder logs. |
| Referrers missing | The registry lacks the Referrers API. The crate falls back to the tag scheme and counts it in `trogon_sandbox_oci_referrers_fallback_total`. |

## See also

- [Snapshot store](../../architecture/sandbox/snapshot-store.md) for the architecture and trust boundary.
- [Provider capability profiles](../../architecture/sandbox/capability-profiles.md) for the export flags.
- [Restore from a snapshot](./restore-from-snapshot.md) to consume a digest.
- [Back up snapshots](./snapshot-backup.md) for disaster recovery.
- [Add a pull secret](./add-a-pull-secret.md) for base image pulls.
- [Media types](../../reference/sandbox/media-types.md) for the `ArtifactType` contract.
