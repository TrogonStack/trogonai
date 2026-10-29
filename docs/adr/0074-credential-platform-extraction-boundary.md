---
number: "0074"
slug: credential-platform-extraction-boundary
status: accepted
date: 2026-08-15
---

# ADR#0074: Credential Platform Extraction Boundary

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
is the specific arrangement [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 3 argues against. That is the
largest open gap between the running system and its architecture of record,
and it is a sequencing question rather than a defect in the shipped code:
the first credential slice was built before [ADR#0023](./0023-secret-management-and-key-custody-direction.md) existed.

What has changed since [ADR#0023](./0023-secret-management-and-key-custody-direction.md) is that the code now has a boundary worth
naming. The `secret_store::` module is split into segregated traits
(`SecretStorePut`, `SecretStoreGet`, `SecretStoreRotate`,
`SecretStoreRevoke`, `SecretStoreMetadata`, `SecretStoreDestroy`) that
already match the port [ADR#0023](./0023-secret-management-and-key-custody-direction.md) wants. Extraction is therefore a relocation
behind a stable boundary rather than a redesign. What is missing is an
agreed line: which code moves into the secrets service, which stays in the
gateway, and which questions have to be answered before either side can be
built.

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) does not draw that line, and without it [ADR#0023](./0023-secret-management-and-key-custody-direction.md) and the shipped
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

Every one of them binds to a secrets-service identity. `gateway_read` names
the read the service performs on the gateway's behalf through
`SecretStoreGet`; it is not a role the gateway process holds. The gateway's
requests are authorized the way
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 3
already prescribes, by the tenant-scoped NATS user JWT and the NATS account
that scope its resolve subjects, so no OpenBao policy is attached to the
gateway after the move.

### 4. Four boundary questions, answered before extraction starts

Each of them changes the shape of the secrets-service boundary, so answering
them during the move would have meant rebuilding the boundary mid-flight.
They are answered below. The extraction is unblocked; it is still not
scheduled.

#### Q1: The resolve surface has a value verb and an operation verb

Both. [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 7 already floats verifying webhook HMACs through
Transit so the signing secret never leaves OpenBao. That verification is the
operation verb, and it is a `verify` call on [ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s `KeyManagement` port: the
secrets service runs it against Transit and returns a verdict. The gateway
reaches it through that port's secrets-service client and never calls Transit
directly, because [ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s single-client rule keeps the secrets service the
only OpenBao client. That covers ten of the twelve sources and takes the
gateway off the plaintext path almost entirely.
The value verb remains for Discord's bot token and for outbound provider
calls, which need the value itself. A new source uses the operation verb
unless it can show it needs the value.

This question comes first because it determines whether resolve latency is
on the webhook hot path at all, which constrains Q2.

#### Q2: The cache stays in the gateway

The gateway keeps `RuntimeCredentialCache`, and [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 6 reads as
amended: a consumer may hold resolved values as bounded ambient state under
the [ADR#0049](./0049-revocation-latency-target.md) bound of 300s plus 30s
jitter behind fan-out invalidation, not only for the duration of one
in-flight operation. That bound applies to consumers that resolve per use. A
long-lived connection, such as the Discord gateway WebSocket, keeps the bot
token it connected with until that connection ends, and the revocation event
evicts only the cached copy. Discord does not guarantee that resetting a bot
token ends a session that already authenticated, so the gateway closes the
connection when it receives the revocation event and reconnects with a freshly
resolved token. Until that close lands, the held connection is the residual
exposure window, and it sits outside the
[ADR#0049](./0049-revocation-latency-target.md) bound.

Moving the cache into the service was priced and rejected: it puts a NATS
round trip on every webhook, and the staleness bound is enforced identically
on either side of the boundary.

#### Q3: Lifecycle state and idempotency records stay on JetStream and NATS KV

The credential aggregate remains the JetStream event stream that
[ADR#0047](./0047-event-sourced-credential-metadata.md) made the source of
truth, relocated into the secrets service by Decision 1, and the management
idempotency records remain in NATS KV. No relational database enters the
credential platform. The credential platform spec's Architecture Boundaries
section, which assigned metadata, refs, status, and lifecycle state to an
application database, reads as amended to match. Whatever a future control
plane needs from this data, it reads as a projection over the same stream.

#### Q4: The credential ref is opaque before the wire contract exists

The internal `CredentialId` stops being the parseable
`openbao:{owner}:{scope_key}:{kind}` string while `CredentialRef` is still an
in-process type. Because the id doubles as the aggregate stream id, the
change is an event-store migration, and it is sequenced as the first
extraction task rather than deferred past the wire contract. Stored OpenBao
paths are not rewritten: the path a credential was written under stays
recorded in its aggregate as adapter-private state, so the path convention
[ADR#0046](./0046-project-anchored-resource-hierarchy.md) ratified holds, and so does its rule that an id carries the project
and nothing above it, because an opaque id carries no hierarchy at all.

The migration contract is that this is the only re-key. It runs once, as that
first extraction task, while `CredentialRef` is still an in-process type. From
then on `CredentialId` values and stream identities are preserved verbatim
through the rest of the extraction: relocating the aggregate into the secrets
service re-keys nothing, and projections and references keep working without
a mapping table.

The management API already emits `PublicCredentialId`, composed from owner,
scope, and kind, so no storage backend is named there. The `StorageBackend`
enum in the public credential metadata contract (`OPENBAO`, `IN_MEMORY`,
`STATIC_CONFIG`) is removed for the same reason: it names a storage backend
in exactly the way [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 2 forbids. The aggregate's own state may
keep that knowledge as adapter-private detail, and any protection fact a
caller legitimately needs is expressed as protection metadata, never as a
backend name.

## Consequences

- The gateway's current OpenBao client is prototype scaffolding with a
  named end state, not an accidental architecture. Work that deepens the
  gateway's coupling to OpenBao specifics should be weighed against this
  ADR.
- The trait split is now load-bearing. Collapsing the segregated traits
  back into a unified `SecretStore` would erase both the extraction seam
  and the derivation of the five policies.
- With the four questions answered, the extraction is unblocked. It is
  still not scheduled; this ADR fixes the boundary so that the credential
  platform keeps shipping against a target instead of drifting away from
  one.
- [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 6 and the [credential platform spec](../research/credential-platform/credential-platform-spec.md)'s Architecture
  Boundaries section read as amended by Q2 and Q3. This ADR is the current
  text on cache placement and lifecycle-state placement.
