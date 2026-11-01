---
number: "0030"
slug: customer-controlled-key-backend-routing
status: draft
date: 2026-07-18
---

# ADR#0030: Customer-Controlled Key Backend Routing

> **Refined by [ADR#0083](./0083-sealed-content-envelope.md) (2026-10-30).** The
> Non-Goal on the payload cipher, nonce construction and envelope format is
> settled for the sealing purpose: AES-256-GCM with random nonces through
> aws-lc-rs, in a `SealedContent` envelope that persists, beside the ciphertext,
> a `KeyRef`, the binding version and a `WrappedDek` per wrap, as Decision 2
> requires; every wrapped `Dek` is 32 bytes. The secrets service is the
> encryption caller, and Transit `datakey/plaintext` is the typed optional
> capability for provider-side generation that Decision 2 anticipates. A data
> key is derived from shares, and a customer managed tenant's needs a platform
> share and a customer share: the separate decision on deliberate two-party
> control that the double-wrap alternative anticipates, taken with a split key
> rather than a double wrap. For sealing data keys only, Decision 2's rule
> against reusable caches, Decision 6's no-retention sentence and guarantee, and
> the Decision 7 cache bullet are amended to a zeroizing cache that holds a data
> key at most sixty seconds after its provider unwrap; the rule against fallback
> is unchanged.

## Context

[ADR#0023](./0023-secret-management-and-key-custody-direction.md) chose
OpenBao as the single provider behind separate `SecretStore` and
`KeyManagement` ports. It also kept customer-supplied keys open as a future
internal routing change in the platform secrets service.

Some enterprise customers require more than importing their key material into
infrastructure operated by the platform. They require an independent
operational veto: they must be able to disable the key or revoke the platform's
access in infrastructure they control, without platform cooperation, and have
subsequent cryptographic operations fail closed.

The term "bring your own key" is not precise enough for this requirement. A key
that a customer generates and then imports into the platform's OpenBao is BYOK
by origin, but the platform still controls whether that key remains usable. A
key retained in a customer-controlled OpenBao, AWS account, or Google Cloud
project gives the customer the required operational veto. AWS KMS and Google
Cloud KMS still hold key material inside their respective cloud security
boundaries, so this decision does not describe customer-hosted exclusive
custody.

Envelope encryption gives the three providers a useful common boundary. The
platform encrypts application data locally with a data-encryption key (DEK),
and the selected key backend encrypts only that DEK. The provider never needs
the application payload. OpenBao Transit, AWS KMS, and Google Cloud KMS can all
support that narrow operation while retaining different authentication,
resource, lifecycle, and consistency semantics.

This [ADR](../glossary/adr) amends Decision 1, the provider-artifact rule in Decision 2, and the
customer-controlled portion of Decision 6 in
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) if accepted.
OpenBao remains the `SecretStore` provider and the default `KeyManagement`
provider. The typed boundaries, service mediation, [tenant](../glossary/tenant) isolation, audit, and
fail-closed rules in
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) remain in
force. Its scheduled platform key-destruction rule continues to govern
platform-managed keys; an external customer's provider remains authoritative
for keys the customer controls.

This ADR records direction. It is not approval to implement the adapters.

## Decision

### 1. Initial adoption includes three built-in key backends

The initial implementation of this ADR supports and conformance-tests these
built-in backends:

- OpenBao Transit, as the open source and self-hosted default.
- AWS Key Management Service, using a key in the customer's AWS account.
- Google Cloud Key Management Service, using a key in the customer's Google
  Cloud project.

The platform secrets service remains the only provider client. `KeyRef` is an
opaque platform reference to the key-encryption key (KEK) that wraps DEKs.
Data-plane callers cannot select a provider, supply a provider resource name,
or parse provider ciphertext. A provider locator appears only in the authorized
administrative bind operation in Decision 4, in service-private binding state,
and in the owner's read-back of its own customer managed binding described
below, never in a business entity or ordinary operation request. Provider SDK
types and protocols do not cross the service boundary.

The tenant that owns a customer managed binding may read back that binding's
locator in the binding's detail, so it can match the binding to the key in its
provider's console and audit log. The locator never appears in a wrap or unwrap
request or response, in a projection consumed by another service or another
tenant, or for a platform-owned binding.

