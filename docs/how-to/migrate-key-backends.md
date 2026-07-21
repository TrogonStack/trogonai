# Migrate Between Key Backends

This page describes the intended design recorded in
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
(accepted) and
[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) and
[ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) (both draft).
None of it is implemented yet. It is the model the implementation must
satisfy, not current behavior.

## When to use this

Use this procedure to move a tenant's existing envelopes from one bound key
to another. This is a custody change, not an incident response. Typical
reasons to migrate: a tenant adopts a
[customer managed key](../glossary/customer-managed-key) after starting on
the [managed key](../glossary/managed-key) tier, a tenant moves between two
customer managed backends, or a tenant leaves a customer managed key and
returns to the managed tier.

Do not use this procedure if you suspect the old key-encryption key (KEK) is
compromised. Rewrapping under a new key does not change who already had
access to the plaintext `Dek` the old key protected, so a suspected
compromise requires new `Dek` values and re-encryption of the payload, not a
rewrap of the existing ciphertext (per [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4). See
[Handle Unusable Keys](./handle-unusable-keys.md) for that procedure instead.

## Preconditions

Before you start a migration, confirm all of the following:

- Both the old backend and the new backend must be available for the entire
  migration. There is no partial-availability path: if either backend cannot
  serve `wrap_dek` or `unwrap_dek` requests, the migration cannot proceed.
- The new key does not need to be bound ahead of time through a separate
  procedure. Binding it through `BindExternalKey` is the first step of the
  migration flow described below, and it must complete before any envelope's
  `Dek` is rewrapped under the new `KeyRef` (per [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4).
- If a customer disables the old key before migration completes, they have
  chosen unavailability over recoverability. The platform cannot unwrap the
  envelopes still referencing a disabled old key, and it cannot recover them
  by any other means.
- The source key must still allow unwrap (active or decrypt-only), and the
  target key must allow wrap (active). See
  [Key States](../architecture/key-states.md) for the full state and
  operation matrix.

## How migration works

Migration follows the rewrap flow that [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4 defines. The
console and API request shapes that trigger it are an open decision that
[ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) leaves for
later work; what follows is the operation sequence the implementation must
satisfy, not a concrete request format:

1. Bind the new key, producing a new `KeyRef`. This step must complete before
   any envelope migrates.
2. Cut over new writes before rewrapping anything: set the new `KeyRef` as
   the default wrapping key for every purpose whose default is currently the
   old `KeyRef`. Defaults are resolved server-side on wrap
   (per [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
   Decision 3), so after this step no new envelope can be wrapped under the
   old `KeyRef`, and the set of envelopes to migrate stops growing. A wrap
   already in flight when the default changes may still commit under the old
   `KeyRef`; such operations are bounded by their operation deadline, and the
   completion check below catches anything they commit.
3. For each envelope, unwrap its `Dek` with the old `KeyRef`, then wrap that
   same `Dek` with the new `KeyRef`, producing a new `WrappedDek`.
4. Commit the new `KeyRef` and the new `WrappedDek` together, as one
   transactional or compare-and-set mutation against the envelope's expected
   version. That mutation records a stable migration operation ID.
5. If the mutation conflicts, nothing partial is committed and the
   currently committed pair remains authoritative. A retry rereads the
   envelope: it recognizes its own operation ID as already committed when
   applicable, and otherwise recomputes only if no different migration has
   superseded it.
6. A mismatched `KeyRef` and `WrappedDek` pair, one that does not name the key
   that actually produced it, is never visible to any reader.

## Rules

- There is no configuration-only cutover between backends, and no fallback
  from one backend to another. Every backend change goes through this
  explicit rewrap.
- Setting the default wrapping key for new envelopes and migrating existing
  envelopes are separate actions. Changing a tenant's default affects only
  envelopes created after the change; it does not migrate anything already
  written. In a migration they compose in a fixed order: the default cutover
  comes first, so the set of envelopes referencing the old `KeyRef` stops
  growing before the rewrap and the final reference scan.
- The old `KeyRef` stays immutable: its binding to a backend and provider
  locator never changes in place. It is retired only after no purpose still
  selects it as a default wrapping key and no envelope references it any
  longer.

## Verifying completion

- Confirm that no purpose still selects the old `KeyRef` as its default
  wrapping key. The cutover in the migration flow makes this true before any
  envelope is rewrapped; this check guards against a default added
  concurrently.
- Run the reference scan only after wrap operations that could have resolved
  the old default before the cutover have drained, which their operation
  deadline bounds. Then confirm that no envelope still references the old
  `KeyRef`.
- Retire the old `KeyRef` only after both checks pass.
- Expect the audit record for each migrated envelope to carry the tenant,
  the `KeyRef`, the `KeyBackendId`, the purpose, the provider, the outcome,
  and a correlation ID (per [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 7).

## See also

- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](../adr/0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](../adr/0030-customer-controlled-key-backend-routing.md)
- [ADR#0033: Two-Tier Key Custody Product Model](../adr/0033-two-tier-key-custody-product-model.md)
- [Key Management](../architecture/key-management.md)
- [Key States](../architecture/key-states.md)
- [Key Custody](../architecture/key-custody.md)
- [Handle Unusable Keys](./handle-unusable-keys.md)
- [Bring Your Own AWS KMS Key](./bring-your-own-aws-kms-key.md)
