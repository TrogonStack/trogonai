# Key States

This page describes the intended design recorded in
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
(accepted) and
[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) and
[ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) (both draft).
None of it is implemented yet. It is the model the implementation must
satisfy, not current behavior.

This page defines the states a `KeyRef` can occupy, the rules governing state
changes and destruction, and which operations each state allows.

## Platform-managed key states

[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6 fixes five explicit key states. They describe a
[managed key](../glossary/managed-key): an OpenBao Transit key operated by
the platform behind the secrets service.

- **Active**: the key performs `wrap_dek` for new envelopes and `unwrap_dek`
  for existing ones. This is the only state that admits new envelopes.
- **Decrypt-only**: the key still performs `unwrap_dek` for envelopes already
  wrapped under it, but no longer performs `wrap_dek`. This state lets an
  operator retire a key from producing new envelopes while existing data, and
  migration away from that data, remain readable.
- **Disabled**: the key performs neither `wrap_dek` nor `unwrap_dek`. Use is
  administratively suspended.
- **Pending destruction**: destruction has been scheduled and is under
  review. The key performs neither `wrap_dek` nor `unwrap_dek` while pending.
- **Destroyed**: the key material is gone. The key performs neither
  `wrap_dek` nor `unwrap_dek`, permanently.

Two rules govern transitions into and around these states:

- Destruction is scheduled and reviewable; it is never an immediate API
  operation ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6). [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) fixes the existence of a pending
  destruction state and a review step, but not how long a key waits in that
  state before it moves to destroyed. The duration of that waiting period is
  an open decision, left for the implementation that [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) gates behind
  its adoption proofs.
- A compromised KEK is not handled by moving between these states. It
  requires new DEKs and re-encryption of the payload under those new DEKs
  ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md) Decision 6; [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4). Rewrapping the same DEK under a
  different key changes nothing about who already holds the plaintext the
  compromised key protected, so rewrapping is not a substitute for
  re-encryption when compromise, rather than routine migration, is the
  reason for the change.

## A customer managed key's state is observed, not owned

For a [customer managed key](../glossary/customer-managed-key), the states
above describe desired and observed state, not enforced state: the bound
provider is authoritative over what the key actually permits ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
Decision 6).

Active therefore never asserts that the customer has not disabled or revoked
the key out of band. The platform's record of a customer managed key's state
is what it last observed, updated by reconciliation after a provider-side
change has already taken effect, not a control the platform enforces before
the fact. Because the provider is authoritative, the customer can disable
the key immediately, without going through the scheduled review that governs
platform-managed destruction, or destroy the underlying key material
irreversibly, and the platform cannot override either action. Scheduled,
reviewable destruction is a rule for platform-managed keys and
platform-owned administrative APIs only; it does not apply to a backend the
customer controls.

This is why the operation matrix below adds a row for customer managed keys
that the five platform-managed states do not name: a provider-side denial
observed on the next attempted operation, rather than a state the platform
itself set.

## State and operation matrix

The table applies each state to the operations a `KeyRef` participates in:

- **wrap_dek**: create a new envelope.
- **unwrap_dek**: read an existing envelope.
- **BindExternalKey / set-as-default**: complete an administrative
  `BindExternalKey` operation against the key, or select an already bound
  `KeyRef` as a tenant's default wrapping key for new envelopes of a purpose
  ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decisions 3 and 4).
- **Migration source**: use the key as the old `KeyRef` in the rewrap
  migration [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4 defines, unwrapping an envelope's `Dek`.
- **Migration target**: use the key as the new `KeyRef` in that same
  migration, wrapping the migrated `Dek`.

Each cell is `allowed` or the exact typed failure category from [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
Decision 6.

| State | wrap_dek (new envelopes) | unwrap_dek (existing envelopes) | BindExternalKey / set-as-default | Migration source | Migration target |
| --- | --- | --- | --- | --- | --- |
| Active | allowed | allowed | allowed | allowed | allowed |
| Decrypt-only | `KeyUseDenied` | allowed | `KeyUseDenied` | allowed | `KeyUseDenied` |
| Disabled | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` |
| Pending destruction | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` |
| Destroyed | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` |
| Provider denies use (customer managed key, observed) | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` | `KeyUseDenied` |

Migration source reuses the `unwrap_dek` gate, and migration target reuses
the `wrap_dek` gate, because migration is defined as an unwrap under the old
`KeyRef` followed by a wrap under the new one ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4).
Decrypt-only is the one state where those two columns diverge: a key that no
longer takes new envelopes can still be migrated away from, because unwrap
still succeeds, but it cannot receive a migration as its target, because
wrap does not.

BindExternalKey / set-as-default is gated with `wrap_dek` for the same
reason in both directions: binding validates a complete wrap and unwrap
round trip before it returns a `KeyRef` ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4), and setting a
`KeyRef` as the default only affects envelopes wrapped after the change
([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 3), so a key that cannot currently wrap cannot be validly
bound or newly selected as a default.

## Enforcement timing

[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 6 states the bound on when a state change, whether
platform-scheduled or provider-side, actually stops operations from
succeeding:

> Once provider enforcement is effective, no new wrap or unwrap can succeed.
> A provider request already executing before enforcement may complete.
> Completed operations and plaintext already delivered cannot be revoked.

That bound is set by the provider, not by the platform, and the providers
document different timing:

- Customer-operated OpenBao documents no bounded propagation time for
  soft-delete of the Transit key, which is the normative per-key kill
  switch.
- AWS documents key disablement as almost immediate but subject to eventual
  consistency.
- Google Cloud documents IAM access changes as consistent within seconds,
  and CryptoKeyVersion disablement as typically taking up to one minute, but
  exceptionally several hours.

## Effects on stored data

Envelope encryption means a destroyed or otherwise unrecoverable KEK makes
every envelope still bound to its `KeyRef` permanently unreadable: the
`WrappedDek` can no longer be unwrapped, and without the `Dek` the payload
ciphertext it protects is unreadable too. [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)'s consequences record
this outcome for customer destruction directly: the platform cannot
override it, and that is both the purpose and the operational cost of
independent customer control, not a failure to route around.

Deliberate destruction of a key still bound to live envelopes is therefore
the crypto-shredding path: destroying the KEK renders every envelope it
protects permanently unreadable without touching the stored ciphertext
itself. For a customer managed key, disabling the key or revoking the
platform's access blocks the same operations while the block persists, but
unlike destruction it is reversible if the customer restores access.

## See also

- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform
  Secrets Service](../adr/0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](../adr/0030-customer-controlled-key-backend-routing.md)
- [ADR#0033: Two-Tier Key Custody Product Model](../adr/0033-two-tier-key-custody-product-model.md)
- [Key Management](./key-management.md)
- [Key Custody](./key-custody.md)
- [Migrate Between Key Backends](../how-to/migrate-key-backends.md)
- [Handle Unusable Keys](../how-to/handle-unusable-keys.md)
- [Bring Your Own AWS KMS Key](../how-to/bring-your-own-aws-kms-key.md)
- [Bring Your Own Google Cloud KMS Key](../how-to/bring-your-own-google-cloud-kms-key.md)
- [Bring Your Own OpenBao](../how-to/bring-your-own-openbao.md)
