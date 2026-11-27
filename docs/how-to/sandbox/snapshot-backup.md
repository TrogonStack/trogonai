# Back up and restore the snapshot registry

This guide covers one-shot and periodic backup of the platform snapshot registry with the upstream `oras backup` and `oras restore` commands, for air-gapped disaster recovery, cross-region migration and regulated archival. Registry operators should use this runbook. Workers never invoke these commands.

## Why this exists

The registry holds derived artifacts (rootfs templates, Hyperlight snapshots, KubeVirt disks) plus their supply-chain referrers (SBOM, scan report, Cosign signature). Some scenarios need a portable offline copy:

- Air-gapped disaster recovery: ship an OCI Image Layout tarball into a disconnected site.
- Cross-region migration: move a tenant to a new registry.
- Regulated archival: keep point-in-time registry state for audit.
- Promotion: copy tagged snapshots from a staging registry to production.

`oras backup` and `oras restore` (ORAS v1.3.0 or later) dump manifests, tags, blobs and referrers into an OCI Image Layout directory or tarball, then replay them into any compliant registry.

The broker's `sandbox-broker-kv-snapshot` bucket is separate. After a registry restore, reconcile the KV records or publish the digests again if they point at a registry host that no longer exists.

## Prerequisites

- The `oras` CLI, v1.3.0 or later.
- Read credentials for the source registry (`oras login`).
- Write credentials for the destination registry.
- Enough disk for the layout tarball. Snapshots can reach gigabytes per tenant.

## Back up one tenant

Snapshot repositories are named `snapshots/<tenant>`, or `snapshots/tenant-hex/<hex>` for a tenant id that contains `:` or `/`. Back up one repository per tenant:

```bash
REG=registry.example.com
TENANT=acme

oras backup "$REG/snapshots/${TENANT}" \
  --output "./backup/${TENANT}-$(date -u +%Y%m%d).tar"
```

`oras backup` captures every manifest and layer reachable from the repository, its tags and the referrer relationships, using the Referrers API or the tag-scheme fallback. Verify the layout:

```bash
tar tf ./backup/acme-20260626.tar | head
oras manifest fetch --oci-layout ./backup/acme-20260626.tar --digest sha256:...
```

ORAS has no tenant API, so scope each backup by repository path.

## Restore into a fresh registry

After standing up a new registry:

```bash
DEST=registry.dr.example.com
oras login "$DEST"

oras restore ./backup/acme-20260626.tar --to "$DEST/snapshots/acme"
```

Confirm that the referrers survived:

```bash
oras discover "$DEST/snapshots/acme" --digest sha256:abc123...
```

Point the broker and the workers at the new registry, then run a test restore lease against a known digest. See [Restore from a snapshot](./restore-from-snapshot.md). If the KV records still carry the old registry host in `OciArtifactRef.registry`, update the allowlists or publish the digests again.

## Periodic backup with the Helm chart

The sandbox chart at `devops/helm/charts/trogon-sandbox` ships a CronJob (`templates/cronjobs/snapshot-backup.yaml`). It renders when `backup.enabled` is true and `backup.repositories` is not empty. It runs `oras backup` for each listed repository and writes tarballs to the PVC named by `backup.storage.pvcName` (default `sandbox-backup`).

```bash
helm upgrade sandbox ./devops/helm/charts/trogon-sandbox \
  --reuse-values \
  --set 'backup.repositories={snapshots/acme}' \
  --set backup.schedule='0 3 * * *'
```

`backup.plainHttp` defaults to true for the bundled Zot registry. Set it to false for a registry behind TLS. Tarballs land in `/backup` on the volume, named `snapshot-backup-<timestamp>.tar.<repository>`.

The `mise run sandbox:dr-drill` task rehearses the recovery. It tears down the bundled Zot, restores from the latest backup and checks that a snapshot is queryable. Set `TEST_REPO` to a repository listed in `backup.repositories`.

## Periodic backup from an external scheduler

For clusters that do not use the chart, schedule these on your operations platform:

1. Nightly: `oras backup` each production repository to a dated tarball in object storage with server-side encryption.
2. Weekly: replicate the tarballs to a second region.
3. Quarterly: run `oras restore` into an isolated registry and run one restore lease end to end.
4. Retention: keep 30 daily and 12 monthly tarballs. Align with registry-side garbage collection so a backup captures referrers before untagged manifests are reaped.

Alert on backup job failure.

## Security notes

- ORAS does not encrypt backup tarballs. Encrypt the object-store bucket, or sign the tarball with Cosign after the backup if policy requires it.
- Treat backup storage as sensitive as the live registry. A poisoned backup is a poisoned warm start.
- Restrict restore credentials to break-glass roles.

## See also

- [Snapshot store](../../architecture/sandbox/snapshot-store.md) for the live digest set.
- [Volumes](../../architecture/sandbox/volumes.md): volume disaster recovery is separate.
- [Publish a snapshot](./publish-a-snapshot.md).
- [Restore from a snapshot](./restore-from-snapshot.md).
- [Media types](../../reference/sandbox/media-types.md).
- [Deploy the stack](./deploy.md) for the Helm install.
