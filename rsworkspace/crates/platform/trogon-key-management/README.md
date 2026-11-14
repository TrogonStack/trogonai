# Portable envelope wrapping

This crate provides the provider-independent wrapping domain and separate
`WrapDek` and `UnwrapDek` ports. Production dependencies contain no provider SDK,
protobuf runtime or transport client. The `test-support` feature adds an
authenticated-encryption fixture and shared conformance checks.

The [contract reference](../../../../docs/reference/key-wrapping.md) defines
exact binding bytes, bounds and evidence limits. The
[adapter roadmap](../../../../docs/research/workload-identity/adapter-roadmap.md)
records the service and provider integration still needed under draft
[ADR#0030](../../../../docs/adr/0030-customer-controlled-key-backend-routing.md)
and accepted [ADR#0083](../../../../docs/adr/0083-sealed-content-envelope.md).

## Domain boundary

Construct the tenant, resource type, resource, purpose and binding version from
authenticated authoritative context. `AuthenticatedBinding::new` composes those
validated values; it does not authenticate their source. `KeyRef` is an opaque
identity that the owning service must resolve and admit for that exact scope
and operation before invoking a provider.

`Dek` owns 32 bytes through a zeroizing, non-cloneable wrapper. Its constructors
consume zeroizing input. Trusted callers can explicitly access the bytes, so
they must avoid unowned copies, serialization, caching and diagnostic logging.
Both `Dek` and `WrappedDek` redact material in diagnostic output. Zeroization
does not promise erasure of every transient compiler, allocator or platform copy.

`WrappedDek` retains the supported suite, format version and bounded opaque
adapter payload. It does not add a provider marker, locator, automatic legacy
decoder or fallback. An existing sealing/share binding has a separate owner and
format; integrating it requires an explicit versioned conversion.

The operation traits consume or return owned key material and expose an
associated typed error. Authentication, current backend registration, key-state
policy, original deadlines, custody, encrypted core NATS transport, audit,
retries and result-release checks remain with the service and adapter owners.
No production key creation, signing, arbitrary encryption or data-key cache is
introduced by these ports.

## Local verification

From the Rust workspace:

```sh
mise exec -- cargo test -p trogon-key-management --all-features
mise exec -- cargo clippy -p trogon-key-management --all-targets --all-features -- -D warnings
mise exec -- cargo fmt -p trogon-key-management -- --check
```

Public tests cover the exact binding vector, Unicode/byte distinctions, closed
versions, material/payload bounds and redaction. The feature-gated fixture
exercises key/context substitution, ciphertext integrity, local admission and
key-state refusal. Its setup controls are test-only and do not implement a
protected production registration authority.

A real adapter must run the common contract checks with valid positive controls
and typed refusal classification, then prove its own provider authorization,
associated-data behavior, errors, cancellation, key lifecycle and audit joins.
The in-memory fixture is not evidence that OpenBao, AWS KMS or Google Cloud KMS
is integrated or qualified.
