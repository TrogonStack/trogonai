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
compare against the event stream. Every managed path is
`trogonai/{owner_id}/credentials/{credential_id}`:

```bash
bao kv list -mount=secret trogonai/
bao kv list -mount=secret trogonai/{owner_id}/credentials/
```

The last segment of each path is the credential id. It is the same opaque id
that names the credential's subject in the `CREDENTIAL_EVENTS` stream,
`credentials.events.v1.{owner_id}.{credential_id}`, where a `.` in the owner id
appears as `~`. Replay that subject to the credential's current state, from
the `CredentialWriteRequested` event that recorded its owner, scope key and
kind. Replay it even when a historical activation or rotation is on record: a
destroyed credential can be written again at the same path, and the historical
event does not account for the new write. A write request alone does not
settle it:

- **`pending_write`.** A write awaiting activation, not an orphan. Check the
  recovery state (`${GATEWAY}/-/credentials/recovery/status`) and leave it to
  the recovery worker, or follow
  [stuck-pending-secret-write.md](stuck-pending-secret-write.md) when it is
  `stuck` or the request predates the checkpoint.
- **`write_failed`.** The put can land in OpenBao even though the gateway
  recorded a failure, for example when the response was lost. The recovery
  worker never retries this state; only a new `PUT` for the same credential
  moves it forward. If the owner still wants the credential, have them
  re-supply it. Reconcile versions after that retry as well: a successful retry
  can activate a new version while the failed attempt's version remains live.
  Otherwise investigate the failed attempt's material as unaccounted for,
  using the version-specific cleanup procedure below.

Compare the version numbers in OpenBao metadata with activation and rotation
versions throughout the credential's stream, including its current state.
After a successful retry, preserve the active version and all other versions
accounted for by the stream. Investigate each additional version separately;
an active credential does not establish that every version at its path is
accounted for. If stream history is incomplete or a pending operation could
still activate the version, defer deletion until that uncertainty is resolved.

Before any management-API or direct cleanup mutation, establish the write
quiescence described in [Quiesce writes and clean up exact versions](#quiesce-writes-and-clean-up-exact-versions).
The current state determines the next step:

**Active in the stream.** Its recorded active version is expected. Any other
accounted versions remain governed by their lifecycle state. Unaccounted
versions at this same path still need reconciliation and, when confirmed
orphaned, the cleanup procedure below.

**Revoked or destroyed in the stream, versions still live in OpenBao.** This
is the dangerous kind: material the platform believes is gone. Re-issuing the
revocation does nothing. When the stream already says revoked, the revoke
handler returns success without calling OpenBao, and when it says destroyed,
revoke is rejected because only an active or rotation-pending credential can
be revoked. The live versions stay live. No cleanup job or path exists to do
this work, so it is manual, and it depends on the state the stream records:

- **Revoked, `cleanup_failed`, or `destroy_requested`.** Request the
  destruction through the management API so the second removal is recorded in
  the event stream. Use the version from the stream and a reason that names
  the incident or reconciliation pass:

  ```bash
  curl -X POST \
    -H "x-trogon-admin-token: ${ADMIN_TOKEN}" \
    -H "Idempotency-Key: $(uuidgen)" \
    -H "Content-Type: application/json" \
    -d '{"owner_id":"{owner_id}","version":{version},"reason":"{reason}"}' \
    "${GATEWAY}/-/credentials/{source}/{integration_id}/{secret}/destructions"
  ```

  It destroys every version at the path in OpenBao. If it fails, the stream
  records `cleanup_failed` and the same request can be repeated.
- **Destroyed.** The destroy request returns the existing destroyed state
  without calling OpenBao, so the API cannot reach the material. Destroy the
  confirmed leftover versions directly under `break_glass_admin`
  ([break-glass-access.md](break-glass-access.md)). Hold the write quiescence
  described below, record the exact versions and recheck the stream before
  destruction. A new write at the same path must not be destroyed. Record
  the reconciliation as the evidence, since the stream has nothing new to say:

  ```bash
  bao kv destroy -mount=secret -versions="${ORPHAN_VERSIONS}" \
    "trogonai/{owner_id}/credentials/{credential_id}"
  ```

**Absent from the stream entirely.** A true orphan. Confirm before deleting:
a credential absent from the stream because the stream was rolled back is
recoverable material for a customer who still expects it to work, and
deleting it turns a bookkeeping problem into data loss. When the path is
confirmed unaccounted for, soft-delete only its confirmed orphan versions
first and destroy only after a retention interval.

```bash
bao kv metadata get -mount=secret \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

Read the metadata before deciding. Creation and update times distinguish a
recently orphaned write from something that has been sitting for months.

### Quiesce writes and clean up exact versions

Before any cleanup mutation, quiesce every writer for this credential path,
including management requests, retries, lifecycle/recovery workers and direct
operator writes. Stop or deny those writers and wait for in-flight writes to
settle. There is no cleanup lock supplied by this runbook; if exclusive write
quiescence cannot be established, do not delete or destroy anything. Keep it
in effect for every cleanup state check and mutation, including management-API
destruction, which removes every version at the path.

Under that quiescence, reread the credential stream and OpenBao metadata.
Record the stream position, current state, credential id and exact version numbers
confirmed orphaned in the incident record. Set `ORPHAN_VERSIONS` to that
nonempty comma-separated list. Recheck the state and version accounting
immediately before deletion; if either changed or quiescence was lost, stop
and reconcile again. Never use a path-only delete, which selects the latest
version and can remove material from a successful retry.

```bash
bao kv delete -mount=secret -versions="${ORPHAN_VERSIONS}" \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

After the retention interval, establish write quiescence again if it was
released. Replay the current stream and reread metadata before any irreversible
action. Confirm each selected version is still unaccounted for and no pending
operation can activate it; preserve active and other accounted versions. If
that cannot be confirmed, leave the material recoverable and investigate.
Destroy only the recorded versions that still qualify, and retain the checks
and outcomes as evidence:

```bash
bao kv destroy -mount=secret -versions="${ORPHAN_VERSIONS}" \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

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
