# Runbook: Cleanup Worker Failure

**State: the cleanup worker does not exist.** There is nothing to fail and
nothing to restart. This file exists so the gap is visible rather than
inferred from a missing runbook, and so the eventual worker inherits a
statement of what its failure means.

Do not confuse this with the **recovery worker**, which does exist, does run,
and has its own failure procedure in
[stuck-pending-secret-write.md](stuck-pending-secret-write.md). The recovery
worker completes interrupted activations. The cleanup worker would reconcile
OpenBao against the event stream and remove orphaned material. Different
jobs, different failure meanings.

## What its failure would mean

Nothing customer-visible, immediately. A cleanup worker that stops does not
break resolution, does not break provisioning, and does not break revocation
through the API. Credential material simply accumulates past the point where
the platform accounts for it.

The consequence is deferred and compounding:

- Orphaned material stays live in OpenBao indefinitely. The blast radius of a
  future OpenBao compromise grows with the backlog.
- Revocations that failed to propagate to OpenBao stay unpropagated, which
  means a credential the customer was told is dead is still live. This is the
  case that turns a silent worker into a security incident.
- The backlog is the only signal, and it grows slowly enough that a worker
  down for weeks looks the same as one that had nothing to do, unless the
  backlog is measured separately from the worker's liveness.

That last point is the design constraint worth carrying forward: **liveness
and backlog must be separate signals.** A worker reporting healthy passes
while its backlog grows is the failure mode that a single "worker is running"
alert cannot see.

## Interim procedure

Reconcile by hand, on a schedule, using
[orphan-openbao-secret-cleanup.md](orphan-openbao-secret-cleanup.md). That
file has the enumeration commands and the three-outcome decision.

Manual reconciliation is not equivalent to the worker. It is unscheduled by
default, it does not measure a backlog, and it is performed by an identity
with more capability than the worker would hold. Treat it as a stopgap that
should be visibly logged each time it runs, so the interval between runs is
itself a known number.

## When the worker is built

- It must not be able to undelete. `lifecycle_worker_cleanup` denies undelete
  deliberately: reversing a revocation is attended break-glass work, not
  something an unattended job does after a disagreement.
- It must not be able to read credential data. The policy grants metadata
  only. Cleanup decides from metadata and never needs plaintext, and a worker
  that can read every credential is a much larger target than one that
  cannot.
- Its backlog needs an instrument. [alerts.md](alerts.md) lists the orphan
  cleanup backlog alert as having no signal today; that is the instrument to
  add alongside the worker, not after it.
