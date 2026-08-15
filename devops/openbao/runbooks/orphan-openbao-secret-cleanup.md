# Runbook: Orphan OpenBao Secret Cleanup

Material exists in OpenBao at a managed path with no corresponding active
credential in the event stream. The reverse of a projection miss: the
platform has forgotten about a secret that still exists.

**State: no cleanup worker exists.** The `lifecycle_worker_cleanup` policy in
[../policies](../policies) grants the `list` capability specifically so a
reconciliation job can enumerate managed paths, and that job has not been
written. There is also no `SecretStoreList` trait; the secret store exposes
put, get, rotate, revoke, metadata, and destroy, and nothing that walks the
tree. Everything below is manual.

**Customer-visible impact:** none. Nothing resolves an orphan, because
resolution goes through the projection and the projection does not know it
exists. The impact is that live credential material persists past the point
where anyone is accounting for it, which is a compliance and blast-radius
problem rather than an availability one.

## How orphans arise

**A write that landed with an activation that did not, and was never
recovered.** The recovery worker repairs these while they are in its scan
window. One that ages past the checkpoint is an orphan. See
[stuck-pending-secret-write.md](stuck-pending-secret-write.md).

**A restore that rolled the event stream and OpenBao to different points.**
The direction where OpenBao is ahead produces orphans directly. See
[backup-and-restore.md](backup-and-restore.md).

**Direct writes to OpenBao.** Break-glass, or an operator bypassing the
management API. The event stream never learns about it. See
[break-glass-access.md](break-glass-access.md).

**A revocation that removed the metadata trail but not the material.** Should
not happen through the API, which revokes by soft-deleting every version and
records it, but a partial manual delete produces exactly this.

## Manual reconciliation

Enumerate the managed tree with an identity holding list capability, then
compare against the event stream. The tree is three levels below the prefix:

```bash
bao kv list -mount=secret trogonai/
bao kv list -mount=secret trogonai/{owner_id}/credentials/
```

For each path found, the credential id is the last segment, and it encodes
`openbao:{owner}:{scope_key}:{kind}`. Look that credential id up in the
credential event stream.

Three outcomes:

**Active in the stream.** Not an orphan. Expected state.

**Revoked or destroyed in the stream, versions still live in OpenBao.** This
is the dangerous kind: material the platform believes is gone. Re-issue the
revocation through the management API so the event stream records the second
revocation, rather than deleting in OpenBao directly.

**Absent from the stream entirely.** A true orphan. Confirm before deleting:
a credential absent from the stream because the stream was rolled back is
recoverable material for a customer who still expects it to work, and
deleting it turns a bookkeeping problem into data loss. When the path is
confirmed unaccounted for, soft-delete it first and destroy only after a
retention interval.

```bash
bao kv metadata get -mount=secret \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

Read the metadata before deciding. Creation and update times distinguish a
recently orphaned write from something that has been sitting for months.

## What the worker would need

Recording this so the eventual implementation does not have to rediscover it:

- A `list` operation on the secret store. The trait does not exist and the
  OpenBao client does not expose one.
- A stream-side index of active credential ids, or a full replay per
  reconciliation pass. A per-path stream query does not scale to a tree walk.
- Soft-delete before destroy, with a retention interval, so a reconciliation
  bug is recoverable. A worker that destroys directly on first disagreement
  will eventually destroy a live credential during a stream lag.
- Alerting on backlog size rather than on individual orphans. One orphan is
  bookkeeping; a growing count is a systemic disagreement between the two
  stores, and the count is the signal. There is no instrument for this today,
  which is why [alerts.md](alerts.md) lists the orphan backlog alert as
  having no signal.
