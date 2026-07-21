# Bring Your Own Google Cloud KMS Key

This page describes the intended design recorded in
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
(accepted) and [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
and [ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) (both
draft). None of it is implemented yet. It is the model the implementation
must satisfy, not current behavior.

Use this procedure to onboard a Google Cloud KMS key as a
[customer managed key](../glossary/customer-managed-key) for your
[tenant](../glossary/tenant). Onboarding moves the key-encryption key out of
the platform's own OpenBao and into a CryptoKey you control in your own
Google Cloud project, so that you hold an independent operational veto over
future use.

## Before you begin

Confirm you have:

- Tenant admin access to the platform console or the admin API. The exact
  console screens and API request and response shapes are an open decision
  that ADR#0033 leaves to future work (ADR#0033 Non-Goals); this procedure
  describes the operations you must complete, not their exact fields.
- A Google Cloud project you control, with permission to create a CryptoKey
  and its containing key ring, and to configure Workload Identity Federation
  in that project.

> **Warning.** Destroying one of this CryptoKey's CryptoKeyVersions later
> makes every envelope still wrapped under that version's material
> permanently unrecoverable. The platform cannot override that outcome once
> you have taken it, because the customer managed tier exists precisely to
> give you that control (ADR#0030 Consequences).

## Step 1: Create the CryptoKey

1. In your Google Cloud project, create a CryptoKey with purpose
   **Symmetric encrypt/decrypt**. This is the only key shape the platform's
   envelope-wrapping contract accepts; signing, HMAC, and asymmetric keys
   are not supported by this capability (ADR#0030 Decision 2 and
   Non-Goals).
2. The platform binds the fully qualified CryptoKey resource name you
   supply in Step 3, not a shorter reference and not a specific
   CryptoKeyVersion (ADR#0030 Decision 4).
3. The adapter requires that the bound CryptoKey support authenticated data
   on every call, and it mandates the CRC32C integrity exchange that Google
   Cloud KMS itself makes optional: on encrypt, it supplies the plaintext
   and authenticated-data CRC32C values, requires both verification flags,
   and verifies the returned ciphertext CRC32C; on decrypt, it supplies the
   ciphertext and authenticated-data CRC32C values and verifies the returned
   plaintext CRC32C (ADR#0030 Decisions 2, 5, and 7). You do not configure
   this exchange yourself; it is adapter behavior on every call against your
   key, and a checksum rejection or mismatch is not treated as a routine
   retry.

## Step 2: Grant access with Workload Identity Federation

1. Start registration for this Google Cloud project in the platform console
   or admin API. Registration establishes the Workload Identity Federation
   trust the platform needs before any key is bound.
2. Configure your workload identity pool and provider to bind the exact
   issuer, audience, subject, attribute mapping, and attribute conditions
   the platform expects. Do not grant access to the broad workload pool;
   bind those exact expected values to the platform's federated principal
   specifically (ADR#0030 Decision 5).
3. By default, grant that federated principal direct access to the exact
   CryptoKey from Step 1. If your policy requires an intermediate identity,
   service-account impersonation is an allowed fallback: the federated
   principal is granted permission to impersonate one service account only,
   and that service account is granted permission to use only the exact
   CryptoKey. No service-account key is ever generated or stored, under
   either arrangement (ADR#0030 Decision 5).
4. Whichever arrangement you choose, grant use permissions only, the
   minimum permissions the platform's round trip requires to encrypt and
   decrypt. Do not grant key administration permissions, such as enabling,
   disabling, or destroying CryptoKeyVersions, or changing the CryptoKey's
   IAM policy; customer-managed provider policies must not grant the
   platform an administrative path back around your own shutdown action
   (ADR#0030 Decision 5).

## Step 3: Register the backend and bind the key

1. Complete the registration started in Step 2. The platform records your
   Google Cloud project as a `KeyBackendRegistration` with an opaque
   `KeyBackendId`. The registration owns connection, authentication, and
   trust configuration for the project; it does not yet name a key
   (ADR#0030 Decision 3).
2. Call `BindExternalKey` with that `KeyBackendId`, the fully qualified
   CryptoKey resource name from Step 1, the key's purpose, and its required
   capabilities.
3. The platform validates access through the federated principal, or the
   impersonated service account, and performs a complete
   authenticated-data round trip against the CryptoKey before it returns an
   opaque `KeyRef` (ADR#0030 Decision 4). A denied access attempt, an
   unsupported key, or a failed round trip does not produce a `KeyRef`.
4. The adapter retains the CryptoKeyVersion name that encryption returns as
   adapter-private audit metadata; it is not part of the `KeyRef` you see.
   Symmetric decrypt selects its version from the ciphertext itself, and
   Google Cloud KMS does not return the exact version used to decrypt, so
   the adapter does not claim to verify a decrypt-time version (ADR#0030
   Decision 4).

## Step 4: Make it the default for a purpose

1. Set the `KeyRef` from Step 3 as the default wrapping key for a purpose.
   The default is policy for envelopes created from this point forward; it
   is not an indirection that retargets anything already written (ADR#0030
   Decision 3).
2. Existing envelopes for that purpose keep the key they were already
   wrapped under. Moving them to the new key is a separate, explicit
   action. See [Migrate Between Key Backends](./migrate-key-backends.md).

## Your kill switch

You bring a key under the customer managed tier specifically so you can
revoke it without the platform's cooperation. In your own Google Cloud
project, do both of the following:

- Remove the federated principal's IAM access to the CryptoKey (or, under
  the impersonation fallback, remove the impersonation grant, the service
  account's access to the CryptoKey, or both).
- Disable every CryptoKeyVersion the bound `KeyRef` relies on.

Disabling is the normative kill switch here, not destroying. Destroying a
CryptoKeyVersion is the permanent-loss action already warned about above,
not a revocable control; only use it when you intend that outcome.

Once either action takes effect, every future `wrap_dek` and `unwrap_dek`
call against this `KeyRef` fails closed. The platform has no fallback key
for a customer managed key and does not attempt one (ADR#0030 Decision 6).

Google Cloud documents IAM changes as consistent within seconds. Version
disablement typically takes up to one minute, but it can exceptionally take
several hours, so allow for that when you verify the change. A request
already in flight when you revoke access or disable a version may still
complete, and plaintext already delivered to a caller before that point
cannot be recalled, by Google Cloud or by the platform (ADR#0030 Decision 6).

## See also

- [Key Management](../architecture/key-management.md)
- [Key States](../architecture/key-states.md)
- [Handle Unusable Keys](./handle-unusable-keys.md)
