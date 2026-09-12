---
number: "0063"
slug: agent-connection-declarations
status: draft
date: 2026-09-11
---

# ADR#0063: An Agent Declares the Connections It Needs, and the Declaration Carries No Version

## Context

[AgentConfiguration](../glossary/agentconfiguration) owns immutable behavior and
its dependency declarations. `AgentDependencies` has collections for skills,
tools, delegates, and memories. None of them can express the most common thing a
real agent needs, which is that its behavior reaches an external system: a
source host, an issue tracker, a chat workspace, an observability backend.

The security plane is not what is missing. [ADR#0023](./0023-secret-management-and-key-custody-direction.md)
places retrievable credentials behind `SecretStore` and exposes only opaque
handles. [ADR#0047](./0047-event-sourced-credential-metadata.md) through
[ADR#0052](./0052-cloud-kms-production-seal.md) fix the credential aggregate's
events, its one-time plaintext exposure rule, its revocation latency target, and
its production seal. [ADR#0032](./0032-model-route-and-credential-binding.md)
goes further for one provider class and defines a
[ModelProviderConnection](../glossary/modelproviderconnection), a
[CredentialBinding](../glossary/credentialbinding), and an attempt-scoped
`ModelAccessGrant`, brokered through a session-scoped proxy so that the
implementation never holds upstream material.

That triple of connection, binding, and grant is the right shape, and
[ADR#0032](./0032-model-route-and-credential-binding.md) Decision 3 says in as
many words that it does not cover the rest: "channel bot tokens belong to the
channel adapter; and tool credentials belong to tool execution." Those two
sentences name owners for the credentials without giving either owner a contract
to name what it needs.

The model plane can get away with that because it has something to derive from.
A pinned adapter reads the runtime-native settings and projects exact model
selections, and admission resolves a route from the projection. Tool and channel
connections have no equivalent: nothing in a runtime's settings payload says
this behavior reaches an issue tracker in a way the platform can verify. The
requirement has to be authored, which makes it revision content, which means it
needs a field.

The [provider agent contracts research corpus](../research/provider-agent-contracts/index.md)
studied four external platforms that reached this problem first. Two findings
from it bear directly on the shape of that field, and both were reached
independently by more than one of the four, which is the only reason to weigh
them above taste.

The first is that material rotates and structural identity does not. Every
platform studied treats a credential's identity as stable across rotation and
treats the bytes as a version underneath it. That is incompatible with how every
other declaration in `AgentDependencies` works, because the others pin an
immutable version and the pin is exactly what makes them reviewable.

The second is that the value must never enter the agent's address space. Four
independent implementations surveyed in that corpus substitute the secret at
network egress from an opaque placeholder, and in all four the substitution both
sets the authenticated form and strips the placeholder rather than leaving both
present. [ADR#0032](./0032-model-route-and-credential-binding.md) Decision 4
already reaches the same conclusion for models through its supervisor and proxy.

This record fixes the declaration only. It does not define the session-scoped
grant, the connection resource for non-model providers, or the egress mediation
component, for the reasons in Decision 5.

## Decision

### 1. Add connections as a fifth dependency collection

`AgentDependencies` gains `repeated ConnectionDeclaration connections = 5`. A
declaration names a requirement class and continues to grant no authority, which
is already the stated invariant for every collection in that message and becomes
load-bearing here.

The collection is additive at a fresh tag and an empty repeated field serializes
to zero bytes, so every configuration digest already minted over an
`AgentConfiguration` without connections stays valid. Existing revisions are not
rewritten and do not need to be.

### 2. A connection declaration pins no version, deliberately

`ConnectionDeclaration` has no exact-pin arm. `ToolDeclaration` and
`DelegateDeclaration` both offer a selector or an exact pin, and
`SkillDeclaration` requires a version and a content digest outright. The
omission here is the decision, not an unfinished shape.

A pin commits a revision to an immutable version. Credential material has no
version a revision can commit to. Rotation replaces the material while the
connection remains the same connection, so a pin would name either material that
the next rotation invalidates, forcing a new [AgentRevision](../glossary/agentrevision)
for every rotation, or a version forbidden to change, which defeats rotation.
Structural identity is the part that holds still under rotation, so structural
identity is the only part a revision declares.

This is the single documented exception to pinning dependencies at admission.
Recording it as an exception is the point: the rule stays absolute everywhere
else, and a future reader finds a reason here rather than an inconsistency.

### 3. The provider is an open identifier and everything narrower is a label

`ConnectionSelector` carries a required `provider` string matched by exact,
case-sensitive equality, plus label predicates in the same form as
`ToolSelector` and `DelegateSelector`.

The provider is not an enumeration. The security plane necessarily maintains a
closed catalog of the sources it can provision, because provisioning one
requires code. That catalog belongs to the security plane. Mirroring it into the
Agent contract would make every new integration a change to that contract and to
every configuration digest that encodes it, and would put a second copy of the
catalog somewhere it is free to drift from the first.

Credential kind, environment, and account distinctions are label predicates for
the same reason. They are the security plane's vocabulary, and a declaration
that restated them would hold the drifting second copy.

### 4. Ambiguity fails admission

Multiple matching connections fail admission, including for an optional
declaration. This is the rule `ToolSelector` and `DelegateSelector` already
state, and the consequence of breaking it is worse here: an arbitrary first
match would silently pick which external system the behavior reaches and which
account it reaches it under.

An optional declaration may resolve to none. It never permits a substitution
outside the declaration, and availability is still not an authorization grant.

### 5. The grant, the connection resource, and the egress point are separate records

A declaration is inert. Something has to turn a declared requirement plus a live
authorization decision into a usable connection for the duration of one
execution, and that something is not defined here.

[ADR#0032](./0032-model-route-and-credential-binding.md) shows what the model
plane's answer looks like and it is a substantial record on its own: an
attempt-scoped grant, a confirmation key held outside the implementation, a
proxy on the hot path, and typed failure and retry rules. The research corpus
found no usable prior art for the tool and channel equivalent, because none of
the four platforms studied separates a declaration from a grant at all. Writing
that record from the model plane's shape by analogy, with no evidence that the
analogy holds, is how a contract acquires a mistake that later has to be
migrated out of durable events.

So this record deliberately stops at the declaration. It is useful on its own:
it makes a revision reviewable for what it reaches, and it is the input any
grant design needs.

## Consequences

- A revision becomes reviewable for which external systems its behavior
  reaches. That question currently has no answer anywhere in the Agent contract.
- Rotation stays a security-plane operation with no Agent-contract consequence.
  No revision is invalidated by a rotation, and no rotation requires minting a
  revision.
- The Agent contract does not learn the security plane's catalog of providers or
  credential kinds. Adding an integration stays a security-plane change.
- Existing configuration digests remain valid, so the addition needs no
  migration of minted revisions.
- A declared connection is not yet usable. Until a grant record exists, a
  declaration is a reviewable statement of intent and admission has nothing to
  resolve it against, so the field ships ahead of the mechanism that consumes it.
- The declaration's granularity is per provider plus labels, not per operation.
  Whether a revision should be able to declare that it reaches an issue tracker
  read-only, the analogue of
  [ADR#0032](./0032-model-route-and-credential-binding.md)'s `allowed_models`, is
  not answered here.
- Whether a declaration or a grant crosses into a
  [child session](../glossary/child-session) is not answered here. Starting a
  child with none is the only default that cannot silently widen access, and
  confirming it is part of the grant record.
- [ADR#0032](./0032-model-route-and-credential-binding.md)'s exclusion of
  channel and tool credentials is narrowed but not lifted. Their custody,
  mediation, and audit behavior remain separate decisions.

## References

- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0047: Event Stream as the Credential Metadata Source of Truth](./0047-event-sourced-credential-metadata.md)
- [ADR#0048: One-Time Plaintext Exposure Contract](./0048-one-time-plaintext-exposure.md)
- [ADR#0062: Runtime-Owned Settings and Platform Declarations](./0062-runtime-owned-settings-and-platform-declarations.md)
- [Provider agent contracts research corpus](../research/provider-agent-contracts/index.md)
- [Combined schema proposal](../research/provider-agent-contracts/combined-schema-proposal.md)
- [Secret Management](../architecture/secret-management.md)
