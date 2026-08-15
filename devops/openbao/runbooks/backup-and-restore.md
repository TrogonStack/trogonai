# Runbook: Backup and Restore

**Customer-visible impact of a restore:** credential material returns to the
state captured in the snapshot. Credentials created after the snapshot have
metadata in the event stream but no material in OpenBao. Credentials
revoked after the snapshot come back alive. Both are handled below; neither
is automatic.

## What a snapshot contains

A Raft snapshot contains the encrypted OpenBao storage: every credential
version, every metadata entry, policies, auth configuration. It is
encrypted by the barrier keyring, so it is worthless without the seal key
that wraps the root key. A snapshot is not a plaintext export and cannot be
read outside a cluster with the matching seal.

Corollary: **your backup strategy includes the KMS key.** A snapshot with a
destroyed seal key is unrecoverable. See
[unseal-and-key-custody.md](unseal-and-key-custody.md).

## Take a snapshot

Take it from a follower, not the leader, so the leader keeps serving:

```bash
bao operator raft snapshot save openbao-$(date -u +%Y%m%dT%H%M%SZ).snap
```

Store snapshots encrypted at rest, in a different account or project from
the cluster, with a retention window long enough to survive a slow-detected
compromise. A snapshot rotation shorter than your detection time means the
only snapshots you keep are the poisoned ones.

Schedule this. An unscheduled snapshot is a snapshot taken the day before
the incident, six months ago.

## Restore

```bash
bao operator raft snapshot restore openbao.snap
```

Restore is destructive: it replaces cluster state. Never run it against a
cluster you have not confirmed is the one you meant.

The restoring cluster must be able to unseal, which means the same seal
configuration and a reachable KMS key. Restoring into a cluster with a
different seal does not work; that path is seal migration, not restore.

## Reconcile with the event stream after a restore

OpenBao holds material. The credential event stream holds metadata. A
restore moves one and not the other, so they disagree until you reconcile.

Two disagreements, opposite in shape:

**Metadata exists, material does not.** A credential created after the
snapshot. The event stream says active; OpenBao has nothing at that path.
The gateway will fail to resolve it, and
`gateway.credential.resolve.failures` will count each attempt.

Resolution: the material is gone. It cannot be recovered from the event
stream, which by design never contains plaintext. The credential must be
re-supplied by whoever owns it, through
`PUT /-/credentials/{source}/.../{secret}`. Treat this as a rotation the
customer did not ask for, and tell them.

**Material exists, metadata says revoked.** A credential revoked after the
snapshot. The restore resurrected material that should be gone.

Resolution: re-issue the revocation. This is the dangerous direction, and
it is the one to check first after any restore:

```bash
# For each credential the event stream marks revoked or destroyed,
# confirm OpenBao agrees.
bao kv metadata get -mount=secret \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

A restored-but-revoked credential is a live secret the customer believes is
dead. Reissuing the DELETE against the management API re-revokes it in
OpenBao and re-records the fact.

**Then force a projection refresh** so the gateway sees the reconciled
state rather than its cache. See
[gateway-projection-miss.md](gateway-projection-miss.md).

## The restore drill

The acceptance criterion is not "we take snapshots." It is "a restore has
been proven to reconcile with the event stream." Run this on a schedule,
into a scratch cluster, never into production:

1. Take a snapshot of a non-production cluster with known contents.
2. Create one credential and revoke another **after** the snapshot, so the
   drill exercises both disagreement directions.
3. Restore the snapshot into a scratch cluster with the same seal
   configuration.
4. Confirm the cluster unseals without human intervention.
5. Walk the event stream and confirm you can detect both disagreements
   using the checks above.
6. Reconcile both. Confirm the gateway resolves the re-supplied credential
   and refuses the re-revoked one.
7. Record how long steps 3 through 6 took. That number is your recovery
   time, and it is the only honest one.

A drill that skips step 2 proves only that the tarball is readable.
