---
number: "0033"
slug: two-tier-key-custody-product-model
status: draft
date: 2026-07-20
---

# ADR#0033: Two-Tier Key Custody Product Model

## Context

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) put OpenBao
behind a platform secrets service with two typed ports, `SecretStore` and
`KeyManagement`. [ADR#0030](./0030-customer-controlled-key-backend-routing.md)
designs customer-controlled key backends behind the `KeyManagement` port:
OpenBao Transit, AWS KMS, and Google Cloud KMS adapters over one
envelope-wrapping contract.

Two questions kept resurfacing while reviewing that direction, and both are
product-model questions rather than adapter mechanics:

1. What are the customer-facing custody offerings? ADR#0030's three adapters
   are easy to misread as three offerings, which conflates who controls a key
   with which vendor hosts it.
2. Which keys may live behind the platform's own key management? Every managed
   key service faces a bootstrapping regress: each layer of key protection
   needs a key, and the bottom layer cannot be protected by the system it
   enables. In this platform the regress shows up as a concrete cycle:
   resolution against the secrets service happens over [NATS](../glossary/nats)
   authenticated with user JWTs minted through `a2a-auth-callout`
   ([ADR#0017](./0017-aauth-agent-authentication.md)), so if the callout
   signing keys were themselves resolved through the secrets service, the
   platform could not cold-start.

Managed cloud key services answer both questions the same way. They present a
default key the provider operates and a customer-managed key the customer
governs, and they terminate their own regress outside the product, in a
hardware security module fleet plus operator ceremony. The custody chain
behind those offerings is explained in
[Key Custody](../architecture/key-custody.md).

This ADR fixes the same two boundaries for this platform. It names the product
tiers above ADR#0030's mechanics and draws the scope line that keeps the
platform's own root of trust out of the business key path. ADR#0030 remains
authoritative for backend registration, binding, provider access, and failure
semantics.

## Decision

### 1. Exactly two customer-facing custody tiers

The product offers two tiers, and only two:

- **[Managed key](../glossary/managed-key)** is the default. The
  key-encryption key is an OpenBao Transit key operated by the platform behind
  the secrets service. A [tenant](../glossary/tenant) configures nothing: no
  registration, no locator, no provider account. Which key wraps a tenant's
  envelopes is resolved server-side from tenant and purpose, per ADR#0030.
- **[Customer managed key](../glossary/customer-managed-key)** is the opt-in.
  The key-encryption key lives in a backend the customer controls, bound
  through the `KeyBackendRegistration` and `BindExternalKey` operations of
  ADR#0030. The customer retains an independent operational veto: disabling
  the key or revoking the platform's access in their own infrastructure fails
  every future wrap and unwrap closed, with no platform fallback.

No middle tier exists. In particular, the product does not offer a dedicated
key that a tenant can observe but whose policy the platform controls, the
shape of a cloud provider's "provider-managed" key. Such a tier changes what a
tenant can see without changing who can stop use, and custody tiers here are
defined by control, not visibility. Importing customer key material into
platform OpenBao remains rejected, and double-wrapping under both a platform
and a customer key remains rejected as the default, as recorded in ADR#0030's
alternatives; deliberate two-party control stays a possible later capability
and a separate decision.

### 2. Tier is registration ownership, not provider choice

A tier names who controls the backend registration, never which vendor
implements it:

| Backend | Tier |
| --- | --- |
| Platform-operated OpenBao Transit | Managed key |
| Customer AWS KMS key | Customer managed key |
| Customer Google Cloud KMS key | Customer managed key |
| Customer-operated OpenBao Transit | Customer managed key |

Product surfaces (console, docs, APIs) present the two tiers, and provider
selection appears only inside the customer-managed flow. Adding an adapter
under ADR#0030's rules changes the provider list, never the tier list.

### 3. Platform boot keys are outside both tiers

No key or credential on the cold-start path from a deployment to a serving
secrets service may be custodied behind the `SecretStore` or `KeyManagement`
ports. That path includes at least the OpenBao bootstrap identity and unseal
material (already fixed by ADR#0023), the `a2a-auth-callout` signing keys that
admit platform workloads to NATS, and the credentials the secrets service uses
for its own connections. These are provisioned by the deployment under the
[ADR#0007](./0007-configuration-sources.md) bootstrap rule.

ADR#0023's evolution path, where the callout's vault-backed signing-key source
becomes a `KeyManagement` caller, stays open only in a topology that takes the
callout off the cold-start path: the callout and the secrets service
authenticate to NATS with deployment-provisioned credentials that are not
subject to callout admission, so the callout can fetch its signing key from an
already-serving secrets service before any callout-admitted workload connects.
Until that ordering is proven, the signing keys remain deployment-provisioned.
Custody hygiene never justifies reintroducing the cycle.

With that line drawn, key custody is a strict dependency chain with no cycles:

> deployment-attested identity → OpenBao → secrets service (`SecretStore`) →
> managed keys (`KeyManagement`) → customer-managed backends

Each stage may depend only on earlier stages. The chain is a structural rule
for designs, and it doubles as the adoption order: a stage ships only when a
consumer demands it, consistent with ADR#0023's proof-gated adoption.

### 4. Business key management wraps tenant business data only

The `KeyManagement` envelope contract of ADR#0030 applies to tenant-scoped
business payloads owned by data-plane subsystems. It does not apply to
platform control-plane state that must be readable before the secrets service
serves, and it is not the mechanism for infrastructure encryption at rest:
encrypting [JetStream](../glossary/jetstream) storage, relational projections,
and disks is a deployment and infrastructure responsibility, outside this
product model.

## Alternatives Considered

### A three-tier model mirroring the cloud providers

AWS presents AWS-owned, AWS-managed, and customer-managed keys. Rejected: the
middle tier grants audit visibility without any control over continued use,
and this platform's tiers exist to answer exactly one question, who can stop
the platform from using a key. ADR#0023's dual-audit rule already records who
used which key in both tiers; surfacing that record to tenants is a
product-surface question, independent of custody tier.

### Envelope-encrypt platform state universally

Rejected. Components that must start before the secrets service serves could
not read their own state, recreating the cycle this ADR removes. ADR#0023
already keeps secret values out of streams and snapshots, so platform state
does not carry material that would justify the dependency.

### Custody platform root keys behind KeyManagement from the start

Rejected. Access to the secrets service is authenticated with identities
minted from those keys; placing them behind the service makes cold start
impossible and turns a platform outage into a potential key-availability
deadlock.

## Non-Goals

- Approving or scheduling implementation. ADR#0030's mechanics remain
  direction, and its prerequisite, the versioned envelope format and cipher
  suite decision, must still exist before adapters are built.
- Changing any registration, binding, provider-access, shutdown, or
  failure-taxonomy rule in ADR#0030.
- Claiming HSM, FIPS, or exclusive-custody properties for either tier;
  ADR#0030's non-goals stand.
- Defining console screens or API shapes; this ADR fixes the vocabulary and
  boundaries they must respect.

## Consequences

- "Managed key" and "customer managed key" become canonical vocabulary with
  glossary entries, and product surfaces stop leaking provider names into the
  custody model.
- The cold-start rule is reviewable: a design that places a boot-path key
  behind the platform ports is structurally rejected rather than debated case
  by case.
- The managed tier inherits ADR#0023's honesty about OpenBao: it is not an
  HSM, and key material exists in server process memory. A customer whose
  requirement is hardware-rooted custody chooses the customer-managed tier
  with a provider that terminates in hardware.
- Deferral stays cheap. Because tiers are resolved server-side behind opaque
  references, a tenant can start on the managed tier and move to a customer
  managed key later through ADR#0030's explicit rewrap migration, without
  data-plane callers changing.

## References

- [ADR#0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](./0030-customer-controlled-key-backend-routing.md)
- [Key Custody](../architecture/key-custody.md)
- [AWS KMS key concepts](https://docs.aws.amazon.com/kms/latest/developerguide/concepts.html)
