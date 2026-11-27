# Cleanup authority and receipt retention

Cancellation and cleanup records prevent delayed requests from creating duplicate work or deleting an unrelated resource. Their lifetime is separate from command output and operation-result readability.

## Retention policy

Keep cleanup authority and replay fences for as long as their identity namespace accepts requests. There is no automatic age-based garbage collection for these records. Do not configure bucket expiry or evict authority records to reclaim storage while their operation, lease or launch identities remain admissible.

| Record | Required retention and settlement |
| --- | --- |
| Cancellation received before admission | Retain the exact cancellation fence so delayed or replayed admission remains refused after restart. |
| Broker cleanup `Requested` or `ProviderConfirmed` | Retain exact allocation ownership and recovery authority until provider disposal and lease, quota, name, drive and monitoring settlement are durably complete. |
| Broker cleanup `Settled` | Retain the settled disposition and replay fence. The active-recovery marker can be removed. Removing it does not authorize deleting the receipt. |
| E2B lease cleanup `Cleanup` | Retain the exact tenant, lease, canonical sandbox identity and claim evidence until the sandbox's absence is durably recorded. A delete acknowledgement or an expired claim is insufficient. |
| E2B lease cleanup `Absent` | Retain absence and replay evidence. Repeating the same cleanup must not issue another delete. |
| Command launch and cancellation custody | Retain unresolved ownership and cancellation fences without expiry. A terminal disposition must remain replayable or durably refused after reconstruction. |

Claim expiry only allows another executor to retry using the retained authority. It does not prove absence, clear a cancellation, create new authority or turn an unknown result into success. Missing, corrupt, foreign or unsupported legacy records cannot authorize destructive reconstruction or a duplicate launch.

The operation retention value, `OPERATION_RETENTION` (seven days), marks when completed results may become eligible for a sweep. It does not expire cancellation tombstones, cleanup receipts or replay fences. The command launch reply journal has a separate 25-hour expiry (`LAUNCH_KEY_MAX_AGE`). New launch authority must not rely on that expiry for safety. Command-output retention and `retention.monitoring` do not govern cleanup authority.

## Broker command custody storage

With `--idempotency-backend=nats-kv` (the default, also set by `SANDBOX_IDEMPOTENCY_BACKEND`), the broker derives its launch namespaces from `--idempotency-bucket` or `SANDBOX_IDEMPOTENCY_BUCKET`, whose default is `sandbox-idempotency`:

| Namespace | Default bucket | Retention |
| --- | --- | --- |
| `<idempotency-bucket>-launches` | `sandbox-idempotency-launches` | Command replies expire after 25 hours. |
| `<idempotency-bucket>-launches-custody` | `sandbox-idempotency-launches-custody` | Immutable cancellation-authority reservations have no expiry or automatic forget. |

The custody bucket is provisioned lazily when a new authority reservation is requested. Reads do not create it. Its defaults are file-backed limits storage, one record per key, 64 MiB total capacity and at most 16 KiB per record. Records contain bounded exact ownership, launch-key and fingerprint identities. Retiring, deleting or expiring a reply does not remove its custody reservation, and another reservation returns the existing authority rather than replacing it.

Each authority operation checks the live storage policy, including cached handles. The bucket must remain file-backed with limits retention, zero maximum age, no message TTL, discard-new behavior, one record per key, a positive byte limit and a message-size limit no greater than 16 KiB. The stream must remain writable and acknowledge writes. Mirrors, sources, subject transforms, republishing and expiring deletion markers are refused, and the bucket's subject namespace must remain unchanged. The broker refuses an unsafe or unreadable policy without rewriting it. Capacity exhaustion rejects new reservations instead of evicting retained authority, and it must not authorize new effects.

Preserve the custody namespace and its contents across restarts and migration. Deleting, purging or recreating it while launch identities remain admissible is unsupported, because lazy provisioning cannot distinguish a deleted namespace from its first creation. The namespace-retirement requirements below still apply.

## Storage and recovery

Persist and back up broker operations (default bucket `sandbox-operations`) and provider custody together. Preserve key namespaces, tenant bindings, exact allocation and assignment identities, and replay fences during migration. Ownership records must contain only the bounded identities and dispositions needed for recovery, without command arguments, environment values or provider credentials.

Provision storage for retained authority. Storage exhaustion must refuse new effects or retain an explicit unresolved outcome. It must not evict unresolved evidence. E2B lease-cleanup storage (bucket `sandbox-e2b-lease-cleanup`) checks the live JetStream configuration for every receipt operation, including existing buckets and cached store handles. It requires file-backed limits retention, no maximum age or per-message expiry, and discard-new behavior when capacity is reached. Mirrors and sources are refused. An unsafe or unreadable configuration blocks cleanup before native deletion and leaves the retained receipt unchanged. Restoring a safe configuration allows recovery from that receipt, and a retained absence receipt still prevents another delete on replay.

There is no supported online receipt-purge procedure. Retiring a namespace requires disabling its admitting clients and writers, draining or durably refusing delayed traffic, reconciling every unresolved resource and accounting effect, and preserving the required receipts and backups. Reusing the old namespace or identities would invalidate that retirement proof.

## Requirements for future compaction

A compactor must replace full records with equivalent durable refusal or settlement fences before removing authority. Its transitions must be revision fenced so a crash or concurrent completion preserves either the full receipt or its replacement.

Reconstruction tests must prove that cancel-before-launch remains canceled, an old launch key cannot create duplicate effects, delayed provider completion cannot revive a retired identity, and quota, refund, name and drive settlement remains idempotent. Until these conditions are implemented and verified, automatic authority compaction remains unsupported.

## Related

- [Data retention controls](../../architecture/sandbox/data-retention.md)
- [E2B runtime and cleanup custody](./e2b-runtime.md)
- Broker operation records: `rsworkspace/crates/sandbox/trogon-sandbox-broker/src/operations.rs`
- Broker command launch storage: `rsworkspace/crates/sandbox/trogon-sandbox-broker/src/command_launch.rs`
- E2B lease cleanup receipts: `rsworkspace/crates/sandbox/trogon-sandbox-e2b-worker/src/lease_cleanup.rs`
