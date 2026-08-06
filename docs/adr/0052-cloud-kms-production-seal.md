---
number: "0052"
slug: cloud-kms-production-seal
status: accepted
date: 2026-08-05
---

# ADR#0052: Cloud KMS Auto-Unseal Is Mandatory for Production OpenBao

## Context

Everything OpenBao persists is encrypted by its barrier key, which is
wrapped by its root key. What protects the top of that chain is a seal
choice: Shamir quorum shares (the default: the root key split into shares,
typically 5 with a threshold of 3, held by humans and re-entered on every
restart), auto-unseal against a cloud KMS key (AWS KMS, GCP Cloud KMS,
Azure Key Vault), or a transit seal chained to another OpenBao.

The cloud KMS option roots the platform's entire encryption chain in
FIPS-validated HSMs: the wrapping key physically never leaves the
provider's hardware, every unwrap is an IAM-gated and audit-logged API
call, and a restart becomes an automated unwrap instead of a quorum of
humans typing shares at whatever hour a node restarts. The dependency
points outward to a third party, so it satisfies
[ADR#0033](./0033-two-tier-key-custody-product-model.md)'s rule that
platform boot keys are deployment-provisioned and never cycle back through
the platform's own services.
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) noted
that OpenBao is not an HSM and reserved the hardware-boundary question as a
new decision. This is that decision.

## Decision

**Production OpenBao MUST auto-unseal against a cloud KMS key. The Shamir
quorum seal is prohibited as the routine production seal.** This is not a
default to be weighed per deployment; it is the rule.

### 1. Providers

GCP Cloud KMS and AWS KMS are the expected providers. Azure Key Vault is an
acceptable adapter of the same posture. The KMS key lives in a
platform-controlled cloud account, with provider-default rotation and
either a multi-region key or a documented recovery path for a KMS outage.

### 2. The single exception

A deployment whose network cannot reach any cloud KMS (air-gapped or
restricted-egress environments) may use the Shamir quorum seal. The
exception is recorded per deployment with its custody ceremony documented,
and OpenBao seal migration keeps the exception reversible when the network
constraint lifts.

### 3. Recovery keys are break-glass only

Auto-unseal still generates a recovery-key quorum. Those shares exist
solely for break-glass operations under
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s
out-of-band ceremony (quorum-held shares, root token revoked after use) and
are never part of routine operation.

### 4. Development

Dev and local environments use the static or single-share dev seal per
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s
dev-mode story. The mandate applies to production only.

### 5. Provisioning

The seal stanza is deployment configuration under
[ADR#0033](./0033-two-tier-key-custody-product-model.md)'s bootstrap rule,
outside the `SecretStore` and `KeyManagement` ports. No platform code
depends on which seal a deployment uses.

## Consequences

- The unseal and key custody runbook has its shape: restarts unseal
  automatically; the runbook covers the KMS-outage path (running nodes stay
  unsealed, restarts block until KMS returns or the documented recovery is
  executed) and the break-glass recovery ceremony.
- The chain of trust reads end to end: cloud HSM wraps the OpenBao root,
  the barrier encrypts platform storage, the secrets service and
  `KeyManagement` route tenant material to managed or customer-managed
  backends per [ADR#0030](./0030-customer-controlled-key-backend-routing.md).
- Auto-unseal protects the bootstrap of the chain only. It does not protect
  a compromised running process or over-permissive API access; those are
  the service auth-method decision (still open) and OpenBao policies.
- A deliberate cloud dependency is accepted for the seal. Deployments that
  cannot accept it fall under the section 2 exception, not under a softer
  reading of the rule.

## References

- [ADR#0023: Secret Management and Key Custody Direction](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](./0030-customer-controlled-key-backend-routing.md)
- [ADR#0033: Two-Tier Key Custody Product Model](./0033-two-tier-key-custody-product-model.md)
- OpenBao seal configuration and seal migration documentation
