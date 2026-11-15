# Portable key-wrapping contract

The `trogon-key-management` Rust crate implements the provider-independent
wrapping values and operation ports described here. Its `test-support` feature
supplies an in-memory authenticated-encryption fixture and reusable conformance
checks. The production domain has no provider SDK or generated protobuf
dependency.

This is a foundation for the design in draft
[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md), reconciled
with accepted [ADR#0083](../adr/0083-sealed-content-envelope.md). It does not
implement a secrets service, remote transport, provider adapter, registration
controller or production key custody. Those owners must compose the domain
with authentication, current policy, audit and the selected provider.

## Values and operation ports

| Value or port | Contract |
| --- | --- |
| `KeyRef` | Nonempty opaque identity. Its presentation does not authorize key use or expose a provider locator. |
| `TenantId`, `ResourceTypeId`, `ResourceId`, `KeyPurpose` | Nonempty exact strings, bounded to 256 Unicode scalar values. No trimming or normalization changes their identity. |
| `BindingVersion` | The supported binding schema version is 1. Unknown versions refuse construction. |
| `MaterialSuite` | The initial suite wraps a 32-byte data key. Unknown suites refuse conversion. |
| `Dek` | Single-owner, non-cloneable key material in a zeroizing wrapper, with redacted diagnostic output. Access to its bytes is a trusted-caller responsibility. |
| `AuthenticatedBinding` | Typed tenant, resource type, resource, purpose and version, with the exact encoding below. Construction validates shape; the service must establish authoritative provenance. |
| `WrappedDek` | Supported suite, format version 1 and an opaque adapter payload of 1 through 65,536 bytes. It contains no common provider selector or locator. |
| `WrapDek` | Consumes an owned `Dek` and wraps it under one exact admitted key and binding. |
| `UnwrapDek` | Opens an opaque wrapped value under the same exact key and authenticated binding, returning owned material. |

Each operation has its own trait and associated error type. Provider transport
evidence belongs to the owning adapter and service mapping, rather than being
flattened into an error string. The shared domain failure categories preserve
denied, unavailable, throttled, binding, transport-integrity, cryptographic
integrity, unsupported-capability and internal outcomes.

## Exact binding bytes

Version 1 encodes the following sequence:

```text
lp("trogon/key-wrapping/binding/v1")
|| u32_be(1)
|| lp(tenant)
|| lp(resource_type)
|| lp(resource)
|| lp(purpose)
```

`lp(value)` is the unsigned 32-bit big-endian length of its exact UTF-8 bytes,
followed by those bytes. Identifier limits count Unicode scalar values, while
length prefixes count bytes. Composed and decomposed Unicode remain different
identities. Field lengths prevent concatenation ambiguity, and the resource
type separates unrelated resources whose local identifiers happen to match.

These bytes match the declared wrapping wire contract in
`secrets/exchange/v1alpha1/key_wrapping.proto`. Protobuf re-encoding is not the
binding encoding. A service derives tenant/resource/purpose from authenticated
authoritative state and compares untrusted input before provider dispatch.
Constructing the Rust value alone does not perform that admission.

The data owner's sealing/share binding and any earlier experimental wrapping
codec are separate formats. Conversion requires an explicit owner-reviewed
version mapping. This contract does not auto-detect another encoding, silently
reinterpret ciphertext or retry a failed unwrap using another binding.

## Custody and use

Only an authorized trusted caller may hold a usable data key. Ordinary wrap
and unwrap calls do not authorize reusable plaintext caches. The narrow
sealing-purpose lease exception in
[ADR#0083](../adr/0083-sealed-content-envelope.md) remains with the sealing
owner and does not become a general wrapping capability.

Zeroizing owned buffers reduces residual material in memory. It does not prove
erasure of every compiler, allocator, operating-system or provider copy. Runtime
implementations must account for cancellation, errors and temporary decode or
encryption buffers without placing usable material in events, logs or traces.

The [sealed wire contract](./workload-identity-configuration.md#secret-source-and-key-wrapping-boundaries)
carries ephemeral inner requests and results inside authenticated xkey boxes
over core NATS. The Rust operation traits are not that transport. An exact
requested key cannot silently become a default or another backend after denial.
An explicitly selected default must be resolved and pinned by its authority
owner before the actual cryptographic operation.

The data owner persists the actual `KeyRef`, `WrappedDek`, binding version and
payload ciphertext as one consistent record. A rewrap migration uses an
explicit admitted target and compare-and-set update; key compromise requires
new data keys and payload re-encryption rather than only changing the wrapper.

## Conformance evidence

The reusable in-memory fixture uses AES-256-GCM to exercise binding and
ciphertext integrity through the public wrapping ports. Its setup, admission
and key-state controls exist only under `test-support`; they do not implement a
production backend registry or protected policy owner.

The suite checks exact encoding, supported versions, material and payload
bounds, altered ciphertext and wrong key/context refusal. Provider-specific
adapters must run the common cases and add actual provider authorization,
associated-data, key-state, timeout, integrity and rotation proofs. A passing
in-memory harness is not OpenBao, AWS or Google Cloud acceptance.

The shared helper requires a test-boundary classifier from the adapter's native
error to `KeyOperationError`. It checks both keys with successful round trips
before and after substitution tests. Unavailable, throttled or internal errors
cannot stand in for a binding or cryptographic integrity refusal. The classifier
and actual provider evidence remain part of the adapter review.

Follow the [adapter roadmap](../research/workload-identity/adapter-roadmap.md)
for the service admission, extraction, real backend and release dependencies.
