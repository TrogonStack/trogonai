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
reports `stuck`.

So a credential in `pending_write` for a few seconds is normal. One stuck
for half an hour means the recovery worker cannot fix it, which is a
different problem from the write having failed.

## Check the recovery worker first

```bash
curl -H "x-trogon-admin-token: ${ADMIN_TOKEN}" \
  "${GATEWAY}/-/credentials/recovery/status"
```

```json
{
  "last_scanned_sequence": 0,
  "next_scan_sequence": 0,
  "consecutive_failure_count": 0,
  "first_failure_unix_seconds": null,
  "retry_after_unix_seconds": null,
  "status": "idle"
}
```

`status` is one of `idle`, `retry_delayed` and `stuck`, and the gateway computes
it on each request, from the checkpoint and the clock. `stuck` wins over
`retry_delayed`, which wins over `idle`. There are no separate boolean fields.

Read it in this order:

**`status: "stuck"`**: the worker has been failing for longer than the 30
minute threshold. This is the page-worthy state. Go to "when the worker is
failing" below.

**`status: "retry_delayed"` with a low `consecutive_failure_count`**: the
worker hit a transient error and is backing off. Check again after
`retry_after_unix_seconds`. Not actionable yet. Once that time passes the
status reads `idle` until the next pass runs, so a non-zero
`consecutive_failure_count` or a non-null `first_failure_unix_seconds` next to
`idle` means failures are not cleared yet, not that the worker is healthy.

**`next_scan_sequence` not advancing across polls, everything else clean**:
the worker is running and finding nothing new. If a credential is
nonetheless stuck, the worker never saw its event: see "when the worker is
healthy but the credential is stuck".

**`consecutive_failure_count` climbing while the checkpoint stays pinned**:
each pass is failing at the same sequence. Something about that specific
event or credential is unprocessable. A failing pass does not stop at the
first problem: it recovers every other credential, keeps the checkpoint
pinned, and logs a warning for each cause with the underlying error.
`credential recovery found an invalid credential event` carries the
`stream_sequence` of the bad event. `credential recovery replay failed`,
`credential recovery planning failed`,
`credential write activation recovery failed` and
`credential rotation activation recovery failed` carry the `credential_id`.
`error = ...` on those lines is the actual cause and everything above is just
its shadow. `credential recovery pass failed` is logged only when a pass
cannot read the credential event stream or its checkpoint. Such a pass
records no failure, so it does not move `consecutive_failure_count`.

## When the worker is failing

The recovery pass touches three systems, and the error message says which:

**OpenBao metadata reads.** The worker calls `SecretStoreMetadata` to decide
whether the material actually landed. An unreachable or sealed OpenBao
fails every pass. Check `bao status`; see
[unseal-and-key-custody.md](unseal-and-key-custody.md).

**Event stream appends.** The worker appends the activation event. A
JetStream problem, a stream at its limit, or a version conflict fails the
append. Check the credentials stream health in NATS.

**Event decode.** An event in the scan window that fails to decode, has no
event case, or carries an invalid credential reference is counted in
`invalid_events` and logged as
`credential recovery found an invalid credential event` with its
`stream_sequence`, which is where to look in the credential stream. Unknown
event types are skipped by design and counted in `skipped_events`, so an
invalid event means a genuinely corrupt payload. The pass still recovers every
other credential, but it does not move the checkpoint past the bad event and
records a failure with the usual backoff, so `status` becomes `stuck` after
30 minutes. Unlike an outage, this does not clear on its own: every retry
reads the event again. A credential whose own stream fails to read, decode or
replay is handled the same way, counted in `failed_recoveries` and logged as
`credential recovery replay failed` with its `credential_id`.

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
value. For `pending_write`, `PUT` it: a write for the same credential is
accepted again and resumes the pending write. For `pending_rotation`, repeat
the `POST` to `.../rotations` with the active version; a `PUT` is rejected
because the credential already exists.

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
