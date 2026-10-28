# Runbook: Secret Leak Response

A credential's plaintext has been exposed: pasted into a ticket, committed
to a repository, logged by a provider, or extracted by an attacker.

**Customer-visible impact:** the integration using this credential stops
working the moment you revoke, and stays broken until a new value is
supplied. That is the correct trade. A leaked credential that still works
is worse than a broken integration.

## Do this first

**Revoke before you investigate.** Scoping takes time; the credential is
live the whole time you spend on it.

```bash
curl -X DELETE \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Idempotency-Key: $(uuidgen)" \
  "${GATEWAY}/-/credentials/{source}/{integration_id}/{secret}"
```

The exact path depends on the credential: `/-/credentials/discord/bot-token`,
`/-/credentials/github/{integration_id}/webhook-secret`,
`/-/credentials/gitlab/{integration_id}/signing-token`, and so on. The
management router is the authority on the path set.

Revocation soft-deletes every version in OpenBao and records the fact in
the event stream. The gateway invalidates its cache on the revocation
event; `gateway.credential.revocation.latency` measures how long that took.
The staleness bound is the cache TTL, 300 seconds plus up to 30 seconds of
jitter, so a resolution in flight may use the old value for up to that long
([ADR#0049](../../../docs/adr/0049-revocation-latency-target.md)).

Do not skip the API and delete the path in OpenBao directly. That leaves
the event stream saying the credential is active, the gateway serving it
from cache with no invalidation event, and the recovery worker with nothing
to reconcile against.

## Then revoke at the provider

Platform revocation stops the platform from using the credential. It does
nothing about anyone else who has the leaked value. For a webhook signing
secret this is mostly moot; for a bot token or an API key it is the whole
problem.

Rotate or revoke the credential in the provider's own console. This is
manual today: no provider-side revocation is implemented. See
[provider-revocation-failure.md](provider-revocation-failure.md).

## Then scope it

**How long was it exposed.** From the moment of the leak to the revocation
timestamp in the event stream.

**Who read it.** From the OpenBao audit log, every read of that credential
path in the window. See
[audit-log-export-and-review.md](audit-log-export-and-review.md). If no
audit device is enabled, this question is unanswerable, and say so in the
incident record rather than implying it was not read.

**What it could do.** A webhook signing secret lets an attacker forge
inbound events. A bot token lets them act as the integration. Scope the
blast radius from the credential kind, not from the fact that it is "a
secret."

**Whether it was used.** Provider-side audit logs, if the provider has
them. Platform logs will not show it: an attacker with the credential
talks to the provider, not to us.

## Then re-supply

```bash
curl -X PUT \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Idempotency-Key: $(uuidgen)" \
  -H "Content-Type: application/json" \
  -d '{"value":"..."}' \
  "${GATEWAY}/-/credentials/{source}/{integration_id}/{secret}"
```

The new value must be genuinely new. Re-supplying the leaked value with a
new version number leaks it again.

## Destroy, when it is warranted

Revocation soft-deletes: the versions are recoverable by an identity with
undelete. For a leak where you want the material gone from storage
entirely:

```bash
curl -X POST \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Idempotency-Key: $(uuidgen)" \
  "${GATEWAY}/-/credentials/{source}/{integration_id}/{secret}/destructions"
```

Destroy is irreversible. It removes the version's material while leaving
the metadata trail. Use it when the concern is the material persisting in
storage, not merely being served.

## Confirm

- The event stream shows the revocation, and the destruction if you ran
  one.
- `bao kv metadata get` on the credential path shows the versions
  soft-deleted (or destroyed).
- A resolve attempt against the gateway fails, rather than returning the
  old value. If it still resolves after the TTL window, the cache did not
  invalidate: see [gateway-projection-miss.md](gateway-projection-miss.md).
- `gateway.credential.revocation.latency` recorded a sample.

## Record it

The incident record includes: what leaked, how, the exposure window, the
revocation timestamp, whether the audit log could answer who read it, what
the provider-side action was, and when the replacement was supplied. If any
of those is unknown, the record says unknown. A leak response that reads as
tidier than it was is the one that teaches nothing.
