# Bring Your Own OpenBao

This page describes the intended design recorded in
[ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)
(accepted) and [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)
and [ADR#0033](../adr/0033-two-tier-key-custody-product-model.md) (both
draft). None of it is implemented yet. It is the model the implementation
must satisfy, not current behavior.

Use this procedure to onboard a Transit key in your own OpenBao deployment
as a [customer managed key](../glossary/customer-managed-key) for your
[tenant](../glossary/tenant). Onboarding moves the key-encryption key out of
the platform's own [OpenBao](../glossary/openbao) and into an OpenBao
deployment you operate, so that you hold an independent operational veto
over future use. Customer-operated OpenBao is the only customer managed
backend that accepts a customer endpoint, so onboarding it carries
requirements the AWS KMS and Google Cloud KMS adapters do not (ADR#0030
Decision 5).

## Before you begin

Confirm you have:

- Tenant admin access to the platform console or the admin API. The exact
  console screens and API request and response shapes are an open decision
  that ADR#0033 leaves to future work (ADR#0033 Non-Goals); this procedure
  describes the operations you must complete, not their exact fields.
- An OpenBao deployment you operate, reachable from the platform's
  deployment over a network path you control.

Because you operate this OpenBao yourself, its availability becomes part of
every `wrap_dek` and `unwrap_dek` call for a key bound to it. If your
OpenBao cannot serve a request, neither can the platform, and there is no
fallback to a platform-operated key (ADR#0030 Decision 6).

Registration enforces requirements this adapter does not ask of AWS KMS or
Google Cloud KMS, because it is the only adapter that accepts a customer
endpoint:

- A minimum OpenBao version of 2.6. A newer version enters support only
  after it passes the platform's conformance suite (ADR#0030 Decision 5).
- A Transit mount with the mount-global `disable_upsert` setting enabled.
  Registration rejects the mount otherwise (ADR#0030 Decision 5).
- Endpoint rules on the connection itself: authenticated HTTPS, hostname
  verification, explicit trust roots, disabled redirects,
  DNS-rebinding-safe resolution, and an egress policy that rejects
  loopback, link-local, metadata, and undeclared internal destinations
  (ADR#0030 Decision 5).

> **Warning.** Destroying this Transit key, or destroying its key material,
> later makes every envelope still wrapped under it permanently
> unrecoverable. The platform cannot override that outcome once you have
> taken it, because the customer managed tier exists precisely to give you
> that control (ADR#0030 Consequences).

## Step 1: Create the Transit key

1. In your OpenBao deployment, create a Transit key with type
   `aes256-gcm96`. This is the AEAD key type the platform's authenticated
   binding requires: the adapter maps the platform's `AuthenticatedBinding`
   onto OpenBao Transit's `associated_data` on this key type, not onto
   `context`, which controls key derivation instead (ADR#0030 Decision 2).
2. Create the key in a Transit mount that has the mount-global
   `disable_upsert` setting enabled. Registration validates this setting
   and rejects the mount if it is disabled (ADR#0030 Decision 5).

## Step 2: Configure authentication for the platform

1. Choose an authentication method for the platform's connection to your
   OpenBao: JWT authentication with a narrowly bound issuer, audience,
   subject, and claims, or certificate authentication bound to the
   expected CA and certificate identity. Server TLS validation and a
   short, explicit token maximum lifetime apply to either method (ADR#0030
   Decision 5).
2. Grant the platform's authentication role use permissions only. Never
   grant restore, deletion, or key-administration permissions.
3. On encrypt paths, grant the platform's token update permission but not
   create permission. This is what keeps a soft-deleted key from being
   silently recreated: the platform can continue to use a key that already
   exists, but it cannot bring a deleted key back by writing to its path
   again (ADR#0030 Decision 5).

## Step 3: Register the endpoint and bind the key

1. Start registration for this OpenBao deployment in the platform console
   or admin API. The registration records your OpenBao deployment as a
   `KeyBackendRegistration` with an opaque `KeyBackendId`. The registration
   owns connection, authentication, and trust configuration for the
   deployment, including its endpoint and namespace; it does not yet name a
   key (ADR#0030 Decision 3).
2. The platform does not claim that an OpenBao path is an immutable native
   identifier, the way it treats an AWS key ARN or a Google Cloud CryptoKey
   resource name. Instead, a stored canary bound to the key proves
   continuity when connection identity or trust material changes (ADR#0030
   Decision 4).
3. Call `BindExternalKey` with the registration's `KeyBackendId`, the exact
   Transit mount and key path from Step 1, the key's purpose, and its
   required capabilities. The platform validates access and performs a
   complete authenticated-data round trip against the key before it
   returns an opaque `KeyRef` (ADR#0030 Decision 4). A denied access
   attempt, an unsupported key, or a failed round trip does not produce a
   `KeyRef`.

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
revoke it without the platform's cooperation. In your own OpenBao
deployment, soft-delete the Transit key. This is the normative per-key kill
switch for a customer-operated OpenBao backend (ADR#0030 Decision 6).

Once soft-deletion takes effect, every future `wrap_dek` and `unwrap_dek`
call against this `KeyRef` fails closed. The platform has no fallback key
for a customer managed key and does not attempt one (ADR#0030 Decision 6).

Explicitly revoking the platform's issued service tokens is an optional
additional control, not a substitute for soft-deleting the key. Changing or
deleting the authentication role you configured in Step 2 only prevents
future logins; a token already issued to the platform remains valid until
it is revoked or it expires on its own (ADR#0030 Decision 6).

OpenBao documents no bounded propagation time for soft-delete enforcement,
so allow for that when you verify the change. A request already in flight
when you soft-delete the key may still complete, and plaintext already
delivered to a caller before that point cannot be recalled, by OpenBao or
by the platform (ADR#0030 Decision 6).

Destroying the key material, as distinct from soft-delete, is permanent
loss. It is not a kill switch you can reverse: every envelope still bound
to the key becomes permanently unrecoverable, and the platform cannot
override that outcome (ADR#0030 Consequences).

## See also

- [Key Management](../architecture/key-management.md)
- [Key States](../architecture/key-states.md)
- [Handle Unusable Keys](./handle-unusable-keys.md)
