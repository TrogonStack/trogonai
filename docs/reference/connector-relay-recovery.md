# Connector Relay Recovery

This is the target recovery contract for the authenticated HTTPS relay. It
defines required behavior for implementation and conformance testing; it does
not claim that a deployed receipt store or proxy implements it.

## Authority and storage owner

The connection proxy owns a durable request-receipt journal and completion
outbox. Its key is authenticated tenant, the grant's complete resource parent,
GrantId and invocation id. Connection custody is recorded separately. IDs are
never reused. Caller input cannot choose a tenant, store or journal epoch.

Use an acknowledged, replicated, revision-checked store. Claim uses create-only
semantics; every transition compares the receipt revision and owner fence.
Stale replicas, cache misses, a lost acknowledgement and a lease timeout never
establish absence. An authoritative conditional create decides the winner.
The adapter must prove its consistency and restoration behavior before use.
Ordinary broker duplicate windows and consumer acknowledgements do not make a
provider HTTP effect transactional. [NATS documents message deduplication and
acknowledgements](https://docs.nats.io/using-nats/developer/develop_jetstream/model_deep_dive);
the receipt lifetime and external effect require the additional rules here.

Receipts retain safe authority references, phase, timestamps, owner fence,
acceptance/completion event identities and bounded outcome metadata. They hold
no HTTP body, query, full URL, request-content digest, signature, token, raw
provider error or personal account identifier. A receipt is sensitive service
state under ADR0092's sealing classification, not a public response cache.

Acceptance and completion event identities use the following exact construction:
let `LP(s)` be `u32be(length(UTF8(s))) || UTF8(s)`. Hash the concatenation of
`LP("connection-proxy-audit-v1")`, `LP(verified tenant account public key)`,
`LP(grant parent ProjectId)`, `LP(GrantId)`, `LP(invocation id)` and
`LP("acceptance")` or `LP("completion")`. The event id is
`connection-proxy-audit-v1/` followed by lowercase hex SHA256 of those bytes.
Only opaque routing identities enter this hash; it is not a content digest or
an account fingerprint. Retries keep the same id and exact safe event payload.

## Request state machine

| Phase | Required evidence | Permitted next phases |
| --- | --- | --- |
| `CLAIMED` | Nonce consumed durably; one worker/fence owns the request | `AUDITED`, `NOT_SENT` |
| `AUDITED` | Durable acceptance fact confirmed on the connection audit stream | `SEND_INTENT`, `NOT_SENT` |
| `SEND_INTENT` | Revision-checked commitment to attempt the outbound send | `COMPLETED`, `OUTCOME_UNKNOWN` |
| `COMPLETED` | Bounded provider response or definitive transport refusal recorded, with body-free completion outbox entry | Terminal |
| `NOT_SENT` | Authoritative fence proves no `SEND_INTENT` committed | Terminal |
| `OUTCOME_UNKNOWN` | Send might have occurred and no definitive completion is recoverable | Terminal |

Authorization, projection freshness, grant state, destination and budget checks
precede claim. Reserve a completion-outbox slot before admitting a request.
After claim, append acceptance with a stable event identity derived from the
receipt identity, confirm its durable receipt and record `AUDITED`. Only then
resolve a credential. Immediately before send, recheck live authority, deadline,
owner fence and receipt revision, then commit `SEND_INTENT` before any outbound
request byte. A failed commit sends nothing. A lost commit acknowledgement is
uncertain and must not trigger a send.

Only the original live worker may send after its successful `SEND_INTENT`.
Disable transport-library automatic retries and redirect following. Recovery
workers never reconstruct or resend the original HTTP request. A crash after
intent but before a socket write intentionally costs an unknown outcome.

A received response that violates the finite transport or credential-response
policy is still evidence that a provider effect might have happened. Report
the typed response refusal without implying that a new request is safe.

The journal's terminal outcome record is also the durable completion outbox.
Publishing completion uses a stable event identity and retries only the audit
append. It never retries the provider call. Returning a provider response
requires durable terminal receipt/outbox acceptance; if that write is uncertain,
return a relay unknown outcome and reconcile. No provider response body is stored
for a later duplicate request.

## Crash and concurrency outcomes

| Scenario | Recovery result |
| --- | --- |
| Concurrent identical invocation ids at different replicas | One conditional create wins; every loser returns replay refusal |
| Crash after claim, before acceptance | Fence the old owner, record `NOT_SENT`; keep the nonce spent |
| Acceptance append committed but reply lost | Reconcile/retry that exact event identity; never infer that it failed or dispatch before confirmation |
| Crash after acceptance, before intent | Fence the owner and record `NOT_SENT`; no replacement worker forwards |
| Intent commit acknowledgement lost | Do not send; reconcile as unknown if the committed phase cannot be established |
| Crash after intent, before/after send | `OUTCOME_UNKNOWN` unless a durable terminal outcome exists; no automatic resend |
| Provider response received, terminal journal write lost | Reconcile the write by receipt identity; unknown if evidence is unavailable |
| Completion audit append unavailable | Drain the durable outbox after recovery; the provider is never called again |
| Journal unavailable or quorum/epoch uncertain | Refuse new dispatch and renewal; no cache or fresh-id bypass |
| Duplicate after any terminal result | Replay refusal, not a cached body and not a fresh provider call |

No phase promises exactly-once provider effects. An invocation is a replay
boundary, not a provider idempotency key. A supervisor must not create a fresh
invocation automatically to hide an unknown outcome. User-requested retries need
an explicit reconciliation or provider-specific idempotency policy. In particular,
[HTTP semantics prohibit a proxy from automatically retrying non-idempotent
requests](https://www.rfc-editor.org/rfc/rfc9110.html#section-9.2.2).
The v1 relay does not automatically replay a request after refreshing a credential;
a safe authentication rejection may trigger health/refresh work for a later call.

## Renewal, pruning and restoration

The journal has an authoritative per-grant root with maximum acknowledged expiry,
terminal/revoked state and epoch. Renewal extends that root durably before a
renewed grant is made usable. A partial renewal can retain receipts longer; it
cannot shorten retention. Never apply a per-request TTL based on the grant's
original expiry while renewal is possible.

Keep consumed ids through the maximum grant expiry plus a 60-second safety
margin. Prune only after the root is terminal, no owner is in flight, no renewal
can commit, and required completion facts are durably accepted. Revocation
closes the root immediately and does not erase spent ids. Service clocks must
be synchronized and monitored; uncertain time refuses proof acceptance/pruning.

A restore, lost receipt series or suspected rollback invalidates all affected
HTTP grants and their confirmation-key attestations before traffic resumes.
Fence old workers, close admission, advance the authoritative restore epoch,
revoke affected grants, reconcile receipts and require fresh grant ids/keys.
The epoch authority must survive independently of the restored snapshot. If
that cannot be proved, affected Sessions remain stopped. Restoring token material
also follows ADR0095's reconnect-required rule; it never restores spend authority.

## Completion backlog and audit retention

Per tenant, durably reserve at most 1,024 outstanding completion slots and 8 MiB
of body-free receipt/outbox metadata across all workers and replicas. Reservation
and admission are fenced against the tenant-wide authoritative budget; a new
worker or restart cannot reset counters, oldest age or outstanding reservations.
Smaller per-worker caps may additionally apply. Refuse new work if either budget is exhausted
or the oldest undelivered completion is 60 seconds old. Existing completion
draining and recovery continue. Capacity is reserved before admission and is
not reclaimed by dropping an unreported outcome. Orphaned reservations require
authoritative reconciliation and fencing of the old owner; lease expiry or a
local empty queue is insufficient evidence to free capacity. An in-memory queue may speed
delivery; it is not the only copy. These are conservative platform defaults,
not throughput or availability claims.

Audit-export outages do not independently gate traffic after the authoritative
audit stream has acknowledged facts. Retention and access controls follow the
[security profile](./connector-security-profile.md). A read model must distinguish
an upstream response, a relay refusal, `NOT_SENT` and an uncertain effect.

## Related contracts

- [Connection HTTP Proxy](./connection-http-proxy.md)
- [HTTP transport profile](./connector-http-transport.md)
- [Binding integration](./connector-binding-integration.md)
- [Conformance requirements](./connector-conformance.md)