The `KeyRef` shown in the provider-neutral port is a service-owned internal
argument after authorization, not an authoritative caller choice. For wrap, the
service resolves it from the authenticated tenant, purpose, and tenant policy.
For unwrap, a caller may return the opaque reference stored with an envelope,
but the service treats it as untrusted input and verifies that its bound tenant
and purpose match the authenticated operation before invoking an adapter.

These are explicit adapters behind the typed `KeyManagement` port, not
dynamically loaded plugins and not a generic provider command API. Product
support means all three adapters ship and pass conformance; an individual
deployment enables only the subset it is prepared to operate. Adding a fourth
provider requires code, conformance tests, and a reviewed capability
declaration.

AWS and Google Cloud are optional, customer-selected integrations. Neither is
required for a production self-hosted deployment, and neither changes the
OpenBao licensing and self-hosting baseline established by
[ADR#0023](./0023-secret-management-and-key-custody-direction.md).

### 2. A narrow, authenticated envelope-encryption contract

The required common capability is `EnvelopeWrapping`, a typed capability within
`KeyManagement`:

```text
wrap_dek(KeyRef, Dek, AuthenticatedBinding) -> WrappedDek
unwrap_dek(KeyRef, WrappedDek, AuthenticatedBinding) -> Dek
```

`Dek` is generated by the authorized encryption caller from a cryptographically
secure random source. Its size is fixed by a versioned envelope-cipher suite and
validated before a provider call; the initial conformance suite covers a
32-byte DEK supported by all three providers. Every application-held usable
copy is owned by a single-owner secret wrapper that zeroizes its buffer on every
operation exit path. The value is never copied into application state, reusable
caches, logs, traces, or durable transports. Complete erasure of every transient
copy from process memory remains a best-effort runtime property.

`WrappedDek` is a platform-owned, versioned envelope that contains provider
ciphertext behind an adapter-private payload codec without exposing provider
markers or formats. The data-owning subsystem persists the `KeyRef`,
`WrappedDek`, binding version, and payload ciphertext together, but never parses
the wrapped value. This is the only exception to the
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) prohibition
on provider artifacts crossing the boundary: raw provider ciphertext may be
carried only as an opaque payload inside this value object and interpreted only
by its adapter.

The raw DEK crosses the service boundary only on authenticated, encrypted [NATS](../glossary/nats)
core request/reply between an authorized encryption caller and the secrets
service: into the service for wrap and back to the caller after unwrap. It never
enters [JetStream](../glossary/jetstream), another durable transport, logs, or traces. This is the same
synchronous sensitive-value boundary
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) establishes
for `SecretStore`.

`AuthenticatedBinding` is a typed, versioned, deterministic, non-secret value
derived from the authenticated tenant, the authoritative resource identity,
the key purpose, and the binding schema version. Those fields use opaque,
non-sensitive identifiers rather than customer names, emails, or payload
values. It never derives tenant or resource identity from caller-supplied
envelope fields. The identical binding is required to unwrap a DEK, preventing
a wrapped DEK from being moved to a different tenant, resource, or purpose.

Each adapter maps that value to the provider's authenticated-data mechanism:

- OpenBao Transit `associated_data` on a supported AEAD key type. OpenBao
  `context`, which controls key derivation, is not the common binding.
- AWS KMS `EncryptionContext`, which is case-sensitive and is recorded in
  plaintext in AWS audit logs. The binding therefore contains no secrets.
- Google Cloud KMS `additionalAuthenticatedData`.

Provider-side data-key generation, arbitrary encrypt/decrypt, signing, HMAC,
key creation, rotation, disablement, and destruction are not required by this
baseline. A later need adds a typed optional capability rather than widening
the baseline into a provider-shaped interface. Binding validates that a key
supports `EnvelopeWrapping`; requesting an unsupported capability returns a
typed failure before any provider call. OpenBao-specific signing or HMAC does
not become part of the three-provider baseline.

### 3. Backend availability is deployment capability; selection is runtime state

Deployment configuration declares which built-in adapters the service can
operate and supplies only their bootstrap identity references. It does not
provision every tenant backend or decide where every key lives.

