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

The snapshot is also not the whole deployment. It holds storage, not the
server configuration, the listener TLS certificates and keys, the seal
stanza, or any plugin binaries. Back these up alongside it, versioned and
under access controls matching the snapshot's: the server configuration,
the certificates and keys, the bootstrap scripts, and an inventory of the
plugins in use with their versions. The KMS key plus a snapshot is still not
enough to rebuild a cluster without them.

## Take a snapshot

Take it from a follower, not the leader, so the leader keeps serving:

```bash
BAO_CLIENT_TIMEOUT=600s \
  bao operator raft snapshot save openbao-$(date -u +%Y%m%dT%H%M%SZ).snap
```

`BAO_CLIENT_TIMEOUT` defaults to 60s, and saving or restoring a snapshot of a
large cluster can take longer than that, so set it explicitly for both
commands. The scheduled job and the drill must fail on a non-zero exit, and
must not keep a snapshot file from a failed run: a truncated file is worse
than a missing one because it looks like a backup.

Store snapshots encrypted at rest, in a different account or project from
the cluster, with a retention window long enough to survive a slow-detected
compromise. A snapshot rotation shorter than your detection time means the
only snapshots you keep are the poisoned ones.

Schedule this. An unscheduled snapshot is a snapshot taken the day before
the incident, six months ago.

## Restore

```bash
BAO_CLIENT_TIMEOUT=600s bao operator raft snapshot restore openbao.snap
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
stream, which by design never contains plaintext. The owner has to supply it
again, and tell them: treat this as a rotation the customer did not ask for.

The management API cannot take the new value, and there is no supported
command that repairs this state:

- The ordinary `PUT` is rejected. A write is accepted only for a credential
  that is missing, in `write_failed`, or destroyed; an active one fails before
  anything reaches OpenBao, with `credential '...' already exists` in the
  gateway log and `500 credential management request failed` to the caller.
  Allowing it would also create a second logical credential.
- A rotation (`POST .../rotations`) fails. It reads the OpenBao metadata of the
  version the stream records as active, finds none, records a rotation failure
  and leaves the credential active on the same version.
- Revoke fails the same way, because it reads the OpenBao metadata first and
  finds none. Destroy is rejected while the credential is active.

The way forward is a direct, audited write of the owner's value to the exact
path and version the stream records. This is a break-glass operation
([break-glass-access.md](break-glass-access.md)) under `break_glass_admin`,
recorded in the incident, and it is warranted here because the normal path
cannot do it. Read the version the stream records for the credential, then the
current state of the path:

```bash
bao kv metadata get -mount=secret \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

The gateway reads the version the stream names, so the write has to land on
that exact version, and the secret key inside the entry must be `value`. Each
write to a path creates the next version. A path with no metadata starts at
version 1, so for a credential whose stream version is 1 a single write is
enough:

```bash
bao kv put -mount=secret \
  "trogonai/{owner_id}/credentials/{credential_id}" value=-
```

Supply the value on standard input rather than on the command line. When the
stream records a later version than the path can produce next, write once per
missing version until `current_version` equals the recorded one, then destroy
the filler versions below it with `bao kv destroy -versions=...`. Confirm with
`bao kv metadata get` that `current_version` matches before moving on. The
gateway also stamps `credential_id`, `credential_kind`, `current_version`,
`owner_id` and `created_at` as custom metadata on the path; set them with
`bao kv metadata patch -custom-metadata=...` so reconciliation tooling sees
the same shape as a gateway-written credential.

This is a gap, not a design: nothing records the repair in the event stream,
so the incident record is the only evidence. A supported restore-material
command would remove the need for it.

**Material exists, metadata says revoked or destroyed.** A credential revoked
after the snapshot. The restore resurrected material that should be gone.

Resolution: this is the dangerous direction, and it is the one to check first
after any restore:

```bash
# For each credential the event stream marks revoked or destroyed,
# confirm OpenBao agrees.
bao kv metadata get -mount=secret \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

A restored-but-revoked credential is a live secret the customer believes is
dead. Reissuing the `DELETE` does not fix it. The revoke handler sees the
credential is already revoked and returns success without calling OpenBao, so
the restored versions stay live. What works depends on the state the stream
records:

**Revoked, or `cleanup_failed`.** Request the destruction through the
management API, with the revoked version from the event stream:

```bash
curl -X POST \
  -H "x-trogon-admin-token: ${ADMIN_TOKEN}" \
  -H "Idempotency-Key: $(uuidgen)" \
  -H "Content-Type: application/json" \
  -d '{"owner_id":"{owner_id}","version":{version},"reason":"{reason}"}' \
  "${GATEWAY}/-/credentials/{source}/{integration_id}/{secret}/destructions"
```

This calls OpenBao to destroy every version at the path and records the
destruction in the event stream. If the restored path does not hold the
recorded version, the call fails with a missing-version error and the stream
records `cleanup_failed`; use the direct form below.

**Destroyed.** The destroy request returns the existing destroyed state
without calling OpenBao, so the API cannot reach the restored material.
Destroy it directly, under `break_glass_admin`, for every version up to
`current_version`:

```bash
bao kv destroy -mount=secret -versions=1,2,3 \
  "trogonai/{owner_id}/credentials/{credential_id}"
```

`bao kv delete -versions=...` soft-deletes instead, which is enough when the
material must stop being served but may stay recoverable. The stream already
holds the correct state in both cases, so the incident record is the evidence
of the direct change.

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
   configuration. Before you do, either block the scratch cluster's egress
   to provider APIs and the wider internet, or guarantee the snapshot
   contains only fake credentials. A restore brings every credential in the
   snapshot back to life, and a scratch cluster that can reach providers can
   send them to third parties.
4. Confirm the cluster unseals without human intervention.
5. Walk the event stream and confirm you can detect both disagreements
   using the checks above.
6. Reconcile both. Confirm the gateway resolves the re-supplied credential
   and refuses the re-revoked one.
7. Record how long steps 3 through 6 took. That number is your recovery
   time, and it is the only honest one.

A drill that skips step 2 proves only that the tarball is readable.
