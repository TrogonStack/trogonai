# Runbook: Stuck Pending Secret Write

A credential is stuck in `pending_write` or `pending_rotation`: the write to
OpenBao was requested, but the activation event that marks it usable never
landed.

**Customer-visible impact:** the credential does not resolve. For a new
credential the integration has never worked; for a rotation the *old*
version is still active and still resolving, so the integration keeps
working on stale material. The second case is quieter and worse.

## What is supposed to happen

The recovery worker runs on an interval. Each pass reads new credential
events, loads the state of every credential they touched, and for anything
sitting in `pending_write` or `pending_rotation` it checks OpenBao metadata
and replays the activation. This is not a repair tool you invoke; it is the
normal path for a crashed or interrupted write, and it usually resolves the
condition without anyone noticing.

It keeps a checkpoint. On failure it backs off: 30 seconds initially,
doubling to a 15 minute ceiling. After 30 minutes of continuous failure it
reports `stuck_recovery`.

So a credential in `pending_write` for a few seconds is normal. One stuck
for half an hour means the recovery worker cannot fix it, which is a
different problem from the write having failed.

## Check the recovery worker first

```bash
curl -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  "${GATEWAY}/-/credentials/recovery/status"
```

```json
{
  "last_scanned_sequence": 0,
  "next_scan_sequence": 0,
  "consecutive_failure_count": 0,
  "first_failure_unix_seconds": null,
  "retry_after_unix_seconds": null,
  "retry_delayed": false,
  "stuck_recovery": false
}
```

Read it in this order:

**`stuck_recovery: true`**: the worker has been failing for longer than
the 30 minute threshold. This is the page-worthy state. Go to "when the
worker is failing" below.

**`retry_delayed: true` with a low `consecutive_failure_count`**: the
worker hit a transient error and is backing off. Check again after
`retry_after_unix_seconds`. Not actionable yet.

**`next_scan_sequence` not advancing across polls, everything else clean**:
the worker is running and finding nothing new. If a credential is
nonetheless stuck, the worker never saw its event: see "when the worker is
healthy but the credential is stuck".

**`consecutive_failure_count` climbing while the checkpoint stays pinned**:
each pass is failing at the same sequence. Something about that specific
event or credential is unprocessable. The gateway logs the underlying error
on each failed pass; `error = ...` on `credential recovery pass failed` is
the actual cause and everything above is just its shadow.

## When the worker is failing

The recovery pass touches three systems, and the error message says which:

**OpenBao metadata reads.** The worker calls `SecretStoreMetadata` to decide
whether the material actually landed. An unreachable or sealed OpenBao
fails every pass. Check `bao status`; see
[unseal-and-key-custody.md](unseal-and-key-custody.md).

**Event stream appends.** The worker appends the activation event. A
JetStream problem, a stream at its limit, or a version conflict fails the
append. Check the credentials stream health in NATS.

**Event decode.** A malformed or unknown event in the scan window fails the
plan build. Unknown event types are skipped by design and counted in
`skipped_events`, so a decode *failure* means a genuinely corrupt payload.
This is the one case where the checkpoint will not advance on its own.

## When the worker is healthy but the credential is stuck

The worker only examines credentials whose events fall in its scan window,
starting from the checkpoint. A credential whose event predates the
checkpoint is never reconsidered.

Confirm the actual state from the event stream for that credential id, then
check OpenBao directly:

```bash
bao kv metadata get -mount=secret \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

Two outcomes:

**Metadata exists in OpenBao, state says pending.** The write succeeded and
only the activation is missing. This is exactly what the worker repairs. If
it is outside the scan window, the fix is to re-drive it, not to hand-write
an activation event.

**No metadata in OpenBao, state says pending.** The write genuinely did not
land. The plaintext is gone: it existed only in the original request and is
never persisted anywhere else, by design
([ADR#0048](../../../docs/adr/0048-one-time-plaintext-exposure.md)). The
credential cannot be recovered, only re-supplied. Ask the owner for a new
value and `PUT` it.

## What not to do

**Do not write the activation event by hand.** The event stream is the
source of truth for credential state. An activation appended without
confirming the material exists produces a credential the gateway believes
is active and cannot resolve, which fails at request time instead of at
provisioning time.

**Do not write material directly into OpenBao to match the pending state.**
Same problem, opposite direction, and it bypasses the fingerprint the
metadata records.

**Do not restart the gateway to "reset" the worker.** The checkpoint is
persisted in JetStream KV; a restart resumes from the same place and
re-hits the same failure, having lost the log context that would have
explained it.
