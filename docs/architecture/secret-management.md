# Secret Management

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
The concrete identity scheme behind a `SecretRef`, how it is encoded, and
whether it is issued at store time or derived deterministically, is not
fixed by the ADR and is an open decision (see
[Open decisions](#open-decisions)).

### Vault

A vault is a user-visible logical grouping of secrets within a company.
It is enforced by policy, not by cryptographic separation: two vaults in
the same company are a console-visible organizing boundary, not two
OpenBao security boundaries ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 5). Whether a vault is a
first-class entity with its own lifecycle (created, renamed, and deleted
independently of the secrets inside it) or purely a naming attribute on a
`SecretRef` is not fixed by the ADR and is an open decision.

### Fingerprints and reason enums

Events and snapshots in the write model below carry references,
fingerprints, and reason enums, never values ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 4). A
fingerprint lets the aggregate and the recovery worker compare whether
this is the same credential material as before, without holding or
re-deriving the material itself. A reason field records why a state
changed (for example, a rotation trigger versus an operator-initiated
revocation) without carrying the credential value. [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) calls these
reason enums; whether the implementation fixes a closed set of typed
values or a bounded non-secret string is an open decision (see
[Open decisions](#open-decisions)).

## The write model

The write model has three parts, which [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 4 records as
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
  pending event and activation by reading OpenBao metadata, never a
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
by [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md); the complete credential state machine is an open decision
(see [Open decisions](#open-decisions)).

A separate axis, which version of the credential material is current and
whether an older version has been removed, is not tracked as platform
state in parallel with OpenBao. It derives from OpenBao KV v2 mechanics
instead ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 4): the key-level `current_version` pointer
identifies the version resolution serves, and the per-version `destroyed`
and `deletion_time` fields report whether that version, or an earlier
one, has been removed. The KV v2 version counter is the credential's
version number; rotation advances `current_version` rather than the
platform maintaining a second counter.

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

Whether a resolved secret value is held in a single-owner, zeroizing
wrapper analogous to `KeyManagement`'s `Dek` wrapper, and exactly what
that wrapper requires, is not fixed by [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) and is an open decision.

## Failure handling

The service fails closed: a secret that cannot be resolved is an
operation that does not happen, and there is no plaintext fallback
([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6). This extends to the service's own bootstrap: an
instance that cannot authenticate to OpenBao, at startup or when renewal
lapses, does not report ready, does not join the resolve queue group, and
retries with backoff rather than serving in a degraded mode ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
Decision 3).

Whether resolve failures map to a stable, caller-visible taxonomy
analogous to `KeyManagement`'s eight `wrap_dek` and `unwrap_dek`
categories is not fixed by [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md), so this taxonomy is an open decision
rather than a restatement of an existing model.

[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) commits to dual audit: a business-context record (caller,
tenant, vault, ref, purpose, outcome) with a correlation ID threaded into
the OpenBao request, so the provider's own audit log joins back to who
asked and why ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6). What the service does when an audit
sink itself fails to accept a record is not fixed; [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) lists correct
behavior when audit sinks fail among the proofs required before adoption,
which means this behavior must be answered before adoption, not assumed.

## What the service never does

- Let a secret value cross into JetStream, a log, or a trace.
- Accept a caller-supplied OpenBao path, or any other provider artifact,
  as a `SecretRef` or anywhere else across the port boundary.
- Hold a standing, broad OpenBao token; it mints short-lived, narrowly
  scoped credentials per request instead ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 3).
- Serve a cached value past its bounded window as a plaintext fallback
  when an invalidation event is missed.

## Open decisions

- **`SecretRef` identity scheme.** How the opaque handle is constructed
  and encoded is not fixed by [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md).
- **Reason value typing.** Whether the reason field recorded on a state
  change becomes a closed set of typed values, as [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 4's
  wording suggests, or stays a bounded non-secret string is not settled.
- **Credential lifecycle state machine.** [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) fixes only the pending
  and active states of the write saga; whether the aggregate also models
  explicit write-failure, rotation-in-progress, or revocation states is
  not fixed.
- **Vault entity lifecycle.** Whether a vault is a first-class entity
  with its own lifecycle, or purely a policy grouping attribute, is not
  fixed by [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 5.
- **Resolved-secret-value wrapper requirements.** Whether resolved values
  use a zeroizing wrapper analogous to `KeyManagement`'s `Dek` wrapper,
  and what that wrapper requires, is not fixed.
- **Caller-visible resolve failure taxonomy.** Whether resolve failures
  map to a stable set of categories, comparable to `KeyManagement`'s
  eight, is not fixed.
- **Audit-sink failure behavior.** [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) names correct behavior when
  audit sinks fail as a required adoption proof, without answering what
  that behavior is ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Consequences).
- **Anomaly detection scope.** [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) states that anomaly detection on
  resolve subjects is structural rather than optional, given that this
  service is the platform's highest-value compromise target, without
  defining what counts as anomalous or what response it triggers
  ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Consequences).

## See also

- [Key Management](./key-management.md)
- [Key Custody](./key-custody.md)
- [Key States](./key-states.md)
