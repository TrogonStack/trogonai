---
number: "0022"
slug: canonical-acp-wire-methods-on-nats
status: rejected
date: 2026-07-09
---

# ADR 0022: Canonical ACP Method Vocabulary in the NATS Layer (Rejected)

## Context

A change was proposed (and briefly implemented) to rename the NATS layer's
method labels from their subject-token forms (`session.new`, bare `prompt`,
`ext.{name}`) to the ACP spec's canonical names (`session/new`,
`session/prompt`, `_{name}`). The motivations were: self-describing wire
traffic, conformance checks against the spec's `meta.json` without a
translation table, and enabling decode through the SDK's `ClientRequest`
enum.

Two facts established during implementation removed those motivations:

1. The content-mode codec (`jsonrpc-nats`) never serializes the method.
   Requests and notifications carry params in the body and an id header; the
   receiving side derives the method from the NATS subject. There is no
   method field on the NATS wire to make self-describing.
2. Enum-based decode on the NATS leg was evaluated and skipped on its own
   merits (see PLAN.md Item 2), so the rename enabled nothing.

## Decision

Rejected and reverted. On the NATS leg, the method's identity IS the subject
token. `wire_method()` and the subject suffix are deliberately the same
vocabulary: one identity serves routing, decode context, telemetry, and
ACLs, and the tests assert that unification. Introducing a parallel
"canonical label" created a second vocabulary with no wire payoff, diverged
from the metrics labels that kept the token names, and traded a NATS-native
invariant for cosmetic spec alignment.

The ACP spec's vocabulary governs the layers where methods actually flow as
JSON-RPC on a wire: the byte-stream boundaries (WebSocket, HTTP, stdio),
which have always used canonical names, including the `_{name}` extension
prefix in `ConnectionClient`. The NATS embedding (subject grammar, token
vocabulary, durability, ACL granularity) is governed by ADR 0003/0004 and
optimizes for NATS, not for spec spelling.

## Consequences

- `wire_method()` returns the subject-token vocabulary, matching
  `from_suffix`/`from_subject_suffix` and the metrics labels.
- Conformance tracking maps spec names to subject tokens explicitly in
  `docs/architecture/acp-conformance.md`, which is the correct place for the
  correspondence to live.
- Any future proposal to re-spell NATS-layer identifiers must prove a
  concrete benefit at the byte level or in operations, not vocabulary
  aesthetics.
