# Handle Unusable Keys

This page describes the intended design recorded in
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
(accepted) and [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
and [ADR#0033](../adr/0033-key-custody-product-model.md) (both
draft). None of it is implemented yet. It is the model the implementation
must satisfy, not current behavior.

Key management operations fail closed. Every `wrap_dek` and `unwrap_dek` call
either succeeds or returns one of eight typed failure categories; there is
never a degraded success and never a fallback to another key backend. This
page maps each category to who acts on it and what that action is.

## Failure categories and responses

The console and API request shapes for the actions below are an open
decision that [ADR#0033](../adr/0033-key-custody-product-model.md) leaves to future work: it fixes only the vocabulary
and boundaries those surfaces must respect, not their screens or request
shapes ([ADR#0033](../adr/0033-key-custody-product-model.md) Non-Goals).

| Category | Likely cause | Who acts | Action |
| --- | --- | --- | --- |
| `KeyUseDenied` | The customer disabled, revoked, or destroyed the key in the owning provider, the platform disabled a managed key, or the owner paused the backend registration (denial reason `BackendPaused`). | The key owner: the tenant for a customer managed key, the platform operator for a managed key. | Confirm whether the denial is intended. If it is, the system is working as designed. If it is not, the key owner re-enables the key or restores access in the owning provider, and operations resume without platform intervention once provider enforcement clears. For a paused registration, the owner resumes it, as described in [If a registration is paused](#if-a-registration-is-paused). There is no automatic retry. |
| `ProviderUnavailable` | Transport failure, timeout, or a provider service failure. | Platform operators. | Bounded backoff already runs inside the operation deadline. Check backend health and provider status. |
| `ProviderThrottled` | An explicit provider quota or rate limit. | Platform operators. | Bounded, provider-directed backoff already runs. Review request volume and provider quotas. |
| `BindingMismatch` | A caller presented a `KeyRef` whose tenant or purpose does not match the authenticated operation. | Platform operators. | Security violation. There is no retry. Treat this as an incident. |
| `TransportIntegrityMismatch` | A provider request checksum was rejected, or a response checksum did not match. | Platform operators. | A limited same-provider retry runs automatically. A persistent mismatch is terminal and audited. |
| `IntegrityFailure` | Cryptographic authenticated-data or ciphertext validation failed. | Platform operators. | There is no retry. Treat this as an incident. |
| `UnsupportedCapability` | The bound key does not support the required typed capability (for example, `EnvelopeWrapping`). | The key owner or administrator. | Rebind to a key that supports the required capability: select a different managed key, or register a new customer managed key through `BindExternalKey`. |
| `InternalFailure` | An unmapped provider error, a malformed response, or an adapter fault. | Platform operators. | Escalate to platform operators. |

## If a registration is paused

A `KeyUseDenied` failure with the denial reason `BackendPaused` means the owner
of the backend registration paused it. The platform never pauses a registration
automatically, so a pause always traces to an owner action. While a registration
is paused, every operation against its keys is denied, and the platform makes no
call to the provider, probes included, so the registration's observed connection
health shows what the platform last observed
([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md), Decision
6). A public response may coarsen the denial, so confirm the reason in the audit
record.

1. Confirm whether the pause is intended. If it is, the system is working as
   designed.
2. If it is not, the registration's owner resumes it. Resume validates the
   registration's current configuration end to end. The registration resumes
   only when validation succeeds; otherwise it stays paused and records the
   validation reason, so address that reason and resume again.
3. After the resume, each key's state still applies. A key that was disabled,
   or whose access was revoked, in the owning provider keeps denying use until
   the owner reverses that in the provider. See
   [Key States](../architecture/key-states.md).

## If you suspect key compromise

Do not rewrap. Rewrap moves an existing `WrappedDek` to a new `KeyRef` while
reusing the same `Dek`, which only makes sense when the old key-encryption
key (KEK) is still sound. A compromised KEK requires new `Dek`s and
re-encryption of the payloads themselves
([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md),
Decision 6). Rewrap migration applies only when the old KEK is not
compromised
([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md),
Decision 4).

To recover from a suspected compromise:

1. Bind or select a `KeyRef` backed by a sound key: a managed key, or a
   customer managed key registered through `BindExternalKey`.
2. Generate new `Dek`s for the affected payloads. The versioned envelope
   format and payload cipher suite that fix the exact `Dek` size are an open
   decision, deferred by
   [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md).
3. Re-encrypt the payloads under the new `Dek`s, and wrap those `Dek`s under
   the new `KeyRef`.
4. Retire the compromised `KeyRef` once no envelope references it.

## If key material is destroyed

Destruction is irreversible, whether it is the platform's scheduled
destruction of a managed key or a customer's direct destruction of a
customer managed key. Every envelope still bound to that `KeyRef` becomes
permanently unrecoverable. For a customer managed key, the platform cannot
override the customer's destruction: the customer's provider is
authoritative
([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md),
Decision 6). Deliberate destruction is the crypto-shredding path, a
legitimate way to render data permanently unreadable, not only a failure
mode.

The duration of the scheduled destruction waiting period for a managed key
is an open decision, deferred by
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md),
Decision 6.

If you encounter this category:

1. Verify which envelopes still reference the destroyed `KeyRef`.
2. Record the loss. Those payloads cannot be recovered by any platform
   operation.

## Cross-tenant note

Public responses may coarsen denied, missing, and disabled outcomes into one
external result, so that a caller cannot use error responses to probe
another tenant's key state
([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md),
Decision 6). Operators diagnosing an incident see the exact internal
category, one of the eight listed above, in the audit record. Do not infer
the internal category from a caller-visible error alone.

## Read the provider attempts in the audit record

The audit record for an operation lists every provider attempt, including the
retries that ran inside the operation deadline. Each attempt shows its attempt
number, start time, duration, and outcome category, plus either the request
identifier the provider returned or an explicit absence reason,
`NotProvidedByProvider` or `NoResponse`. The platform never creates a request
identifier of its own.

A recorded identifier is the one the provider returned. This page does not
claim that it matches an entry in the provider's own audit log, such as
CloudTrail or an OpenBao audit device
([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md), Decision 7).

## See also

- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform
  Secrets Service](../adr/0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend
  Routing](../adr/0030-customer-controlled-key-backend-routing.md)
- [ADR#0033: Key Custody Product
  Model](../adr/0033-key-custody-product-model.md)
- [Key Management](../architecture/key-management.md)
- [Key States](../architecture/key-states.md)
- [Key Custody](../architecture/key-custody.md)
- [Migrate Between Key Backends](./migrate-key-backends.md)
- [tenant](../glossary/tenant)
- [managed key](../glossary/managed-key)
- [customer managed key](../glossary/customer-managed-key)
