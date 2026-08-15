---
number: "0057"
slug: credential-platform-extraction-boundary
status: proposed
date: 2026-08-15
---

# ADR#0057: Credential Platform Extraction Boundary

## Context

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) is the
architecture of record for secret management. It decides that a platform
secrets service is the only process holding an OpenBao client, that every
other consumer holds an opaque `SecretRef` and resolves it over NATS core
request/reply, and that the gateway-embedded prototype is "superseded in
shape but not in substance." It rejects by name both alternatives the
shipped code resembles: **direct reads**, where a workload authenticates to
OpenBao itself, and **control-plane injection**, where resolved values ride
config distribution.

`trogon-gateway` today is the direct-reads shape. `OpenBaoSecretStore` makes
the most internet-exposed process on the platform an OpenBao client, which
is the specific arrangement ADR#0023 Decision 3 argues against. That is the
largest open gap between the running system and its architecture of record,
and it is a sequencing question rather than a defect in the shipped code:
the first credential slice was built before ADR#0023 existed.

What has changed since ADR#0023 is that the code now has a boundary worth
naming. The `secret_store::` module is split into segregated traits
(`SecretStorePut`, `SecretStoreGet`, `SecretStoreRotate`,
`SecretStoreRevoke`, `SecretStoreMetadata`, `SecretStoreDestroy`) that
already match the port ADR#0023 wants. Extraction is therefore a relocation
behind a stable boundary rather than a redesign. What is missing is an
agreed line: which code moves into the secrets service, which stays in the
gateway, and which questions have to be answered before either side can be
built.

ADR#0023 does not draw that line, and without it ADR#0023 and the shipped
credential platform describe two different systems.

## Decision

**The extraction boundary is the `secret_store::` trait split.** Everything
on the OpenBao side of those traits moves into the platform secrets
service. Everything on the caller side stays in `trogon-gateway`, with the
traits reimplemented as a secrets-service client.

### 1. What moves to the secrets service

```text
OpenBao adapter (OpenBaoSecretStore and its path/metadata conventions)
event-sourced credential aggregate, commands, and state snapshots
write, rotation, and destroy sagas
recovery worker
credential management write API and its idempotency ledger
```

### 2. What stays in `trogon-gateway`

```text
runtime projection of refs, not values
typed source value objects (GitHubWebhookSecret, DiscordBotToken, and peers)
source adapters and webhook verification
the SecretStore port, reimplemented as a secrets-service client
```

After the move the gateway has no OpenBao address, token, or policy of its
own.

### 3. The five OpenBao policies are derived from the trait split

`control_plane_write`, `gateway_read`, `lifecycle_worker_cleanup`,
`audit_read`, and `break_glass_admin` are written against the segregated
traits rather than against today's process layout, so the role split
survives the relocation unchanged. They live in
`devops/openbao/policies/`.

### 4. The four questions below are ratified before extraction starts

Each of them changes the shape of the secrets-service boundary, so
answering them during the move means rebuilding the boundary mid-flight.
They are the reason this ADR is `proposed` rather than `accepted`.

## Open Questions Blocking Acceptance

### Q1: Is the resolve surface a value verb, an operation verb, or both?

**Recommendation: both.** ADR#0023 Decision 7 already floats verifying
webhook HMACs through Transit so the signing secret never leaves OpenBao.
That covers ten of the twelve sources and would take the gateway off the
plaintext path almost entirely, but it does not cover Discord's bot token
or outbound provider calls, which need the value itself.

This is first in the list because it determines whether resolve latency is
on the webhook hot path at all, which in turn constrains Q2.

### Q2: After extraction, who holds the cache, and for how long?

**Recommendation: keep the cache in the gateway and amend ADR#0023
Decision 6 to say so.**

The TTL itself is settled:
[ADR#0049](./0049-revocation-latency-target.md) ratifies 300s plus 30s
jitter as the hard staleness bound behind fan-out event invalidation, which
is what Decision 6 means by "TTL as backstop... bounded in minutes."

What is not settled is Decision 6's narrower sentence, that consumers hold
values "for the duration of an in-flight operation, not as ambient state."
That rule was written for consumers of a secrets service that does not
exist yet, and two shipped behaviors already sit outside it:
`RuntimeCredentialCache` is ambient by design, and the Discord bot token is
held for the life of a WebSocket connection.

Moving the cache into the service instead would put a round trip on the
webhook hot path. That is the alternative worth pricing before choosing.

### Q3: Where does credential lifecycle state live after extraction?

**Recommendation: decide alongside the idempotency-record question, since
they have the same answer shape.**

Today the aggregate is a JetStream event stream inside the gateway, per
[ADR#0047](./0047-event-sourced-credential-metadata.md). The credential
platform spec's Architecture Boundaries section assigns metadata, refs,
status, and lifecycle state to the application database. Those are two
homes for one aggregate, and the same fork applies to the management
idempotency records, which are in NATS KV today and could belong to a
future control-plane database.

### Q4: What makes a credential ref opaque, and when does it change?

**Recommendation: sequence it before the wire contract exists**, because it
is far cheaper while `CredentialRef` is an in-process type.

The internal `CredentialId` is still the parseable string
`openbao:{owner}:{scope_key}:{kind}`, and it doubles as the aggregate
stream id, so changing it is an event-store migration rather than a
formatting change.

The management API no longer emits it: responses carry
`PublicCredentialId`, composed from owner, scope, and kind, so no storage
backend is named in a wire contract. That closes the ADR#0023 Decision 2
violation at the boundary and leaves the internal-identifier question open
on purpose.

## Consequences

- The gateway's current OpenBao client is prototype scaffolding with a
  named end state, not an accidental architecture. Work that deepens the
  gateway's coupling to OpenBao specifics should be weighed against this
  ADR.
- The trait split is now load-bearing. Collapsing the segregated traits
  back into a unified `SecretStore` would erase both the extraction seam
  and the derivation of the five policies.
- Until Q1 through Q4 are ratified, the extraction is blocked by design.
  Starting it earlier means answering these questions implicitly, in code,
  under deadline.
- This ADR does not schedule the extraction. It fixes the boundary so that
  the credential platform can keep shipping against a target instead of
  drifting away from one.
