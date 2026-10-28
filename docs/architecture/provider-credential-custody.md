# Provider Credential Custody

Provider management owns connection configuration and permission to use a
credential. The secrets service owns credential material. Keeping that boundary
in the contracts lets a connection use either platform-managed storage or an
authorized external secret source without turning the provider event stream
into another secret store.

This page explains the target boundary for the model-provider contracts under
draft [ADR#0032](../adr/0032-model-route-and-credential-binding.md). It does not
accept that ADR, finalize external secret adapters, or claim that the standalone
secrets service is deployed. Accepted
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) fixes
custody, and accepted
[ADR#0047](../adr/0047-event-sourced-credential-metadata.md) fixes stream
ownership of credential metadata. Proposed
[ADR#0074](../adr/0074-credential-platform-extraction-boundary.md) describes
the unfinished extraction from the gateway prototype.

## Streams reconstruct authority, not material

A provider connection stream can retain connection identity, hierarchy
placement, immutable configuration digests, binding identity, an opaque secret
reference, and lifecycle facts. Replaying it reconstructs which configuration
was admitted and which binding is eligible. It never resolves a reference,
contacts an upstream provider, or repeats a secret write.

Neither plaintext nor encrypted credential values belong in provider commands
that are durably retained, events, snapshots, projections, outboxes, operation
records, or replay fixtures. The exclusion also covers encoded values, wrapped
data keys, and ciphertext envelopes. Encrypting a credential before appending
it would still create another durable material store with its own key-retention
and deletion obligations.

The secrets subsystem has its own metadata aggregate and write saga. Its events
also exclude material. Secret bytes and any backend-owned encryption envelope
remain in the secret backend. This rule is scoped to credential custody; it
does not repeal the separate encrypted-data envelope contract in
[Key Management](./key-management.md).

## A dependency contract for provider management

For stored or externally sourced credentials, provider management should depend
on an opaque reference issued or registered by the secrets service. A reference
is an identity, not a bearer authorization:
its use still requires authenticated tenant, purpose, connection, and lifecycle
checks. Tenant identity comes from authenticated context, never from the
reference or an untrusted request field.

The dependency must provide authorized registration/storage, live resolution
or a purpose-specific operation, metadata inspection for reconciliation,
credential replacement, revocation, and bounded invalidation. A model-access
component may briefly need resolved plaintext to authenticate an upstream
request. That use occurs on the authorized runtime path, outside event replay
and durable configuration distribution.

These are required guarantees, not evidence that every operation is implemented
or ready for use. Until the dependency is available and validated, provider
activation or resolution fails closed. There is no fallback that adds a raw
value, ciphertext, environment-variable locator, or provider-native secret path
to a provider stream.

The reference type and its issuance rules still need agreement with the secrets
work. Do not publish a new provider-specific reference format to get around
that dependency. Contract design can proceed around the reference boundary
without implementing encryption or secret storage in the provider aggregate.

Workload identity and other authentication methods without stored secret
material retain their own typed bindings under draft ADR#0032. They do not
require uploading a fabricated secret to fit this boundary.

## Supplied material uses a separate ingress

A supplied API key enters a dedicated write-only secrets operation. Its request
has a secret-bearing type that does not appear in provider events or response
records. The service returns an opaque reference and safe operation metadata;
provider registration then uses that reference.

The write model remains pending metadata, backend write, and metadata-only
activation. The pending event is not a serialized copy of the ingress request.
An interrupted write is reconciled using backend metadata and stable operation
identity. If the write never reached the backend, recovery cannot recreate the
value from an event; the authorized caller must resubmit it. If the write
succeeded but the response was lost, a retry must recover that result without
creating a conflicting version.

Secret activation and provider-binding admission are separate outcomes. A
stored value does not prove provider identity or model compatibility. The
model-access admission path still validates endpoint, provider subject, purpose,
and live authorization before making the binding usable.

See [Supplied Credential Material](../reference/supplied-credential-material.md)
for proposed protobuf requirements and the evidence behind them.

## External storage stays behind the same reference

An external source is registered with the secrets service using an authorized
secret-manager connection. That subsystem owns provider-specific locators,
backend authentication, version-selection policy, and any permissions needed
to resolve the source. Provider management receives the resulting opaque
reference. It does not copy the external backend's location or credentials into
its own stream.

External secret storage and external encryption-key custody are different
capabilities. Draft [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
describes key-backend routing; it does not establish an implemented adapter for
reading arbitrary externally stored API keys. External source registration,
reference identity, pinned versus current-version selection, and rotation
ownership remain design work. An external update must not bypass validation of
the expected provider subject and billing boundary.

## Encryption policy is not credential content

The credential is addressed by a secret reference. Its protection policy may
select platform-managed encryption or an authorized customer-managed key
reference. The backend owns the actual encryption envelope and reports
protection metadata where supported. Requested policy and observed protection
are separate facts; successful encryption must not be inferred from a request.

Key references must respect the bootstrap and custody boundaries in
[Key Custody](./key-custody.md). An encryption root must not become an ordinary
secret whose own decryption recursively depends on the same unavailable service.
Choosing encryption policy does not require adding key bytes, algorithm knobs,
or an envelope to the provider contract.

## Rotation has different owners

Rotating material inside an encryption key, changing the logical key protecting
a secret, and replacing an API key are distinct operations. Provider management
does not implement cryptographic rotation. The secrets backend and its key
manager own encryption and any required rewrap or migration. The secrets
service coordinates credential-version eligibility, validation, invalidation,
and revocation. Automatically issuing or revoking an upstream API key requires
a supported provider integration; storing a supplied value does not imply that
capability.

AWS KMS rotation retains the logical key identity and automatically chooses
older key material for existing ciphertext. It does not re-encrypt existing
data. Google Secret Manager uses the new primary Cloud KMS version for new
secret versions; existing versions still require their original key versions.
[AWS KMS rotation](https://docs.aws.amazon.com/kms/latest/developerguide/rotate-keys.html),
[Google Secret Manager CMEK](https://docs.cloud.google.com/secret-manager/docs/cmek).

Changing a secret's logical encryption key also differs by backend. AWS Secrets
Manager re-encrypts versions labelled `AWSCURRENT`, `AWSPENDING`, and
`AWSPREVIOUS` when permissions permit, retaining old-key access. Other history
can still depend on the previous key. Google's CMEK configuration changes
apply to future secret versions. Neither operation proves that an old key is
safe to retire.
[AWS key changes](https://docs.aws.amazon.com/secretsmanager/latest/userguide/manage_update-encryption-key.html),
[Google CMEK changes](https://docs.cloud.google.com/secret-manager/docs/cmek).

For credential values, AWS provides managed integrations and Lambda rotation
workflows. Google's general rotation schedule emits a Pub/Sub notification for
a subscriber to act on; specialized managed integrations also exist. A schedule
firing is not proof that a credential changed or passed validation.
[AWS secret rotation](https://docs.aws.amazon.com/secretsmanager/latest/userguide/rotating-secrets.html),
[Google rotation schedules](https://docs.cloud.google.com/secret-manager/docs/secret-rotation),
[Google managed Cloud SQL rotation preview](https://docs.cloud.google.com/secret-manager/regional-secrets/set-up-automatic-rotation-for-cloudsql-secrets-rs).

Under draft ADR#0032, validated replacement of material behind the same stable
reference and provider subject does not rewrite a session route. Changing the
authentication method, reference, or provider identity follows the explicit
binding/connection replacement rules. This distinction lets provider streams
record lifecycle and admission decisions without duplicating secret versions
or maintaining a second encryption-key inventory.

## Evidence required before activation

Reference-only schemas are a boundary we can establish now. Operational
readiness still requires evidence that values cannot enter durable transport or
diagnostics, retries reconcile correctly, tenant/purpose checks reject foreign
references, and rotation/revocation invalidate use within the agreed bound.
Backend loss must fail closed, and retained versions must remain recoverable
under the declared key-retirement policy. Existing source and ADR declarations
are not substitutes for those runtime proofs.