A `KeyBackendRegistration` is durable product state owned by one tenant or by
the platform. It owns connection, authentication, and trust configuration, but
no key locator. It has an opaque `KeyBackendId`, a typed provider variant,
ownership, lifecycle status, admission, observed connection health, provider
access parameters, and a non-secret authentication profile. Raw credentials
never appear in configuration, events, or the registration. Workload identity
and short-lived credentials are preferred; an unavoidable secret is represented
by a `CredentialRef` and remains behind `SecretStore`.

Platform-owned registrations back the managed and dedicated tiers of
[ADR#0033](./0033-key-custody-product-model.md) and are subject to every rule
in this ADR: conformance, failure taxonomy, immutable binding, and rewrap
migration. The first implementation enables one platform-owned OpenBao Transit
registration. A platform-owned AWS KMS or Google Cloud KMS registration uses
the same adapters a customer registration uses; it is not a separate code path.

`KeyBackendId` identifies an immutable provider account, project, or OpenBao
security boundary. A credential, trust root, authorization principal, or
OpenBao endpoint update may create a new registration revision under the same
ID only when validation proves continuity with the same boundary and its bound
keys. An account, project, or unproven endpoint change requires a new
`KeyBackendId`. An OpenBao namespace is part of that security boundary: a
registration names at most one namespace, the root namespace when it names
none, and changing it requires a new `KeyBackendId`. The provider locator associated with a `KeyRef` never changes;
selecting another native key always requires a new `KeyRef` and migration,
whether or not its backend ID is shared. Lifecycle, admission, and health fields
may change without changing identity.

A registration's state is held in separate dimensions, and none of them stands
in for another:

- Lifecycle is owned by the platform: `PendingValidation`, `Active`, or
  `Retired`. A failed validation keeps the registration pending and records the
  reason. A new revision takes effect only after it validates, and the current
  revision stays in force until then.
- Admission is controlled by the registration's owner and is authoritative:
  `Enabled` or `Paused`. Only the owner may change it. A new registration starts
  `Enabled`: lifecycle alone keeps it out of use until it validates, so first
  activation needs no separate resume. Decision 6 defines what a pause does and
  does not mean.
- Observed connection health is advisory and is never an admission input. Its
  status is `Unknown`, `Connected`, or `Failed`, and a failure carries a typed
  reason: `AuthenticationFailed`, `AccessDenied`, `TlsFailure`, `Unreachable`,
  `Timeout`, `InvalidResponse`, or `CapabilityMissing`. Health also carries the
  observation time, trust expiry signals, and latency observations. The
  provider call itself fails closed, so health never needs to block anything.

Trust expiry signals are timestamps, one for each piece of trust material the
registration depends on: the endpoint's server certificate not-after, the client
certificate not-after, and the trust root not-after. When a `CredentialRef` is in
use, health also carries that credential's issued-at or expiry time. A value that
is not known stays absent and is never recorded as zero. Warning thresholds are
product policy and are not fixed by this ADR. Expiry never blocks admission.
When a certificate or trust root actually expires, the TLS handshake fails
closed.

A tenant may choose a previously bound `KeyRef` as the default wrapping key for
new envelopes of a given purpose. That default is policy for future envelopes,
not an indirection that can retarget existing ones. The first implementation
does not allocate keys in an external provider, so a backend registration alone
cannot be selected as a default key. Backend and key selection are resolved
server-side from durable state and are never accepted from a wrap or unwrap
request.

### 4. Every key is durably and immutably bound to one backend registration

After the customer provisions a provider key, an authenticated administrative
`BindExternalKey` operation accepts one `KeyBackendId`, one exact native
locator, its purpose, and its required capabilities. The service validates
identity, access, and a complete authenticated-data round trip, then returns a
new opaque `KeyRef`. Provider and locator selection is allowed only on this
authorized control-plane operation, never on wrap or unwrap.

Each `KeyRef` resolves internally to that `KeyBackendId` and provider locator.
The binding cannot change in place. Provider-native key versions may rotate
beneath the same logical provider resource when the provider guarantees old
ciphertext remains decryptable, but a default-key or deployment-config change
cannot move an existing `KeyRef`.

Apart from the owner read-back in Decision 1, the provider locators remain
adapter-private:

- OpenBao stores the registered endpoint and namespace plus the Transit mount
  and key path. The platform does not claim that an OpenBao path is an
  immutable native identifier. A stored canary bound to that key is used to
  prove continuity when connection identity or trust material changes.
- AWS stores the full KMS key ARN, never a mutable alias ARN. The adapter
  verifies the `KeyId` returned by encryption and supplies the expected key ARN
  during decryption.
- Google Cloud stores the fully qualified CryptoKey resource name. The adapter
  retains the CryptoKeyVersion name returned by encryption as adapter-private
  audit metadata and verifies the integrity fields mandated by this adapter
  contract. Symmetric decryption selects the version from the ciphertext and
  does not return the exact version used, so the adapter does not claim to
  verify that version from the decrypt response.

A multi-Region AWS key binds by the exact ARN of the key in one Region, like any
other key. The platform never fails over to a replica automatically: each
replica has its own key state and key policy, so a failover could bypass a
customer's disabling of the bound key. Moving to a replica is a new `KeyRef` and
a rewrap migration.

Moving ciphertext to another backend is an explicit migration. It creates a
new `KeyRef` bound to the new backend, unwraps with the old `KeyRef`, wraps with
the new `KeyRef`, and commits the new reference and wrapped value as one
transactional or compare-and-set mutation against the envelope's expected
version. That mutation records a stable migration operation ID. If provider
wrapping succeeds but the mutation conflicts, no partial pair is committed and
the currently committed pair remains authoritative. A retry rereads the
envelope, recognizes its operation ID as committed when applicable, and
otherwise recomputes only if no different migration has superseded it. A
partial `KeyRef` and `WrappedDek` pair is never visible. The old `KeyRef` remains
immutable and can be retired only after no envelope references it. Both
backends must be available during migration. There is no configuration-only
cutover and no fallback from one backend to another. This rewrap flow applies
only to a custody migration when the old KEK is not compromised. A suspected
compromise requires new DEKs and payload re-encryption under
[ADR#0023](./0023-secret-management-and-key-custody-direction.md), not reuse of
the existing DEKs.

### 5. Provider access preserves customer control

Provider authentication and authorization are specific to each adapter:

- OpenBao uses either JWT authentication with a narrowly bound issuer, audience,
  subject, and claims, or certificate authentication bound to the expected CA
  and certificate identity. Server TLS validation and a short explicit token
  maximum lifetime apply to either method. A
  customer-operated OpenBao can provide independent control; the platform's
  default OpenBao cannot provide a customer-owned kill switch merely because a
  key was imported into it. The service receives use permissions only, never
  restore, deletion, or key-administration permissions. The customer
  administrator configures the mount-global `disable_upsert` setting, and
  registration validation rejects the mount unless it is enabled. The platform
  token receives update but not create permission on encrypt paths, so
  soft-deleting a key cannot be followed by silent recreation.
- AWS uses a deployment-attested principal with short-lived source credentials
  to call STS `AssumeRole` into a customer-owned role with a platform-generated,
  tenant-unique external ID. The customer role's trust policy allows the exact
  deployment principal and requires that exact `sts:ExternalId`. Registration
  does not retain the role ARN until attempts with a missing and an incorrect
  external ID are both denied. Temporary credentials are scoped to the exact
  KMS key and the minimum cryptographic actions. No long-lived AWS access key is
  stored.
- Google Cloud uses Workload Identity Federation for direct access to the exact
  CryptoKey. The federation binds the expected issuer, audience, subject,
  attribute mapping, and attribute conditions without a broad workload-pool
  grant. Service-account impersonation is an allowed fallback when the
  customer's policy requires it: the federated principal can impersonate only
  that service account, and the service account can use only the exact
  CryptoKey. No service-account key is stored.

Customer-operated OpenBao is the only adapter that accepts a customer endpoint.
It requires authenticated HTTPS, hostname verification, explicit trust roots,
disabled redirects, DNS-rebinding-safe resolution, and an egress policy that
rejects loopback, link-local, metadata, and undeclared internal destinations.
AWS and Google Cloud service endpoints are derived from the verified provider
resource; tenant registrations cannot override them.

Customer-managed provider policies must not grant the platform administrative
ability to undo the customer's shutdown action. Setup validation proves access
and the complete baseline capability without broadening those permissions: a
minimum OpenBao 2.6 deployment with an `aes256-gcm96` Transit key and upsert
disabled, an AWS symmetric encrypt-and-decrypt key addressed by its exact ARN,
or a Google Cloud symmetric encrypt-and-decrypt CryptoKey with authenticated
data and the adapter-mandated CRC32C integrity exchange. Newer OpenBao versions
enter support only after the same conformance suite passes.

### 6. Customer shutdown is on every operation path

No wrap or unwrap succeeds without participation by the backend bound to the
`KeyRef`. The service does not retain a usable unwrapped DEK in application
state or cache after its in-flight operation, pre-unwrap DEKs for later use, or
fall back to a platform key when the customer backend denies or fails.

The customer stops future use through the provider they control:

- In customer-operated OpenBao, soft-delete the Transit key as the normative
  per-key kill switch. Explicitly revoking issued service tokens is an optional
  additional control. Changing or deleting an authentication role only prevents
  future login; already issued tokens remain until revoked or expired. OpenBao
  documents no bounded propagation time for soft-delete enforcement.
- In AWS KMS, disable the key or revoke the assumed role's access in the key and
  IAM policies. AWS documents disablement as almost immediate but subject to
  eventual consistency.
- In Google Cloud KMS, remove the federated principal's IAM access and disable
  every relevant CryptoKeyVersion. IAM changes are documented as consistent
  within seconds. Version disablement typically takes up to one minute but can
  exceptionally take several hours.

The guarantee is deliberately bounded by the external provider:

> Once provider enforcement is effective, no new wrap or unwrap can succeed. A
> provider request already executing before enforcement may complete. Completed
> operations and plaintext already delivered cannot be revoked.

A registration's owner can also pause it. A pause is a platform-side admission
control that the owner can reverse. It is not the veto above: the provider-side
action stays the authoritative stop, because the customer's guarantee must not
depend on the platform honoring its own flag. After a service instance observes
the pause, it makes no provider call for the registration, probes included, so
observed health is not refreshed until resume. Wrap, unwrap, `BindExternalKey`,
set-as-default, migration source use, and migration target use all fail on that
instance.

The failure is the existing `KeyUseDenied` category with the denial reason
`BackendPaused`, not a new category. The retry rule is unchanged: there is no
automatic retry, and the owner must resume the registration first. The public
service may coarsen the denial like any other.

An operation against an existing `KeyRef` checks in a fixed order. The service
authenticates the caller and resolves the reference, including its tenant and
purpose binding, requires the registration lifecycle to be `Active`, checks the
registration's admission, applies the key-state and capability matrix in
[Key States](../architecture/key-states.md), and only then calls the provider.
`PendingValidation` and `Retired` registrations stop before provider access even
when admission is `Enabled`. Every retry inside the operation deadline repeats
the lifecycle and admission checks before its next attempt.

`BindExternalKey` has no existing `KeyRef` to resolve. It authenticates the
caller, authorizes the tenant's use of the supplied `KeyBackendId`, locator,
purpose, and capabilities, requires that backend registration's lifecycle be
`Active`, checks its admission, and only then performs provider validation and
applies the discovered key-state and capability gates before creating a
`KeyRef`.

A pause is acknowledged once the admission fact is durably committed; that
acknowledgement does not claim that every service instance has observed it.
Instances enforce the pause through pushed invalidation of their admission view
with a hard time-to-live backstop, the same shape as
[ADR#0049](./0049-revocation-latency-target.md). Until measurement establishes
and records that propagation target, the platform publishes no bounded
pause-enforcement time. A stale instance may admit and start a provider request
after acknowledgement but before it observes the pause, and a request already
in flight may complete. The no-provider-call guarantee begins when the serving
instance observes the pause.

The platform never pauses a customer registration automatically from health
signals. That would turn a transient provider fault into a platform-initiated
denial and hide the real failure category.

A registration update submitted while paused is staged without validation,
because validating it would call the provider. Resume validates the newest
staged revision end to end, or the current revision when none is staged, so an
owner can replace expired trust material during a pause and resume onto it.
Admission resumes, and a staged revision becomes current, only when that
validation succeeds; otherwise the registration stays paused on its current
revision and records the validation reason. After a resume the key-state matrix
still applies.

Provider unavailability, denial, disabled keys, binding mismatch, integrity
failure, and throttling produce typed failure outcomes and never a successful
degraded mode.

The stable domain failure categories preserve security and retry semantics:

| Category | Includes | Automatic retry | Audit classification |
| --- | --- | --- | --- |
| `KeyUseDenied` | Customer authorization denial, a disabled, destroyed, or missing key, or a paused registration (denial reason `BackendPaused`) | No; external customer action, or the owner's resume of a pause, must change first | Security denial |
| `ProviderUnavailable` | Transport failure, timeout, or provider service failure | Bounded backoff within the operation deadline | Operational failure |
| `ProviderThrottled` | Explicit provider quota or rate limit | Bounded provider-directed backoff | Capacity failure |
| `BindingMismatch` | `KeyRef` tenant or purpose does not match the authenticated operation | No | Security violation |
| `TransportIntegrityMismatch` | Provider request checksum rejection or response checksum mismatch | Limited same-provider retry; persistent mismatch is terminal | Transport integrity violation |
| `IntegrityFailure` | Cryptographic authenticated-data or ciphertext validation fails | No | Cryptographic integrity violation |
| `UnsupportedCapability` | The bound key lacks the requested typed capability | No | Contract rejection |
| `InternalFailure` | Unmapped provider error, malformed response, or adapter invariant violation | No | Adapter fault |

The public service may coarsen denied, missing, and disabled outcomes to avoid a
cross-tenant resource oracle, while retaining the internal category and audit
record. Operational unavailability and throttling never collapse into a
security denial, and retries never switch providers.

Each provider attempt has a timeout no larger than the remaining operation
deadline, and an attempt timeout maps to `ProviderUnavailable`. Latency is
recorded per registration and operation kind. A published per-call latency
budget for customer backends is deferred until it is measured against real
providers, and it is then recorded the way
[ADR#0049](./0049-revocation-latency-target.md) records its target.

For a customer-controlled key, platform lifecycle state is desired and observed
state; the provider is authoritative. `Active` never asserts that the customer
has not disabled or revoked the key out of band. Reconciliation records those
changes, but a customer can bypass platform scheduling, disable immediately, or
destroy material irreversibly. Scheduled, reviewable destruction remains a
rule for platform-managed keys and platform-owned administrative APIs only.

### 7. Adoption requires one conformance suite

All three adapters must pass the same behavioral proofs before the decision is
implemented:

- A wrapped DEK cannot be unwrapped under another authenticated tenant,
  resource, purpose, or binding version.
- Wrap resolves the internal `KeyRef` from authenticated tenant and purpose
  state; unwrap authorizes a presented opaque reference against that state. No
  caller-selected reference reaches an adapter before those checks, and no
  caller can override its backend registration or provider locator.
- Administrative binding rejects the wrong provider identity, unsupported
  purpose or capability, and incomplete authenticated-data behavior.
- Every application-held DEK uses the owning zeroizing wrapper on success,
  error, cancellation, and retry paths, with no copies in application state,
  reusable caches, logs, traces, or durable transports.
- Provider denial, unavailability, and customer shutdown fail closed with no
  backend fallback.
- Each provider error maps to the stable category, retry rule, and audit
  classification above without string matching or resource-oracle leakage.
- Every provider attempt runs under a timeout no larger than the remaining
  operation deadline, an attempt timeout maps to `ProviderUnavailable`, and
  latency is recorded per registration and operation kind.
- A paused registration fails every gated operation with `KeyUseDenied` and the
  denial reason `BackendPaused` without calling the provider, a retry that
  starts after an instance observes the pause repeats admission and fails the
  same way, an update staged during the pause is validated by resume and
  becomes current only if it passes, and resume admits operations again only
  after that validation succeeds.
- AWS cross-account setup denies `AssumeRole` from an unexpected principal and
  with a missing or incorrect tenant-unique external ID, then limits the
  resulting session to the exact key.
- OpenBao deletion cannot trigger encrypt-path key recreation.
- The Google Cloud encrypt path supplies plaintext and authenticated-data
  CRC32C values, requires both verification flags, and verifies the returned
  ciphertext CRC32C. The decrypt path supplies ciphertext and authenticated-data
  CRC32C values and verifies the returned plaintext CRC32C, even though the API
  makes request checksums optional. A checksum rejection or mismatch receives
  only a limited same-provider retry; a persistent mismatch is terminal and
  audited as a transport-integrity violation.
- Explicit rewrap migration preserves decryptability and cannot become an
  implicit configuration cutover. Concurrent attempts and retries use envelope
  version checks and the migration operation ID without exposing a mismatched
  `KeyRef` and `WrappedDek` pair.
- Audit and telemetry identify the tenant, `KeyRef`, `KeyBackendId`, purpose,
  provider, outcome, and correlation ID without recording DEKs or credentials;
  they record only the approved non-secret projection of authenticated binding.
- Each operation produces one audit record that lists every provider attempt
  with its attempt number, start time, duration, outcome category, and either
  the request identifier the provider returned or an explicit absence reason,
  `NotProvidedByProvider` or `NoResponse`. The platform never synthesizes a
  request identifier, and no per-attempt value enters the authenticated binding,
  which must be identical at wrap and unwrap.
- Documentation and product copy describe a recorded identifier as joining a
  provider's own audit log, such as CloudTrail or an OpenBao audit device, only
  after a conformance case proves that join for that provider. Until then they
  say only that it is the identifier the provider returned.

Provider-specific integration tests run against supported provider
configurations in addition to contract tests. OpenBao support does not assume
that other versions or Transit key types satisfy the baseline.

## Alternatives Considered

### Keep OpenBao as the only backend and let customers import keys

Rejected for the enterprise requirement. Importing a customer-originated key
into platform-operated OpenBao changes where the key began, not who can stop
the platform from using it.

### Double-wrap every DEK with a platform key and a customer key

Rejected for the built-in adapters. Requiring the customer backend on the only
unwrap path already provides the customer's operational veto. A second layer
adds another rotation, migration, outage, audit, and recovery surface without
strengthening that requirement. Deliberate two-party cryptographic control may
justify double wrapping later, but it is a separate capability and decision, and
any reconsideration belongs to the ADR reserved for an XKS proxy adapter,
described below.

### Make OpenBao Transit dynamically pluggable

Rejected. The stable extension point is the typed `KeyManagement` port, not the
OpenBao API. Runtime-loaded provider plugins would create an unbounded trust,
compatibility, deployment, and conformance surface. The three reviewed adapters
are built-in implementations of one small contract.

### Support only customer-operated OpenBao

Rejected as the enterprise integration boundary. It would require customers
already governing keys in AWS or Google Cloud to deploy and operate another
custody system, or to export key material from the system whose independent
control they are trying to preserve. Native cloud adapters use those existing
policy, audit, and shutdown boundaries directly.

### Select one backend only through deployment configuration

Rejected. A hosted service must support different customer-owned backends at
the same time, and a tenant may need an explicit migration between them.
Deployment configuration describes available capabilities; durable domain
state owns registrations, defaults for new envelopes, and immutable per-key
bindings.

### Let each workload call the customer's provider directly

Rejected for the same reasons
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) rejects direct
OpenBao reads. It spreads high-value provider identities and authorization
logic into every workload, exposes provider details to business code, and
prevents one enforceable audit, cache, and fail-closed boundary.

### Adopt the AWS XKS proxy API as a fourth adapter

Deferred, not rejected. It belongs to its own ADR, which must include a vendor
interoperability proof. The known obstacles are that proxy authentication is a
static SigV4 credential pair, which would need a `CredentialRef` and runs
against the preference for short-lived credentials in Decisions 3 and 5, and
that a vendor's proxy may accept only AWS as the caller.

Until that ADR exists, an AWS KMS key backed by an AWS external key store, or a
Google Cloud KMS key protected by Cloud EKM, is addressed through the existing
adapter and passes the same validation. The platform makes no claim about where
its material lives.

## Non-Goals

- Changing OpenBao KV v2 as the `SecretStore` selected by
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md).
- Creating, importing, rotating, disabling, or destroying keys in customer
  provider accounts.
- Exposing a complete KMS API or provider-specific operation escape hatch.
- Supporting signing, HMAC, asymmetric keys, provider-side data-key generation,
  or arbitrary payload encryption in the first capability.
- Choosing the payload cipher, nonce construction, or complete versioned
  envelope format. That decision must exist before implementation and supplies
  the DEK size validated by this contract.
- Adding Azure Key Vault, an XKS proxy adapter, or an unreviewed provider in this
  decision.
- Promising zero-latency revocation, cancellation of provider-accepted work, or
  recall of plaintext already delivered.
- Claiming an HSM, FIPS boundary, AWS external key store, Google Cloud EKM, or
  customer-hosted exclusive custody merely because a customer-owned cloud key
  is used. This holds for product copy as well: no surface describes a tier or
  backend as HSM-backed, FIPS-validated, XKS custody, or EKM custody.

## Open Questions

These are intentionally unresolved:

- Whether a `KeyRef` may model an explicit, customer-approved replica set of a
  multi-Region AWS key. Until that is decided, each replica is a separate
  `KeyRef` and the platform never fails over between them.
- How private connectivity to a customer endpoint is provided. One constraint
  is already fixed: any private path is operator-provisioned and declared per
  registration, never a free-form customer-supplied private address, because the
  egress policy in Decision 5 rejects undeclared internal destinations.

## Consequences

- Customers can retain an independent operational veto over future decrypt and
  encrypt operations when they control the configured backend and its access
  policy.
- OpenBao remains the license-compatible default and self-hosted path, while
  hosted enterprise tenants can use keys already governed in AWS or Google
  Cloud.
- The platform accepts provider latency, quotas, regional availability, and
  policy changes on the data path. Those dependencies are reported as backend
  health and operation outcomes, never hidden by fallback.
- Provider breadth creates a permanent adapter and conformance-test obligation.
  New features must distinguish mandatory baseline behavior from optional
  provider capabilities.
- Immutable bindings make migrations explicit and auditable, but require both
  old and new providers to cooperate during rewrap. A customer who disables the
  old key before migration chooses unavailability over recoverability.
- Customer destruction of a key or key version can make every envelope still
  bound to it permanently unrecoverable. The platform cannot override that
  outcome, which is both the purpose and the operational cost of independent
  customer control.
- Customer-operated OpenBao introduces a customer-selected network dependency.
  Trust-root lifecycle, egress policy, DNS safety, and endpoint availability
  become part of onboarding and operations. Certificate and trust-root expiry
  times are carried in observed health so owners can be warned in advance, and
  an actual expiry still fails the TLS handshake closed. Private connectivity is
  listed under Open Questions.
- AWS audit logs expose the non-secret encryption context. Authenticated binding
  design and logging review must continue to treat provider audit behavior as
  part of the external contract.

## References

- [ADR#0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
- [ADR#0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [OpenBao Transit API](https://openbao.org/api-docs/secret/transit/)
- [OpenBao JWT authentication](https://openbao.org/docs/auth/jwt/)
- [AWS KMS Encrypt API](https://docs.aws.amazon.com/kms/latest/APIReference/API_Encrypt.html)
- [AWS KMS Decrypt API](https://docs.aws.amazon.com/kms/latest/APIReference/API_Decrypt.html)
- [AWS KMS encryption context](https://docs.aws.amazon.com/kms/latest/developerguide/encrypt_context.html)
- [AWS KMS key identifiers](https://docs.aws.amazon.com/kms/latest/developerguide/concepts.html)
- [AWS KMS unusable keys](https://docs.aws.amazon.com/kms/latest/developerguide/unusable-kms-keys.html)
- [AWS STS AssumeRole API](https://docs.aws.amazon.com/STS/latest/APIReference/API_AssumeRole.html)
- [AWS confused-deputy protection for third-party access](https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_common-scenarios_third-party.html)
- [Google Cloud KMS Encrypt API](https://docs.cloud.google.com/kms/docs/reference/rest/v1/projects.locations.keyRings.cryptoKeys/encrypt)
- [Google Cloud KMS additional authenticated data](https://docs.cloud.google.com/kms/docs/additional-authenticated-data)
- [Google Cloud KMS resource hierarchy](https://docs.cloud.google.com/kms/docs/resource-hierarchy)
- [Google Cloud KMS consistency](https://docs.cloud.google.com/kms/docs/consistency)
- [Google Cloud Workload Identity Federation](https://docs.cloud.google.com/iam/docs/workload-identity-federation)
- [Google Cloud services supported by Workload Identity Federation](https://docs.cloud.google.com/iam/docs/federated-identity-supported-services)
