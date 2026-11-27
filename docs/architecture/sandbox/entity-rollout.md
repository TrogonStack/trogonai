# Persistent sandbox entity: modes, soak plan and intent reconciler

This page explains how the broker's persistent sandbox entity store is rolled out, why the default mode is `shadow` and not `on`, and what the intent reconciler does in `on` mode. For the verbs and error kinds see [Client NATS permissions](../../reference/sandbox/client-nats-permissions.md).

## What exists

`SANDBOX_ENTITY_MODE` (or `--entity-mode`) has three values:

| Mode | Entity store | Responses | Delete verb |
| --- | --- | --- | --- |
| `off` | Not created. | Legacy name index and active-lease store only. | `entity_not_enabled` |
| `shadow` (default) | Mirrors named persistent leases, repaired by a periodic sweep. | Unchanged. An entity failure is logged and swallowed. | `entity_not_enabled` |
| `on` | Source of truth for named persistent sandboxes. | An entity failure fails the request with `entity_unavailable`. | Served |

The mirror is `rsworkspace/crates/sandbox/trogon-sandbox-broker/src/entity_shadow.rs` and the store is `entity_store.rs` in the same directory. The entity moves `Stopped`, then `Starting{lease}`, then `Running{lease}`, then back to `Stopped`, and is written before the lease is dispatched. The repair sweep reconciles observed state against the active-lease store. A bound entity whose lease is gone for longer than the pending timeout returns to `Stopped`, a `Starting` entity next to a running lease advances, and a name claim without an entity is removed.

## Why the default is `shadow`

`on` changes failure behavior: a broker with a broken entity bucket would start refusing named leases. `shadow` is safe by construction (a mirror that cannot start leaves the broker running without it, and no response depends on it), and it produces the evidence needed to flip. `off` remains available for a deployment whose NATS account cannot create the entity buckets or stream.

## Soak plan

1. Run `shadow` on every environment for at least two weeks, including a deployment that restarts brokers and workers during the window.
2. Watch these signals. Each must stay at zero (or explainable) for the last seven days:
   - `sandbox entity shadow write failed` warnings, by `op`.
   - `RepairReport.runs_ended` per sweep. A steady non-zero value means releases are not reaching the entity hooks, so find the path before flipping.
   - Entities in `Starting` or `Running` whose lease is not in the active-lease store, and the reverse (a running named lease with no entity).
   - `sandbox entity event publish failed` warnings.
3. Compare, for a sample of tenants, the legacy name index against `sandbox.entity.list`. Every named persistent lease has one entity, and the entity's persistence tag head matches the snapshot tag.
4. Exercise failure on a staging broker in `on` mode: stop the entity bucket and confirm named lease requests answer `entity_unavailable` and succeed on retry with the same idempotency key once it returns.

## Flip plan

1. Flip one staging broker to `on`, run the verification suites, and keep it for a week.
2. Flip production brokers one replica at a time. Mixed `shadow` and `on` replicas are safe because `on` only adds strictness on the request path and both write the same records.
3. Rollback is changing the flag back to `shadow`. Entities already created stay in the store, and the legacy name index was never stopped, so named sandboxes keep resolving. Do not use `off` as a rollback once entity delete has been used, because a deleted entity's lease and snapshots are already gone.

## Reconciler for recorded intent

The broker runs a reconcile pass on every entity sweep tick, but only in `on` mode. In `shadow` and `off` it does nothing, because the legacy stores decide there. `SANDBOX_ENTITY_RECONCILE=false` turns it off even in `on`. The default is true.

The pass acts only on intent the record already carries, and drives the existing durable operations:

- `Deleting`: finishes the delete.
- `Stopping`, lease live: releases the lease. Lease gone: ends the run.
- `Paused`, lease running: runs the pause operation with the id `rc-<entity>-<revision>-pause`, so a repeated pass replays the stored result. Lease gone: ends the run.
- `Running` with a lease a user paused: resumes it with the operation id `rc-<entity>-<revision>-resume`.
- Anything else, including `Running` over an idle-paused lease, is left alone, so a pause a user asked for is never undone. The pause verb records `Paused` first.

Leases of another tenant, or with a pause or resume already in flight, are skipped. A step that fails is counted and retried on the next tick.

### Who writes the intent

The broker's own verbs write it, before they act, in both `shadow` and `on`:

| Verb | Entity moves to | If the verb then fails |
| --- | --- | --- |
| stop lease | `Stopping` | Stays. `on` retries the release, and a repeat stop is idempotent. |
| pause lease | `Paused` | Taken back to `Running`. |
| resume lease | `Running` | Taken back to `Paused`. |
| delete (named sandbox or entity) | `Deleting` | Stays. `on` retries the delete. |

A write is skipped when the entity already holds the state, so the record revision, which the pause and resume operation ids derive from, only moves on a real change. A verb whose lease has no entity (unnamed, or created before the mode), or whose entity is bound to another lease, is untouched. If the write fails, the verb fails with `entity_unavailable` in `on` and carries on in `shadow`.

`Deleting` may follow a bound state. The entity keeps the bound lease in `current_lease` so the delete can still end that run. `Running` over a paused lease that the idle evaluator did not pause means a resume was recorded and the broker died before thawing it, so the pass resumes it. A lease the idle evaluator paused is left alone.

Restart recovery has two layers. The durable operation records re-drive a pause or resume that was recorded. The entity record covers the window before the operation exists: the intent is written first, so the reconciler finds it after the restart and runs the same operation. In `shadow` nothing reconciles, so an interrupted verb leaves its intent on the entity until the user repeats the verb, and the repair sweep does not clear it. Check for entities in `Stopping`, `Paused` or `Deleting` next to a lease that disagrees before the flip.
