# Publish a signed, scanned snapshot

This tutorial takes you from a running sandbox lease to a Cosign-signed, scanned snapshot pushed to a local Zot registry. At the end you have a snapshot digest you can hand to another worker for a cold restore.

It assumes you finished [your first sandbox lease](./first-sandbox-lease.md), and have `docker`, `cosign`, `trivy`, `syft` and `oras` on your path.

## What you will build

```text
docker worker -> snapshot export -> OCI registry (Zot)
                                       |
trogon-sandbox-snapshot-builder -> scan (trivy) -> SBOM (syft) -> Cosign signature
                                       |
                                 referrers attached to the snapshot digest
```

## Steps

### 1. Start a local Zot registry

```bash
docker run -d --name zot -p 5000:5000 ghcr.io/project-zot/zot:v2.1.2
```

### 2. Configure the snapshot builder

```bash
export SANDBOX_NATS_URL=nats://127.0.0.1:4222
export SANDBOX_REGISTRY_URL=127.0.0.1:5000
export SANDBOX_OCI_INSECURE_REGISTRIES=127.0.0.1:5000
export SANDBOX_COSIGN_KMS_URI=awskms:///alias/sandbox-demo
export SANDBOX_REQUIRE_SCAN_PASS=true
```

`SANDBOX_COSIGN_KMS_URI` names an AWS KMS key that `cosign` signs with. When it is unset, the builder skips signing. `SANDBOX_REQUIRE_SCAN_PASS` defaults to `true`. While it is `true`, the builder runs Trivy against the snapshot, attaches the scan report and fails the publication when the scan does not pass the severity score in `SANDBOX_SCAN_CVSS_THRESHOLD` (default `7.0`). When it is `false`, the builder runs no scan and attaches no scan report.

### 3. Start the snapshot builder and the Docker worker

```bash
cargo run -p trogon-sandbox-snapshot-builder &
cargo run -p trogon-sandbox-docker-worker -- --snapshot-registry 127.0.0.1:5000 &
```

The Docker worker also needs its image settings, such as `SANDBOX_IMAGE` and `SANDBOX_EGRESS_PROXY_IMAGE`, as in the first lease tutorial.

### 4. Lease with `snapshot_out`

Add a `snapshot_out` policy to the lease spec from the first lease tutorial:

```json
{
  "snapshot_out": {
    "artifact_type": "sandbox_rootfs_v1",
    "target_repository": "snapshots/demo"
  }
}
```

`retention`, `prewarm` and `tag_name` are optional fields of the same object.

### 5. Watch the publication

In another terminal:

```bash
nats sub 'sandbox.snapshot.published'
```

When the lease releases, you see a payload like this:

```json
{
  "lease_id": "...",
  "digest": "sha256:abcdef...",
  "artifact_type": "sandbox_rootfs_v1",
  "tenant": "demo",
  "bytes": 6291456
}
```

### 6. Inspect the artifact and its referrers

```bash
oras manifest fetch --plain-http 127.0.0.1:5000/snapshots/demo@sha256:abcdef...
oras discover --plain-http 127.0.0.1:5000/snapshots/demo@sha256:abcdef...
```

`discover` lists the Cosign signature, the SBOM and the scan report as referrers whose subject is your digest.

### 7. Verify the signature

```bash
cosign verify --key <public key> \
  127.0.0.1:5000/snapshots/demo@sha256:abcdef...
```

## What happened

1. When the lease released with a `snapshot_out` policy, the snapshot builder reserved the capture with the broker so the snapshot is counted once.
2. It asked the worker that held the lease to export the snapshot to the registry.
3. It pulled the artifact back, scanned it with Trivy, attached an SBOM from Syft and signed the manifest with the configured KMS key. The signature, SBOM and scan report are attached as referrers.
4. It recorded the digest in the JetStream KV bucket `sandbox-broker-kv-snapshot` and published `SnapshotPublished` on `sandbox.snapshot.published`.
5. It confirmed the accounting with the broker and moved the snapshot tag, if the policy named one.

## Next steps

- [Onboard a tenant](./onboard-tenant.md) for per-tenant S3 and KMS setup.
- Lower `SANDBOX_SCAN_CVSS_THRESHOLD` and watch the builder reject a snapshot with known vulnerabilities.
- Restore from the snapshot on a different worker by leasing with a `snapshot` image that carries the digest.

## See also

- [Snapshot store](../../architecture/sandbox/snapshot-store.md)
- [Publish a snapshot](../../how-to/sandbox/publish-a-snapshot.md)
- [Media types](../../reference/sandbox/media-types.md)
