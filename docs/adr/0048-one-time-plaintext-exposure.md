---
number: "0048"
slug: one-time-plaintext-exposure
status: accepted
date: 2026-08-05
---

# ADR#0048: One-Time Plaintext Exposure Contract

## Context

Two open questions gate the credential platform's API contracts: whether an
idempotent replay of a create or reroll may re-serve the one-time plaintext it
returned the first time, and whether any server-side escrow of one-time
material (holding the plaintext briefly so a client can fetch it again) is
allowed at all. Both questions trade user convenience against widening the set
of places and moments where raw secret material exists.

The platform's response rules already commit to metadata-only reads, and the
scoped idempotency ledger stores response snapshots for replay. Whatever
those snapshots contain is retained for the idempotency TTL and replayed to
anyone who can present the key, so their content decides both questions
mechanically.

## Decision

Plaintext appears exactly once, in the direct response to the request that
created it. Everything else is metadata.

- Create, rotate, resubmit, and API-key reroll responses are the only
  surfaces that ever carry generated or submitted plaintext, and only in the
  immediate response to the winning request.
- Idempotency response snapshots are metadata-only by construction. A replay
  under the same idempotency key returns the same operation, resource ids,
  and status, and never the plaintext, even for the caller who originally
  received it.
- No server-side escrow of one-time material exists in any form.
- Recovery from a lost one-time response is reroll (Trogonai-issued keys) or
  resubmission (provider-supplied secrets), never recovery of the original
  value.

## Consequences

- The idempotency record shape stays metadata-only by contract. The KV ledger
  stays inside security-test and audit coverage, which verifies that no
  plaintext ever enters its schema or replay path.
- The UI must state plainly that a value shown once cannot be recovered and
  offer reroll or resubmit as the recovery action.
- Retry-safety guidance for API clients: capture the plaintext from the
  first successful response; a retry that lands as a replay will not carry
  it again.
- Security tests assert that no idempotency snapshot, log, trace, metric, or
  event payload contains plaintext.

## References

- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
