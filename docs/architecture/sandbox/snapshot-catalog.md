# Snapshot catalog, keep-N retention and the orphan sweep

The broker runs two background passes over published snapshots. This page explains what each prunes, what neither may touch, and how to roll them out. The code is in `rsworkspace/crates/sandbox/trogon-sandbox-broker/src/retention_sweep.rs` and `rsworkspace/crates/sandbox/trogon-sandbox-broker/src/snapshot_catalog.rs`.

## The two passes

**Tag pass** (`retention_sweep`). For each tag with a retention policy, the digests the tag used to point at are evicted when they fall outside `max_tags` or `max_age_days`. A digest that was used to start a sandbox recently counts as recent.

**Catalog pass** (`snapshot_catalog`). The catalog is the set of `SnapshotKvRecord`s the snapshot builder writes to the `sandbox-broker-kv-snapshot` bucket, one per digest, with tenant, source lease, registry location, parent and creation time. The pass:

1. Keeps the newest N snapshots of each (tenant, source lease) and deletes the rest from the registry, then removes their records and publishes `SnapshotDeleted`. If the registry delete fails the record stays, so the next pass retries.
2. Finds orphans: artifacts the registry holds that the catalog has no record of. The builder publishes the artifact before it writes the record, so an orphan is deleted only after it has been seen without a record on passes at least the grace period apart.

## What is never pruned

Both passes skip a digest when:

- a lease that is not `Released` or `Failed`, or any lease with a name, was restored from it (`restored_from`) or holds it as its remote dormancy artifact,
- a snapshot tag currently points at it,
- it is an ancestor (by `parent`) of a digest protected for either reason,
- for keep-N only, its record has no creation time. It counts toward N but is not deleted, because its age is unknown.

If the active leases or the tags cannot be read, the pass skips the tick rather than treating an unreadable set as an empty one.

## Tenants

The tag pass sweeps the tenants listed in `SANDBOX_RETENTION_SWEEP_TENANTS`. Tag keys carry the escaped tenant before the first `_` (see [KV key layout](./kv-key-layout.md)), so the store can list tenants itself. With `SANDBOX_RETENTION_SWEEP_DISCOVER_TENANTS=true`, the listed tenants and every tenant that owns a tag are swept. The catalog pass always reads tag pins for every tenant the store knows, because the catalog spans all tenants.

## Flags

| Flag | Default | Effect |
|---|---|---|
| `SANDBOX_RETENTION_SWEEP_TENANTS` | empty | Tenants for the tag pass. |
| `SANDBOX_RETENTION_SWEEP_DISCOVER_TENANTS` | `false` | Add every tenant found in the tag store. |
| `SANDBOX_SNAPSHOT_KEEP_PER_LEASE` | `0` (off) | Snapshots kept per tenant and source lease. |
| `SANDBOX_SNAPSHOT_ORPHAN_SWEEP` | `false` | Run the orphan pass, listing artifacts from the OCI registry. |
| `SANDBOX_SNAPSHOT_ORPHAN_GRACE_SECS` | `86400` | How long an artifact lacks a record before it is an orphan. |

The passes run on the retention tick, `SANDBOX_RETENTION_TICK_MS` (default 300000).

## How the registry is listed

Registries often lack a catalog endpoint, so the lister uses the distribution tag list. Every snapshot push leaves a staging tag `sandbox-artifact-sha256-<hex>`. The lister reads those tags back into digests and keeps only manifests whose artifact type is a snapshot type. Referrers such as an SBOM share the tag scheme and are skipped, as is any manifest whose type cannot be read. A repository whose tags cannot be read is skipped for that pass. Pagination is bounded and follows only same-origin next links.

## Rollout

Defaults change nothing. Both behaviors delete data, so roll them out in steps:

1. Turn on `SANDBOX_RETENTION_SWEEP_DISCOVER_TENANTS` on one broker. Tag retention policies then apply to tenants that were never listed. Check the broker's retention sweep logs for evictions before widening.
2. Set `SANDBOX_SNAPSHOT_KEEP_PER_LEASE` to a large number first (for example 50) and watch what the catalog sweep prunes, then lower it.
3. Set `SANDBOX_SNAPSHOT_ORPHAN_SWEEP=true`. The broker then lists artifacts from the registry. Leave the grace at its default until the catalog sweep logs show only artifacts you expect.

Several brokers may run the passes at once. A repeated delete of an artifact that is already gone is harmless, and each pass re-reads the leases and tags it protects with.

## Not covered

- Repositories nobody points to. The lister reads the tag list of each repository named by a catalog record, plus the current and legacy repository of every catalog tenant and every tenant in `SANDBOX_RETENTION_SWEEP_TENANTS`. A tenant with no catalog record and not in that list is not scanned.
- Native (provider-held) snapshots in the `sandbox-native-snapshots` bucket.
