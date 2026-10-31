---
number: "0033"
slug: key-custody-product-model
status: draft
date: 2026-07-20
---

# ADR#0033: Key Custody Product Model

> **Refined by [ADR#0083](./0083-sealed-content-envelope.md) (2026-10-30).**
> Under Decision 6, the envelope contract also covers, for the sealing purpose,
> credential records, every claim-check blob, the `PLATFORM` account's own
> included, and the sensitive streams of customer managed tenants with the bytes
> they reference; state that must be readable before the secrets service serves
> stays unsealed, and the
> [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) server
> cipher remains infrastructure encryption outside this model. The sealing
> purpose has no shared managed key: every tenant's platform share is wrapped
> under a dedicated key, free on Transit (Decision 3), which makes dedicated
> keys for every tenant the fixed rule for this purpose alone, a cloud KMS
> registration's per-key bill accepted. A customer managed tenant adds a
> customer share, the deliberate two-party control Decision 1 leaves to a
> separate decision. Each wrap and unwrap still routes to exactly one
> registration (Decision 5), so unwrapping the customer share never depends on a
> platform-owned key, while opening sealed content needs every share; the
> secrets service's xkey is boot material under Decision 5.

## Context

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) put OpenBao
behind a platform secrets service with two typed ports, `SecretStore` and
`KeyManagement`. [ADR#0030](./0030-customer-controlled-key-backend-routing.md)
designs key backends behind the `KeyManagement` port: OpenBao Transit, AWS
KMS, and Google Cloud KMS adapters over one envelope-wrapping contract, with
customer-controlled registrations as the motivating case.

Three questions kept resurfacing while reviewing that direction, and all of
them are product-model questions rather than adapter mechanics:

1. What are the customer-facing custody offerings?
   [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s adapters are
   easy to misread as offerings, which conflates who controls a key, whether it
   wraps one tenant's data or many tenants' data, and which vendor hosts it.
2. Which backend does the platform run its own offerings on, and may that
   choice change without changing the offerings?
3. Which keys may live behind the platform's own key management? Every managed
   key service faces a bootstrapping regress: each layer of key protection
   needs a key, and the bottom layer cannot be protected by the system it
   enables. In this platform the regress shows up as a concrete cycle:
   resolution against the secrets service happens over [NATS](../glossary/nats)
   authenticated with user JWTs minted through `a2a-auth-callout`
   ([ADR#0017](./0017-aauth-agent-authentication.md)), so if the callout
   signing keys were themselves resolved through the secrets service, the
   platform could not cold-start.

The product shape matters for the first two. Trogonai is one global
multi-tenant deployment: one secrets service, one platform OpenBao cluster, and
one connector fleet serving every [tenant](../glossary/tenant). The tenant, the
company aligned with its NATS account under
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 5,
is the isolation unit. There is no per-customer cloud account and no regional
deployment boundary to lean on, and those are exactly the boundaries cloud
providers use to define their own custody tiers.

Managed cloud key services present a provider-owned key shared across
customers, a provider-operated key dedicated to one account, and a
customer-managed key the customer governs, and they terminate their own regress
outside the product, in a hardware security module fleet plus operator
ceremony. The custody chain behind those offerings is explained in
[Key Custody](../architecture/key-custody.md).

This ADR fixes the same boundaries for this platform. It names the product
tiers above [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s
mechanics, fixes how the platform's own backend is chosen, and draws the scope
line that keeps the platform's root of trust out of the business key path.
[ADR#0030](./0030-customer-controlled-key-backend-routing.md) remains
authoritative for backend registration, binding, provider access, and failure
semantics.

## Decision

### 1. Three customer-facing custody tiers, defined by dedication and control

A key-encryption key has two product-visible properties: whether it wraps one
tenant's envelopes or many tenants' envelopes (dedication), and who can stop
the platform from using it (control). The valid combinations are the tiers,
and there are no others:

- **[Managed key](../glossary/managed-key)** is the default. A platform-owned
  KEK, one per purpose, shared by every tenant that has not selected another
  tier and operated in the platform's managed backend (Decision 3). A tenant
  configures nothing: no registration, no locator, no provider account. Tenant
  separation is the `AuthenticatedBinding` of
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 2,
  enforced as authenticated data on every wrap and unwrap, so a wrapped DEK
  cannot be moved between tenants, resources, or purposes. The tier offers no
  tenant-visible key identity, no per-tenant rotation, and no per-tenant
  cryptographic erasure: removing a tenant's data under a managed key means
  destroying that tenant's envelopes.
- **[Dedicated key](../glossary/dedicated-key)** is an opt-in. A KEK the
  platform operates in the same managed backend that wraps exactly one tenant's
  envelopes, one per tenant per purpose. The tenant sees the key's identity and
  its usage audit, rotation runs for that tenant alone, and the scheduled,
  reviewable destruction of
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 6
  erases that tenant's envelopes without touching anyone else's. Policy and
  availability remain platform-controlled; the tenant has no independent veto.
- **[Customer managed key](../glossary/customer-managed-key)** is an opt-in.
  The KEK lives in a backend the customer controls, bound through the
  `KeyBackendRegistration` and `BindExternalKey` operations of
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md). The customer
  retains an independent operational veto: disabling the key or revoking the
  platform's access in their own infrastructure fails every future wrap and
  unwrap closed, with no platform fallback.

Tenant means the company aligned with its NATS account under
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)
Decision 5. The project of
[ADR#0046](./0046-project-anchored-resource-hierarchy.md) is the owner
scope inside a tenant: it travels in the `AuthenticatedBinding` and is not a
unit of key dedication.

A shared key under one customer's control is not a tier, because a shared key
cannot be subject to one customer's veto. Importing customer key material into
platform OpenBao remains rejected, and double-wrapping under both a platform
key and a customer key remains rejected as the default, as recorded in
[ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s alternatives;
deliberate two-party control stays a possible later capability and a separate
decision.

### 2. Tier is the registration's owner and dedication, not provider choice

A tier names who owns the backend registration and whether the bound key is
dedicated to one tenant, never which vendor implements it:

| Backend | Registration owner | Wraps | Tier |
| --- | --- | --- | --- |
| Platform-operated OpenBao Transit | Platform | Many tenants | Managed key |
| Platform-operated OpenBao Transit | Platform | One tenant | Dedicated key |
| Platform-owned AWS KMS or Google Cloud KMS key | Platform | Many tenants or one tenant | Managed key or dedicated key |
| Customer AWS KMS key | Customer | One tenant | Customer managed key |
| Customer Google Cloud KMS key | Customer | One tenant | Customer managed key |
| Customer-operated OpenBao Transit | Customer | One tenant | Customer managed key |

A platform-owned AWS KMS or Google Cloud KMS key is allowed only once that
provider's key states are mapped onto the platform-managed key states in
[Key States](../architecture/key-states.md). Those states describe only an
OpenBao Transit key today, and until that mapping exists the platform does not
offer that alternative.

Product surfaces (console, docs, APIs) present the tiers, and provider
selection appears only inside the customer-managed flow. Adding an adapter
under [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s rules
changes the provider list, never the tier list.

### 3. The platform's managed backend is a platform-owned registration

The managed and dedicated tiers run on a backend the platform registers for
itself through the same `KeyBackendRegistration` that customers use. The
platform registration is a first-class registration, not a special case:
[ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s binding rules,
failure taxonomy, conformance suite, and rewrap migration apply to it
unchanged, so the adapters customers depend on are exercised by the platform's
own traffic before any customer binds a key.

The default platform backend is OpenBao Transit in the platform's own OpenBao,
sealed by a cloud KMS key under
[ADR#0052](./0052-cloud-kms-production-seal.md); in the hosted deployment that
seal is AWS KMS. A platform-owned AWS KMS or Google Cloud KMS registration is
an allowed alternative once its key states are mapped as the tier table
requires, and it carries the key hygiene
[ADR#0052](./0052-cloud-kms-production-seal.md) requires of the seal key:
provider-generated material, deliberate rotation, old key versions never
destroyed, deletion protection, and a tested recovery path. The platform
claims no hardware-rooted property for the managed or dedicated tier on any
backend; a customer whose requirement is hardware-rooted custody chooses the
customer managed tier with a provider that terminates in hardware.
Which backend the platform runs is a platform operations decision recorded in
deployment capability and routing policy. It is never a tenant-visible setting
and it does not change the tier list. Moving the platform's tiers to another
backend is [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s
explicit rewrap migration per `KeyRef`; there is no configuration-only cutover.

Which platform tier a tenant lands on, and whether dedicated keys are
provisioned at tenant creation or on first wrap, is platform policy resolved
server-side from the authenticated tenant and purpose. Wrap and unwrap requests
never select a tier, a key, or a backend. OpenBao Transit keys cost nothing per
key, so the platform may provision dedicated keys widely there; a cloud KMS
registration bills per key, which is one reason the shared managed key is the
default tier rather than the dedicated key.

### 4. Routing takes tenant and purpose as inputs, not deployment or region

Backend selection and `KeyRef` resolution take the authenticated tenant and the
key purpose as inputs, consistent with
[ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 1.
Region and deployment are not inputs today, because there is one deployment.
If regional deployments or data residency requirements arrive, region becomes
an attribute of backend registrations (cloud KMS keys are regional already) and
an input to routing policy. That adds routing inputs; it does not add or change
a tier.

A `KeyRef` is tenant-scoped in every tier. Under the managed tier many
tenants' `KeyRef`s resolve to one platform key, so the unwrap-time check that
a `KeyRef`'s bound tenant and purpose match the authenticated operation holds
unchanged, and moving a tenant between tiers is a rewrap from one of that
tenant's `KeyRef`s to another.

### 5. Platform boot keys are outside every tier

No key or credential on the cold-start path from a deployment to a serving
secrets service may be custodied behind the `SecretStore` or `KeyManagement`
ports. That path includes at least the OpenBao bootstrap identity and unseal
material (already fixed by
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)), the NATS
operator and account signing seeds, the `a2a-auth-callout` signing keys that
admit platform workloads to NATS, and the NATS credentials the callout and the
secrets service use for their own connections. One callout signing source
serves every tenant account, so account signing keys are platform boot keys,
never tenant-tier keys. These are provisioned by the deployment under the
[ADR#0007](./0007-configuration-sources.md) bootstrap rule.

[ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s evolution
path, where the callout's vault-backed signing-key source becomes a
`KeyManagement` caller, stays open only in a topology that takes the callout
off the cold-start path: the callout and the secrets service authenticate to
NATS with deployment-provisioned credentials that are not subject to callout
admission, so the callout can fetch its signing key from an already-serving
secrets service before any callout-admitted workload connects. Until that
ordering is proven, the signing keys remain deployment-provisioned. Custody
hygiene never justifies reintroducing the cycle.

With that line drawn, key custody is a strict dependency chain with no cycles:

> deployment-attested identity → OpenBao → secrets service (`SecretStore`) →
> `KeyManagement`, which routes each operation to exactly one backend
> registration: the platform's own (managed and dedicated keys) or a
> customer's (customer managed keys)

Each stage may depend only on earlier stages, and the two registration
families are alternatives behind the `KeyManagement` port, not stages of one
another: an operation against a customer managed key never routes through, or
depends on, a platform-owned key. Adoption order is a separate statement
layered on the same chain: the managed key ships first, the dedicated key next
because it is the same backend with per-tenant keys, and customer-managed
backends when a consumer demands them, consistent with
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s
proof-gated adoption.

### 6. Business key management wraps tenant business data only

The `KeyManagement` envelope contract of
[ADR#0030](./0030-customer-controlled-key-backend-routing.md) applies to
tenant-scoped business payloads owned by data-plane subsystems. It does not
apply to platform control-plane state that must be readable before the secrets
service serves, and it is not the mechanism for infrastructure encryption at
rest: encrypting [JetStream](../glossary/jetstream) storage, relational
projections, and disks is a deployment and infrastructure responsibility,
outside this product model.

### 7. The secret store is not the pluggable part

Custody choice lives in the KEK backend behind `KeyManagement`. The
`SecretStore` provider remains OpenBao KV v2 as
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 1
selected, and portability of the store itself is deliberately deferred behind
the segregated `secret_store::` traits that
[ADR#0074](./0074-credential-platform-extraction-boundary.md) names as the
extraction boundary. Replacing the store would reopen the derivation of the
OpenBao policies, the KV v2 version and custom-metadata semantics that the
[ADR#0047](./0047-event-sourced-credential-metadata.md) recovery worker
reconciles against, and destruction semantics, and none of that is required to
offer any tier above. Secrets a customer keeps in their own secret manager are
external sources registered behind the same opaque `SecretRef`
([ADR#0079](./0079-external-secret-sources.md)); they are sources, not
replacement stores.

## Alternatives Considered

### Two tiers, managed and customer managed

The previous draft of this ADR offered exactly two tiers, defined by control
alone, and rejected a middle tier as visibility without control. That left
per-tenant dedication as an unnamed internal choice of the managed tier. In a
single global deployment, dedication is the only separation that does not rest
on policy evaluation being correct, and it is the property that makes
per-tenant rotation and cryptographic erasure possible. Leaving it unnamed
meant those guarantees could not be promised, audited, or sold. The objection
to visibility stands: the dedicated tier is defined by dedication, erasure, and
rotation scope, and its audit visibility is a product surface over
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s dual audit,
not the reason the tier exists.

### Three tiers by account placement, mirroring the cloud providers

AWS presents AWS-owned, AWS-managed, and customer-managed keys, where the
middle tier is a key in the customer's account that the provider operates.
There is no customer account on this platform, so the same property is
expressed as dedication to one tenant. Adopted in substance, not in form.

### Dedicated keys for every tenant and no shared tier

Rejected as the fixed rule. It is free on OpenBao Transit but billed per key on
a cloud KMS registration, and it couples tenant creation to key provisioning in
whichever backend the platform runs. The platform may still choose to
provision dedicated keys for every tenant on Transit as policy; the tier list
does not depend on that choice.

### Hardcode the platform's tiers to OpenBao Transit

Rejected, after an earlier draft assumed it. Fixing the managed and dedicated
tiers to one backend reads as the simpler design, but the assumption breaks the
day a deployment cannot run that backend, for example a government cloud with
an approved key service, and the fix at that point is a redesign of the custody
model rather than a new registration. The registry of
[ADR#0030](./0030-customer-controlled-key-backend-routing.md) already models
backends and adapters for customers, so pluggability for the platform's own
keys costs one more registration owner, not a second mechanism. The first
implementation still ships one backend; what the platform does not do is let
that backend's name leak into the tier model.

### Envelope-encrypt platform state universally

Rejected. Components that must start before the secrets service serves could
not read their own state, recreating the cycle this ADR removes.
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) already
keeps secret values out of streams and snapshots, so platform state does not
carry material that would justify the dependency.

### Custody platform root keys behind KeyManagement from the start

Rejected. Access to the secrets service is authenticated with identities
minted from those keys; placing them behind the service makes cold start
impossible and turns a platform outage into a potential key-availability
deadlock.

## Non-Goals

- Approving or scheduling implementation.
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s mechanics
  remain direction, and its prerequisite, the versioned envelope format and
  cipher suite decision, must still exist before adapters are built.
- Changing any registration, binding, provider-access, shutdown, or
  failure-taxonomy rule in
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md).
- Claiming HSM, FIPS, or exclusive-custody properties for any tier;
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s non-goals
  stand.
- Defining console screens, API shapes, pricing, or plan assignment of tiers;
  this ADR fixes the vocabulary and boundaries they must respect.
- Designing regional deployments or data residency.

## Consequences

- "Managed key", "dedicated key", and "customer managed key" become canonical
  vocabulary with glossary entries, and product surfaces stop leaking provider
  names into the custody model.
- The platform's own backend goes through the customer registration path, so
  the [ADR#0030](./0030-customer-controlled-key-backend-routing.md) conformance
  suite runs against production traffic before any customer binds a key, and a
  platform backend change is a reviewed rewrap migration rather than a
  configuration edit.
- The cold-start rule is reviewable: a design that places a boot-path key
  behind the platform ports is structurally rejected rather than debated case
  by case.
- The managed and dedicated tiers inherit
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s honesty
  about OpenBao: it is not an HSM, and key material exists in server process
  memory. A customer whose requirement is hardware-rooted custody chooses the
  customer managed tier with a provider that terminates in hardware, and the
  platform may move its own registration to a cloud KMS without a tier change.
- Deferral stays cheap. Because tiers are resolved server-side behind opaque
  references, a tenant can start on the managed key and move to a dedicated
  key or a customer managed key later through
  [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s explicit
  rewrap migration of that tenant's envelopes, without data-plane callers
  changing.
- Routing by tenant and purpose means a later regional split changes routing
  inputs, not tiers.

## References

- [ADR#0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](./0030-customer-controlled-key-backend-routing.md)
- [ADR#0047: Event-Sourced Credential Metadata](./0047-event-sourced-credential-metadata.md)
- [ADR#0052: Cloud KMS Auto-Unseal Is Mandatory for Production OpenBao](./0052-cloud-kms-production-seal.md)
- [ADR#0074: Credential Platform Extraction Boundary](./0074-credential-platform-extraction-boundary.md)
- [ADR#0079: External Secret Sources Resolve Behind the Secrets Service](./0079-external-secret-sources.md)
- [Key Custody](../architecture/key-custody.md)
- [AWS KMS key concepts](https://docs.aws.amazon.com/kms/latest/developerguide/concepts.html)
