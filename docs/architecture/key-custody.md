# Key Custody

Every encrypted payload sits at the top of a chain of keys, and every such
chain must end somewhere. This page explains that chain for the platform's
encryption model and why it terminates where it does. The decisions themselves
live in [ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md),
[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md), and
[ADR#0033](../adr/0033-key-custody-product-model.md); this page is
the background that makes them legible.

## The custody chain

The platform uses envelope encryption
([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)): a
payload is encrypted locally under a data-encryption key (DEK), and the DEK is
wrapped by a key-encryption key (KEK) held in a key backend. The backend sees
only DEKs, never payloads. Reading data therefore walks a chain downward:

> payload ciphertext → wrapped DEK → KEK in a key backend → whatever protects
> that backend's keys

The last hop is the interesting one. Wrapping a key with another key only
moves the question of protection; it never answers it.

## Every chain terminates outside encryption

The bottom key of any key-management system cannot itself be encrypted. It can
only be protected by something that is not encryption: hardware, human
ceremony, or attested identity.

- **AWS KMS** generates and uses key material inside a fleet of hardware
  security modules. AWS documents that, at rest, this material is wrapped
  under regional domain keys that exist only inside the HSM fleet and are
  distributed between HSMs through an operator ceremony (see the
  [AWS KMS cryptographic details](https://docs.aws.amazon.com/kms/latest/cryptographic-details/intro.html)
  documentation). "Customer managed" in AWS governs a key's
  policy, lifecycle, and access; physical custody terminates in AWS hardware
  in every AWS management tier. AWS also lets a key's material originate
  elsewhere (imported material, a CloudHSM-backed store, or an external key
  store), which relocates the termination point without changing the product
  model above it. The key shape the platform's envelope contract requires, a
  symmetric encrypt-and-decrypt key, is available in every origin, and the
  origin never changes the encrypt and decrypt API an adapter calls, so a
  bound key's origin is invisible to the platform. In an external key store,
  AWS documents that ciphertext is double-encrypted, first under AWS-held key
  material and then under the external manager's key, so neither party alone
  can decrypt it.
- **[OpenBao](../glossary/openbao)** encrypts everything it stores, including
  Transit keys, under its barrier keyring. The keyring is encrypted by the
  root key, and the root key is protected by the seal: quorum-held Shamir
  shares of the unseal key, or auto-unseal against another KMS. Production
  mandates the cloud KMS auto-unseal
  ([ADR#0052](../adr/0052-cloud-kms-production-seal.md)). OpenBao is not an
  HSM, and unsealed key material exists in server
  process memory
  ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)).
- **The platform** reaches OpenBao through deployment-attested identity, never
  a stored secret
  ([ADR#0023](../adr/0023-secret-management-and-key-custody-direction.md)). On
  the platform side, the chain terminates in the deployment platform plus the
  operator's unseal ceremony.

## The custody tiers on that chain

[ADR#0033](../adr/0033-key-custody-product-model.md) defines the
customer-facing tiers by two properties of the KEK hop: whether the key wraps
one tenant's envelopes or many tenants' envelopes, and who can stop the
platform from using it.

- With a **[managed key](../glossary/managed-key)**, the KEK is a shared key
  operated by the platform in its own registered backend, OpenBao Transit by
  default. Tenants are kept apart by the authenticated binding on every wrap
  and unwrap, not by separate keys. The chain terminates in the platform
  operator's OpenBao barrier keyring and its seal: in production, mandatory
  auto-unseal against a platform-controlled, HSM-backed cloud KMS key
  ([ADR#0052](../adr/0052-cloud-kms-production-seal.md)). The Shamir quorum
  seal survives only under that ADR's air-gapped exception; auto-unseal still
  generates a recovery-key quorum for break-glass administrative operations,
  and those shares cannot unseal the cluster.
- With a **[dedicated key](../glossary/dedicated-key)**, the KEK sits in the
  same platform backend but wraps one tenant's envelopes only. The chain
  terminates in the same place; what changes is that rotation and destruction
  of that key touch one tenant, so the platform can cryptographically erase
  one tenant without affecting another.
- With a **[customer managed key](../glossary/customer-managed-key)**, the KEK
  lives in a backend the customer controls: their AWS KMS key, their Google
  Cloud KMS key, or their own OpenBao. The platform holds only wrapped DEKs,
  and the chain terminates wherever the customer's provider terminates it.
  Disabling the key or revoking the platform's access in that backend fails
  all future wrap and unwrap operations closed
  ([ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)).

No tier makes hardware or exclusive-custody claims beyond its backend; those
bounds are recorded in [ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md)'s non-goals.
The platform may move its own backend to a cloud KMS registration without
changing the tiers, because the tier is the registration's owner and
dedication, never the vendor ([ADR#0033](../adr/0033-key-custody-product-model.md)).

## Why platform boot keys stay off this chain

The chain above exists only once the secrets service is up, and the secrets
service is reached over authenticated [NATS](../glossary/nats): callers
present user JWTs minted with the `a2a-auth-callout` signing keys
([ADR#0017](../adr/0017-aauth-agent-authentication.md)). If those signing
keys, or any credential on the cold-start path, were custodied behind the
secrets service, the platform could not boot: the service would guard the keys
required to reach it.

[ADR#0033](../adr/0033-key-custody-product-model.md) therefore fixes
a strict dependency order:

> deployment-attested identity → OpenBao → secrets service (`SecretStore`) →
> `KeyManagement`, which routes each operation to exactly one backend
> registration: the platform's own (managed and dedicated keys) or a
> customer's (customer managed keys)

Each stage may depend only on earlier stages, and the two registration
families are alternatives behind the `KeyManagement` port: an operation
against a customer managed key never routes through, or depends on, a
platform-owned key. Platform bootstrap material comes from the deployment
([ADR#0007](../adr/0007-configuration-sources.md)), and business key
management wraps [tenant](../glossary/tenant) business payloads only.

## See also

- [Key Management](./key-management.md)
- [Key States](./key-states.md)
