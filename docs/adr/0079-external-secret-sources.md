---
number: "0079"
slug: external-secret-sources
status: draft
date: 2026-10-28
---

# ADR#0079: External Secret Sources Resolve Behind the Secrets Service

## Context

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) made the
platform secrets service the only process that touches a secret backend and
gave every consumer an opaque `SecretRef`.
[Provider Credential Custody](../architecture/provider-credential-custody.md)
extends that boundary to credentials a customer keeps in a secret manager they
operate: the source is registered with the secrets service, and provider
management receives a reference, never a locator. The same page lists what
that left undecided: reference identity for external entries, pinned versus
current-version selection, rotation ownership, and behavior when the source
disappears.

No adapter for an external secret manager exists. The gateway's
`secret_store::` module has OpenBao, in-memory, and static-config
implementations only, and
[ADR#0074](./0074-credential-platform-extraction-boundary.md) fixes that
module's segregated traits as the boundary the secrets service is extracted
behind. Deciding the external-source semantics now keeps the extraction from
encoding a different answer by accident, and keeps external sources from being
built as additional `SecretStore` backends, which
[ADR#0033](./0033-key-custody-product-model.md) Decision 7 rules out.

This ADR records direction for the first version. It does not approve or
schedule an adapter.

## Decision

### 1. A source is a tenant-owned registration that yields references

An external source is a `SecretSourceRegistration`: durable, tenant-scoped
state owned by the secrets service, with a typed provider variant (AWS Secrets
Manager, Google Secret Manager, customer-operated OpenBao KV), the
provider-specific locator of each selected entry, and a non-secret
authentication profile. Workload identity and cross-account role assumption
with a tenant-unique external ID are the expected authentication shapes; an
unavoidable long-lived credential for reaching the source is itself stored
behind `SecretStore`. Registration validates access with a read of metadata,
not material, and returns one opaque `SecretRef` per selected entry.

Provider management, model access, and every other consumer hold that
reference exactly as they hold a reference to platform-stored material.
Nothing downstream can tell the two apart, and no provider locator or source
credential enters a provider stream.

### 2. Resolution reads the source's current version

Resolving a source-backed reference reads the provider's current version at
resolve time: the `AWSCURRENT` staging label in AWS Secrets Manager, the latest
enabled version in Google Secret Manager, and the current KV v2 version in
OpenBao. The first version does not offer pinning to a specific provider
version. A reference therefore follows the customer's rotation without any
platform action.

### 3. The customer owns rotation; the platform owns validation

The platform never writes to, rotates, or deletes an entry in a customer's
source. `SecretStorePut`, `SecretStoreRotate`, and `SecretStoreDestroy` are
not offered against source-backed references; revoking the reference and
re-registering are the only mutations. A rotated value becomes eligible for
use only after it passes the same checks a resubmitted value would, including
[ADR#0032](./0032-model-route-and-credential-binding.md)'s expected provider
subject and billing boundary validation for model provider connections. A
rotation schedule firing at the provider is not evidence that a value changed
or that it is valid.

### 4. Caching and invalidation follow the platform bound

Resolved values are cached under the
[ADR#0049](./0049-revocation-latency-target.md) bound of 300 seconds plus up to
30 seconds of jitter, in the same caches and with the same fan-out
invalidation as platform-stored material. The first version subscribes to no
provider change notifications; a rotation at the source is observed within the
bound. Revoking the reference on the platform invalidates immediately through
the existing revocation event.

This narrows [ADR#0049](./0049-revocation-latency-target.md) for source-backed
references. Its event path stays the operative mechanism for every change the
platform makes, including revoking the reference. A change made at the source,
including the customer revoking the platform's access there, raises no platform
event in the first version, so the bound is the guarantee for it. This ADR
records that as an accepted limit of source-backed references, not as a failure
of the event path, and the change-notification subscription below is the route
to lifting it. A customer who needs immediate effect revokes the reference on
the platform.

### 5. Disappearance fails closed

A deleted or disabled entry, revoked access, or an unreachable source produces
a typed resolve failure in the categories
[ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 6
already defines for key backends: denied, unavailable, and throttled stay
distinct, and operational unavailability never collapses into a security
denial. The consumer's connection is marked degraded with the category and the
reference. There is no fallback to a cached value past the bound, no fallback
to a platform-held copy, and no automatic re-pointing to another entry. The
platform never copies source material into OpenBao to protect itself from
source outages; doing so would create a second custodian with its own
retention and deletion obligations.

## Alternatives Considered

### Pin a provider version at registration

Rejected for the first version. A pinned reference silently stops following
the customer's rotation, which is the behavior customers choose an external
source to get, and it needs its own re-pin workflow and staleness alerting
before it is safe to offer.

### Subscribe to provider change notifications

Deferred. EventBridge and Pub/Sub notifications would shorten the observation
window below the [ADR#0049](./0049-revocation-latency-target.md) bound, but a
notification is not proof of a valid change, so the validation path of
Decision 3 is required either way. Adding the subscriptions later changes
latency, not semantics.

### Copy the entry into OpenBao on registration

Rejected. It turns an external source into an import with a stale copy,
defeats the customer's control over the material, and adds deletion
obligations the platform cannot discharge once the source is revoked.

### Build external sources as `SecretStore` backends in the gateway

Rejected by [ADR#0023](./0023-secret-management-and-key-custody-direction.md)
Decision 3 and [ADR#0033](./0033-key-custody-product-model.md) Decision 7.
Sources are reached only by the secrets service, and they are read-only
sources behind the same reference, not replacement stores.

## Non-Goals

- Implementing or scheduling any source adapter.
- Writing to, rotating, or deleting entries in customer sources.
- Pinned versions, change-notification subscriptions, or per-source cache
  bounds.
- Azure Key Vault or any provider not already named by
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md).
- Encryption-key custody for the material a source holds. That is the source
  operator's concern; the tiers of
  [ADR#0033](./0033-key-custody-product-model.md) apply to envelopes the
  platform holds, not to a customer's secret manager.

## Consequences

- Provider management and model access never gain a second reference type or
  a provider-native locator field; the boundary
  [Provider Credential Custody](../architecture/provider-credential-custody.md)
  asks for holds.
- Customers get rotation without platform coordination, and accept that a
  rotated value is observed within the
  [ADR#0049](./0049-revocation-latency-target.md) bound rather than instantly.
- An external source outage is a tenant-visible degradation, never a silent
  fallback, which is the posture
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md) takes for
  customer managed keys.
- The segregated traits of
  [ADR#0074](./0074-credential-platform-extraction-boundary.md) gain a
  read-only implementation shape, `SecretStoreGet` and `SecretStoreMetadata`
  without the write traits, which is further reason to keep the traits
  segregated.

## References

- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](./0030-customer-controlled-key-backend-routing.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0033: Key Custody Product Model](./0033-key-custody-product-model.md)
- [ADR#0049: Revocation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0074: Credential Platform Extraction Boundary](./0074-credential-platform-extraction-boundary.md)
- [Provider Credential Custody](../architecture/provider-credential-custody.md)
- [AWS Secrets Manager version staging labels](https://docs.aws.amazon.com/secretsmanager/latest/userguide/getting-started.html#term_version)
- [AWS confused-deputy protection for third-party access](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_common-scenarios_third-party.html)
- [Google Secret Manager versions](https://docs.cloud.google.com/secret-manager/docs/secret-versions)
