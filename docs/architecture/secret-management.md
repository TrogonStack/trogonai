# Secret Management

The completed event-contract design for custody, connections and webhooks is in
[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md),
[ADR#0067](../adr/0067-webhook-ingress-and-delivery.md),
[ADR#0068](../adr/0068-provisioning-sagas-and-owned-resource-cleanup.md), and the
[contract reference](../reference/secret-and-connection-contracts.md). They
settle the contract choices listed on this page. The custody/connection and
webhook decisions are accepted; the provisioning decision remains draft pending
its separate maintainer signoff. The description below records the earlier accepted custody
direction and its implementation gaps; it does not claim the new services exist.

This page describes the intended design recorded in
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
(accepted) and
[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) and
[ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) (both draft).
None of it is implemented yet. It is the model the implementation must
satisfy, not current behavior. [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) records that the write model below
was validated end to end by a prototype embedded in `trogon-gateway`, and
that adopting the ADR means extracting that prototype's mechanics into the
secrets service, not rebuilding them (Decision 4). That prototype's code is
not present in this repository, so this page restates only what the ADR
itself fixes.

## Scope

This page covers only `SecretStore`, the port that stores and resolves
retrievable credentials: provider OAuth tokens, webhook signing secrets,
bot tokens, and API credentials ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 2). Cryptographic
operations, key custody tiers, and key states belong to the
`KeyManagement` port, covered by [Key Management](./key-management.md),
[Key States](./key-states.md), and [Key Custody](./key-custody.md). The
two ports are typed separately so that business code never sees a generic
provider interface, and neither port's caller ever holds an OpenBao
artifact ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 2).

## Concepts

### SecretRef

A `SecretRef` is an opaque handle a caller holds in place of a
credential. The secrets service resolves it internally to a
[tenant](../glossary/tenant), a vault, and a key; it is never an OpenBao
path, and a caller cannot parse or construct one ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 2).
The accepted contract in
[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md) makes it a
random stable identifier that survives material rotation. See the
[settled contract decisions](#settled-contract-decisions) for related boundaries.

### Vault

A vault is a user-visible logical grouping of secrets within a company.
It is enforced by policy, not by cryptographic separation: two vaults in
the same company are a console-visible organizing boundary, not two
OpenBao security boundaries ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 5). The event contract in
[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md) makes vaults
independent project-owned metadata aggregates with their own lifecycle.

### Connection

A connection is the external system a credential authenticates against: a
source host, an issue tracker, a chat workspace, an observability backend. It
is the structural half of a pair whose other half is the material, and it is
the half that holds still under rotation. Replacing the bytes behind a
credential does not make it a different connection.

