# Runbook: Gateway Projection Miss

A credential exists and is active in the event stream, but the gateway
cannot resolve it at request time.

**Customer-visible impact:** the integration fails. Which failure the caller
sees tells you where in the chain it broke, so read the error before doing
anything else.

## Read the error first

`RuntimeCredentialError` distinguishes four situations that look identical
from the outside. The gateway maps all of them to `401` for the caller, so
the distinction is in the gateway logs, not the response.

**`IntegrationNotFound`**: the projection has no entry for this
integration key at all. The projection never saw the credential, or saw it
and removed it. This is the classic projection miss; go to "the projection
is behind" below.

**`IntegrationNotResolvable`**: the projection has the integration, and its
status is not resolvable. The projection is working correctly and telling
you the credential is suspended or revoked. This is not a projection
problem. If the customer believes it should be active, the disagreement is
in the event stream, not the cache.

**`CredentialMissing`**: the integration resolves, but not for this kind.
The projection has, say, a signing token and the caller asked for a bot
token. Either the credential was never supplied for that kind, or the
caller is asking for the wrong one.

**`DeliveryDenied`**: the credential exists and is active, and the delivery
policy refused this request: an unlisted host, an unauthorized runtime
service, or a disallowed injection location. This is enforcement working. It
is deliberately indistinguishable from "no such credential" to the caller,
so the gateway log is the only place the reason appears. Check the
`gateway.credential.delivery.denied` counter's `reason` label against what
the caller was doing.

**A store error**: the projection and policy both passed, and the read
against OpenBao failed. `gateway.credential.resolve.failures` counts these.
Check `bao status` and
[unseal-and-key-custody.md](unseal-and-key-custody.md).

## The projection is behind

The runtime projection is built from the credential event stream by a
checkpointed refresh worker. Each pass reads events after the checkpoint,
applies them to projections, and advances the checkpoint. A projection miss
for a credential that definitely exists means one of:

**The refresh worker is not running or is failing.** Its passes log under
the credential processor. A worker down since before the credential was
created explains the miss exactly.

**The checkpoint is stale relative to the credential's event.** Same
symptom, different cause: the worker is alive but not making progress. The
checkpoint sequence should track the stream's last sequence closely.

**The event was published but the projection derived nothing from it.** An
event kind the projection does not handle, or a state that does not produce
a resolvable projection, is skipped silently. This is correct behavior for
unknown kinds and a bug for known ones.

**The credential is genuinely not active.** Confirm from the event stream
before treating this as a projection problem. A credential still in
`pending_write` is not a projection miss; see
[stuck-pending-secret-write.md](stuck-pending-secret-write.md).

## Force a refresh

The projection rebuilds from the stream. Restarting the gateway drops the
in-memory projection and cache, and the refresh worker rebuilds from its
checkpoint. That is the blunt instrument and it is usually the right one:
the projection holds no authoritative state, so nothing is lost by
discarding it.

It does not help if the checkpoint itself is the problem, since a restart
resumes from the same checkpoint. Rewinding the checkpoint replays the
stream from an earlier sequence and rebuilds the affected projections.
Replay is idempotent by construction: applying an event twice produces the
same projection state.

## Cache versus projection

Two layers, and they fail differently:

**The projection** maps an integration key to credential references and a
delivery policy. It has no secret material. It is rebuilt from the stream.

**The cache** maps a credential reference to resolved material, with a 300
second TTL plus per-credential jitter of up to 30 seconds
([ADR#0049](../../../docs/adr/0049-revocation-latency-target.md)). It is
invalidated explicitly on revocation.

A stale *cache* serves a credential that should be gone. A stale
*projection* fails to serve a credential that should be there. The first is
a security concern with a bounded window; the second is an availability
concern with no bound until the projection catches up.

`gateway.credential.cache.hits` and `gateway.credential.cache.misses`
distinguish them. A miss rate near 100% for an integration that is serving
traffic means the cache is being invalidated or never populated, not that
the projection is missing: a missing projection produces no cache activity
at all, because the denial happens before the store read.

## After a restore

An OpenBao restore reintroduces both disagreement directions and the
projection will faithfully serve whichever the event stream describes. Do
the reconciliation in
[backup-and-restore.md](backup-and-restore.md) first, then force the
refresh. Refreshing before reconciling just rebuilds a projection of the
unreconciled state.
