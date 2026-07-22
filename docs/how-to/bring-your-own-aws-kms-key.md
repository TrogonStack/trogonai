# Bring Your Own AWS KMS Key

This page describes the intended design recorded in
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
(accepted) and [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
and [ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) (both
draft). None of it is implemented yet. It is the model the implementation
must satisfy, not current behavior.

Use this procedure to onboard an AWS KMS key as a
[customer managed key](../glossary/customer-managed-key) for your
[tenant](../glossary/tenant). Onboarding moves the key-encryption key out of
the platform's own OpenBao and into a key you control in your own AWS
account, so that you hold an independent operational veto over future use.

## Before you begin

Confirm you have:

- Tenant admin access to the platform console or the admin API. The exact
  console screens and API request and response shapes are an open decision
  that [ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) leaves to future work ([ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) Non-Goals); this procedure
  describes the operations you must complete, not their exact fields.
- An AWS account you control, with permission to create a KMS key and an IAM
  role in that account.

> **Warning.** Destroying this key, or destroying its key material, later
> makes every envelope still wrapped under it permanently unrecoverable. The
> platform cannot override that outcome once you have taken it, because the
> customer managed tier exists precisely to give you that control ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
> Consequences).

## Step 1: Create the KMS key

1. In your AWS account, create a KMS key with key type **Symmetric** and key
   usage **Encrypt and decrypt**. This is the only key shape the platform's
   envelope-wrapping contract accepts; signing, HMAC, and asymmetric keys are
   not supported by this capability ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 2 and Non-Goals).
2. Choose any key material origin. AWS-generated material is recommended.
   Imported key material, a CloudHSM-backed custom key store, and an
   external key store all present the same encrypt-and-decrypt surface, and
   the origin you choose is invisible to the platform: the adapter calls the
   same encrypt and decrypt operations regardless of where the material
   lives (see [Key Custody](../architecture/key-custody.md)).
   - If you import key material, remember that AWS lets it expire or be
     deleted on a schedule you set. Plan re-binding or key rotation around
     that schedule yourself; the platform does not track it for you.
   - If you use an external key store, your external key manager sits on the
     availability path for every encrypt and decrypt call against this key.
     If it cannot serve a request, AWS KMS cannot complete the call, and
     neither can the platform.
3. Create the key as single-region unless you have an independent reason to
   replicate it. A multi-region replica key has its own key ARN in AWS, and
   the platform binds one exact key ARN per `KeyRef` ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4).
   Each replica therefore requires its own bind and becomes a separate
   binding, not one binding that follows the whole multi-region key.

## Step 2: Create the IAM role the platform will assume

1. Start registration for this AWS account in the platform console or admin
   API. The platform shows you a platform-generated `sts:ExternalId` that is
   unique to your tenant. Keep it at hand; you need it for the trust policy
   in the next step.
2. Create an IAM role in your AWS account whose trust policy allows exactly
   the platform's deployment principal to assume it, and requires exactly
   that `sts:ExternalId`. Do not widen the trust policy to another principal
   or drop the external ID condition: both together are what the platform
   verifies before it will retain the role.
3. Attach a permissions policy scoped to the one key ARN from Step 1,
   granting only the minimum cryptographic actions the platform's round trip
   requires ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 5 scopes credentials to the exact key and the
   minimum cryptographic actions, but the exact action list is not fixed by
   the ADR). Grant nothing else on this role. The platform stores no
   long-lived AWS credentials; it reaches your account by assuming this role
   from its own deployment-attested identity.
4. Enter the role ARN in the registration. The platform proves control
   before it retains that ARN: it attempts to assume the role with a
   missing `sts:ExternalId` and again with an incorrect one, and
   registration does not retain the role ARN unless both attempts are
   denied ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 5).

## Step 3: Register the backend and bind the key

1. Complete the registration started in Step 2. The platform records your
   AWS account as a `KeyBackendRegistration` with an opaque `KeyBackendId`.
   The registration owns connection, authentication, and trust
   configuration for the account; it does not yet name a key ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
   Decision 3).
2. Call `BindExternalKey` with that `KeyBackendId`, the exact key ARN from
   Step 1 (never a key alias, which is mutable and not accepted), the key's
   purpose, and its required capabilities.
3. The platform validates access through the assumed role and performs a
   complete authenticated-data round trip against the key before it returns
   an opaque `KeyRef` ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 4). A denied access attempt, an
   unsupported key, or a failed round trip does not produce a `KeyRef`.

## Step 4: Make it the default for a purpose

1. Set the `KeyRef` from Step 3 as the default wrapping key for a purpose.
   The default is policy for envelopes created from this point forward; it
   is not an indirection that retargets anything already written ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
   Decision 3).
2. Existing envelopes for that purpose keep the key they were already
   wrapped under. Moving them to the new key is a separate, explicit action.
   See [Migrate Between Key Backends](./migrate-key-backends.md).

## Your kill switch

You bring a key under the customer managed tier specifically so you can
revoke it without the platform's cooperation. In your own AWS account, do
either of the following:

- Disable the KMS key.
- Revoke the assumed role's access, by removing its permissions policy, by
  tightening its trust policy so the platform's deployment principal can no
  longer assume it, or both.

Disabling is the normative kill switch here, not destroying. Scheduling the
key for deletion, as warned above, is the permanent-loss action, not a
revocable control; only use it when you intend that outcome.

Once either action takes effect, every future `wrap_dek` and `unwrap_dek`
call against this `KeyRef` fails closed. The platform has no fallback key
for a customer managed key and does not attempt one ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 6).

AWS documents key disablement as almost immediate but subject to eventual
consistency, so allow for that when you verify the change. A request already
in flight when you disable the key or revoke access may still complete, and
plaintext already delivered to a caller before that point cannot be
recalled, by AWS or by the platform ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) Decision 6).

## See also

- [Key Management](../architecture/key-management.md)
- [Key States](../architecture/key-states.md)
- [Handle Unusable Keys](./handle-unusable-keys.md)