The general connection protobuf resource is defined by accepted
[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md); its runtime
does not exist yet.
[ADR#0032](../adr/0032-model-route-and-credential-binding.md) defines a
[ModelProviderConnection](../glossary/modelproviderconnection) for one provider
class, and draft [ADR#0063](../adr/0063-agent-connection-declarations.md)
Decision 5 defers the form for every other class to its own record. So a
`ConnectionDeclaration` names a provider and label predicates that admission is
meant to resolve against a catalog the security plane owns, and that catalog is
described in the [contract reference](../reference/secret-and-connection-contracts.md).
Until a runtime implements admission and mediation, a declaration remains a
reviewable statement of intent rather than an operational resolution path.

### Fingerprints and reason enums

Events and snapshots in the write model below carry references,
fingerprints, and reason enums, never values ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 4). The new event contracts use fingerprints only for non-secret account,
endpoint and configuration identities, never secret material. Version attribution
uses opaque operation ownership recorded atomically with material inside custody.
Closed reason enums describe lifecycle changes without provider response text,
as fixed by [ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md).

## The write model

The baseline write model in [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 4 was
validated end to end by the gateway-embedded prototype:

- **The credential aggregate.** An event-sourced aggregate on
  [JetStream](../glossary/jetstream). Its events and snapshots carry
  refs, fingerprints, and reason enums, and never the secret value
  itself.
- **The write saga.** Storing a credential proceeds as a pending event, an
  OpenBao write, and an activation recorded with metadata only. The value
  is written to OpenBao inside that saga; the platform's own event log
  never receives it.
- **The recovery worker.** It reconciles an aggregate stuck between the
  pending event and activation. The final contract inspects private atomic
  version attribution inside custody and exposes only metadata, never a
  value, to determine whether the write actually landed.

There is no distributed transaction between the platform's state and
OpenBao. The saga and the recovery worker are the entire consistency
mechanism; a stuck write is a recoverable state, not a two-phase-commit
failure ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 4).

[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) attributes this shape to a prototype that already built it end to
end, and it calls extracting that prototype, not rebuilding it, the work
adopting this ADR requires. The prototype's internal type and event names,
its exact operation surface, and the recovery worker's precise
reconciliation algorithm are not published in the ADR and are not restated
here beyond what Decision 4 fixes.

## Credential states

The write path fixes two states. Pending runs from the moment the saga's
pending event is recorded until the OpenBao write and activation
complete; active follows once activation is recorded with metadata only
and the credential is resolvable. The recovery worker reconciles an
aggregate stuck between those two. Richer lifecycle states (an explicit
write-failure state, rotation in progress, revocation) are not enumerated
by [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md); the completed event lifecycle is defined by
[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md) and its
compensation rules by
[ADR#0068](../adr/0068-provisioning-sagas-and-owned-resource-cleanup.md).

Physical version numbers remain OpenBao KV v2 version numbers; the platform
records observed destruction and deletion metadata rather than inventing a
second backend version counter. The active version used by the platform is the
version named in its durable activation event. OpenBao's `current_version` may
instead identify an unverified candidate or non-secret fence marker and must
never independently select material for use. This refinement is fixed in
[ADR#0068](../adr/0068-provisioning-sagas-and-owned-resource-cleanup.md).

[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) does not enumerate a complete credential lifecycle state machine
beyond what Decision 4 fixes here. The five explicit states in Decision 6
(active, decrypt-only, disabled, pending destruction, destroyed) belong
to `KeyManagement`'s `KeyRef`, not to `SecretStore`'s credential
aggregate, and should not be read across.

## Resolution

A caller resolves a `SecretRef` over NATS core request/reply,
authenticated by the tenant-scoped NATS user JWTs `a2a-auth-callout`
already mints ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 3). Tenant identity is always derived
server-side from the authenticated connection, never from a field the
caller supplies; the NATS account, which is the tenant unit, scopes who
can even address a company's resolve subjects.

Values never traverse [JetStream](../glossary/jetstream), a log, or a
trace. JetStream carries only the metadata that surrounds a credential:
rotation, revocation, cache-invalidation, and audit-correlation events,
never the value itself ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 3).

## Who may resolve a ref

[Resolution](#resolution) above answers how a caller that already holds a
`SecretRef` exchanges it for a value, and how tenant scoping bounds which
company's refs that caller can address. It does not answer how a caller comes to
hold a ref in the first place. That question has three parts, and this page owns
only one of them.

- **The store.** Everything above: what material exists, what state it is in,
  and how a holder resolves it. Fixed by
  [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) and, for
  the credential aggregate's events and exposure rule, by
  [ADR#0047](../adr/0047-event-sourced-credential-metadata.md) and
  [ADR#0048](../adr/0048-one-time-plaintext-exposure.md).
- **The declaration.** What an Agent revision says it needs. Draft
  [ADR#0063](../adr/0063-agent-connection-declarations.md) puts a
  `ConnectionDeclaration` on `AgentDependencies` that names a provider and label
  predicates and carries no credential, no ref, and no version. It grants
  nothing; it is a reviewable statement that this behavior reaches that system.
- **The grant.** What turns a declared requirement plus a live authorization
  decision into a usable connection for the duration of one execution. For
  model providers, draft
  [ADR#0032](../adr/0032-model-route-and-credential-binding.md) defines this as
  an attempt-scoped `ModelAccessGrant`. Tool and channel grants use the connection event contracts in
  [ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md).

The agent runtime is not the caller in any of these cases. Draft
[ADR#0032](../adr/0032-model-route-and-credential-binding.md) Decision 4 fixes
that for the model path: the native implementation "never receives the token,
confirmation key, renewal authority, upstream credential, or permission to
resolve a `SecretRef`", because a platform-controlled supervisor holds the grant
and exposes only a session-scoped endpoint.

The [provider agent contracts research corpus](../research/provider-agent-contracts/index.md)
found the same boundary reached independently by four external implementations
outside this repository, which is worth recording because it means the shape is
not a local preference. In all four the agent holds an opaque placeholder and
the real value is substituted into the outbound request at network egress, and
in all four the substitution both sets the authenticated form and removes the
placeholder rather than leaving both present. The connection executor boundary in
[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md) now owns
credential-bearing tool and channel transport; its runtime remains future work.

### The same words mean different things elsewhere

Five layers sit between stored material and a usable connection, and this page
owns only the first. Naming them in order is what keeps the rest legible:

1. **Vault.** A policy-enforced grouping of stored material within a company.
2. **Credential.** The material itself plus its state, addressed by a
   `SecretRef`.
3. **Connection.** The external system that material authenticates against.
4. **Declaration.** What an Agent revision says it reaches. Grants nothing.
5. **Grant.** What turns a declaration plus a live authorization decision into
   something usable for one execution.

The [research corpus](../research/provider-agent-contracts/index.md) uses some
of the same words for different layers, so a reader comparing the two needs the
mapping rather than the vocabulary:

| Layer | Anthropic Managed Agents | OpenAI Agents API | OpenComputer | xAI |
| --- | --- | --- | --- | --- |
| Vault | `vault` (`vlt_...`) | `vault` | `SecretStore` | none |
| Credential | `vault_credential` (`vcrd_...`) | `vault.credential` | a name passed to `useSecret()` | passed inline on every request |
| Connection | not separable, a credential is bound to one `mcp_server_url` | not separable, a credential matches a server URL | `defineConnection`, fused with the credential | none |
| Declaration | none | none | `defineConnection` in source, published as `requiredConnections` | none |
| Grant | none | none | none, egress substitution stands in for one | none |
| Attachment point | `vault_ids[]` fixed at session creation | `vault_ids[]` at session creation | `secretStore` at sandbox creation | none |

Two collisions in that table are worth stating outright, because both have
already produced a wrong reading of the corpus.

The first is the word vault. Ours is a console-visible organizing boundary and
nothing more: two vaults in the same company are a policy grouping, not two
OpenBao security boundaries ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 5).
Anthropic's and OpenAI's vault is that plus an attachment point. `vault_ids[]`
is fixed on the session at creation, which makes their vault the unit of
session-scoped reach. Ours carries no such meaning, and nothing in this
platform attaches a vault to a session today.

The second is the word connector, which is not another name for a vault id. In
the OpenAI **Responses** API a `connector_id` points at a provider-held stored
connection, one layer below a vault. The **Agents** API has no such field: its
per-tool selector is `credential_id`, documented as "The vault credential
selected for this MCP server", so it selects material and not a connection.
xAI's OpenAPI schema carries `connector_id` and the documentation states that
it and `require_approval` "are not currently supported", so the only other
appearance of the word in the corpus is a rejection. A connector in the sense
either vendor uses it is the resource our
[Connection](#connection) concept says we do not have.

## Caching

Caches are bounded, with event-driven invalidation from rotation and
revocation events as the primary mechanism and a TTL backstop as the
maximum stale window after a missed event; that TTL is bounded in
minutes, not hours. Invalidation is fan-out, not queue-group
work-sharing: resolve traffic load-balances across the queue group, but
every secrets-service replica must observe rotation and revocation
through its own JetStream consumer, because queue-group delivery would
evict one replica's cache and leave the others serving a stale or revoked
credential. A consumer holds a resolved value only for the duration of an
in-flight operation, never as ambient state ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6).

[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md) requires
single-owner zeroizing material values, with redacted formatting and bounded
operation lifetimes. This is a future implementation requirement.

## Failure handling

The service fails closed: a secret that cannot be resolved is an
operation that does not happen, and there is no plaintext fallback
([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6). This extends to the service's own bootstrap: an
instance that cannot authenticate to OpenBao, at startup or when renewal
lapses, does not report ready, does not join the resolve queue group, and
retries with backoff rather than serving in a degraded mode ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
Decision 3).

[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md) fixes
closed non-secret failure enums. Provider response strings cannot become public
diagnostics or durable event payloads.

[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) commits to dual audit: a business-context record (caller,
tenant, vault, ref, purpose, outcome) with a correlation ID threaded into
the OpenBao request, so the provider's own audit log joins back to who
asked and why ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6). [ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md) requires
durable audit acceptance before secret use, including cache hits; failure stops
the operation. Runtime proof remains required before adoption.

## What the service never does

- Let a secret value cross into JetStream, a log, or a trace.
- Accept a caller-supplied OpenBao path, or any other provider artifact,
  as a `SecretRef` or anywhere else across the port boundary.
- Hold a standing, broad OpenBao token; it mints short-lived, narrowly
  scoped credentials per request instead ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 3).
- Serve a cached value past its bounded window as a plaintext fallback
  when an invalidation event is missed.

## Settled contract decisions

The earlier baseline left the following choices open.
[ADR#0066](../adr/0066-secret-service-and-connection-boundaries.md),
[ADR#0067](../adr/0067-webhook-ingress-and-delivery.md) and
[ADR#0068](../adr/0068-provisioning-sagas-and-owned-resource-cleanup.md) now supply concrete
answers and protobuf contracts. The custody/connection and webhook decisions
are accepted; only the provisioning decision awaits maintainer signoff.

| Choice | Event contract |
| --- | --- |
| SecretRef identity | Random opaque identity, stable across rotation |
| Reason values and resolve failures | Closed non-secret enums |
| Credential lifecycle | Separate policy state, physical versions and recoverable operations |
| Vault lifecycle | Independent project-owned metadata aggregate |
| External side effects | Owner-stream reservation, immutable plan, exact receipts and compensation through confirmed absence |
| Resolved value lifetime | Zeroizing single-owner value, bounded to one operation |
| Audit failure | Fail secret use closed, including cache hits |
| Non-model connection access | Versioned connector, stable connection, purpose binding and scoped grant |
| Incoming and outgoing webhooks | Separate verification and delivery authority, durable recovery |
| Anomaly response | Inline denial and metadata alerts; policy changes remain explicit audited commands |

## See also

- [ADR#0063: An Agent Declares the Connections It Needs](../adr/0063-agent-connection-declarations.md)
- [Provider agent contracts research corpus](../research/provider-agent-contracts/index.md)
- [Key Management](./key-management.md)
- [Key Custody](./key-custody.md)
- [Key States](./key-states.md)
