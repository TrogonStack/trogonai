---
number: "0070"
slug: agent-offered-interface-and-throughput-limits
status: draft
date: 2026-10-16
---

# ADR#0070: An Agent Declares What It Offers and Whose Authority It Carries; Throughput Ceilings Stay on the Policy Plane

## Context

[ADR#0025](./0025-agent-definition-data-ownership.md) admits a datum into
[AgentConfiguration](../glossary/agentconfiguration) only when it is the Agent's
own behavior or dependency declaration, every session pinned to the revision
should begin with it, and changing it should invalidate behavioral
verification. Everything the configuration holds today looks inward: runtime
settings, skill pins, and tool, delegate, memory, and connection declarations
under [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md)
and [ADR#0063](./0063-agent-connection-declarations.md) all describe what the
Agent needs. Nothing describes what it offers.

That gap blocks a decision already made.
[ADR#0012](./0012-ard-compatible-discovery-catalog.md) allows an
[A2A](../glossary/a2a) agent card to be exported as a catalog entry with media
type `application/a2a-agent-card+json`. The
[A2A specification](https://a2a-protocol.org/latest/specification/) requires a
card to list the skills the agent offers, its default input and output modes,
and its capabilities, including streaming, push notifications, and state
transition history. The card's name and description have sources in the Agent
and its configuration. Its skills, modes, and capabilities have no source
anywhere in the schema, so an export would have to invent them.

A second inward gap concerns whose authority a run carries. Some Agents act
only as themselves. Others exist to act for an authenticated human, and a
session that starts without one is a different run, not the same run with less
context. Nothing in the configuration says which kind of Agent it is, so
admission cannot reject a session that starts in the wrong mode.

The policy plane has its own gap. `policy/agent_budgets` caps money per Agent
over a calendar month or a lifetime. An agentic loop can make unbounded model
calls inside one session, and the platform has no ceiling on a single session's
spend and no throughput control at all: no token or request ceiling over a
window and no cap on how many model calls one session may make.
[ADR#0025](./0025-agent-definition-data-ownership.md) Decision 5 already lists
"budget, token, turn, concurrency, and delegation ceilings" among the data that
never enters an AgentConfiguration, and
[ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md) gives
budgets their own control plane. The question is only where the new ceilings
live on that plane.

The [agent platform research index](../research/agent-platform/index.md)
records the products studied for the definition, session, and policy split this
record extends.

## Decision

### 1. `AgentConfiguration.interface` declares what the Agent offers

`AgentConfiguration` gains `AgentInterface interface`. An `AgentInterface`
carries:

- `offered_skills`, each an `OfferedSkill` with a stable `skill_key`, a `name`,
  a `description`, `tags`, `examples`, and optional per-skill input and output
  media type overrides that otherwise inherit the interface defaults;
- `default_input_media_types` and `default_output_media_types`; and
- `InteractionCapabilities`, with `streaming`, `push_notifications`, and
  `state_transition_history`.

The interface passes every statement of the
[ADR#0025](./0025-agent-definition-data-ownership.md) ownership test. It is the
Agent's own declaration of behavior. Every session pinned to the revision
begins with the same offer, and session admission validates inbound and
outbound parts against it. Changing it changes what callers may ask for and
what they get back, which invalidates behavioral verification and requires a
new revision.

It is the protocol-neutral source from which the catalog layer of
[ADR#0012](./0012-ard-compatible-discovery-catalog.md) derives an A2A card.
The interface is not A2A-shaped on purpose, consistent with that record's rule
that the neutral model must not reuse A2A identifiers as its domain.

An `OfferedSkill` is the opposite direction from `SkillDeclaration`. A
`SkillDeclaration` pins content the Agent consumes; an `OfferedSkill`
advertises an ability callers consume. Advertising grants no authority and
resolves no dependency.

An absent interface means the Agent offers no external interface and cannot be
exported as a callable catalog entry.

The interface deliberately omits the remaining card fields, because each
already has an owner:

- endpoints and transports are platform bindings under
  [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md);
- security schemes belong to
  [ADR#0017](./0017-aauth-agent-authentication.md);
- card signatures are produced at export with the Agent's own key under
  [ADR#0036](./0036-agent-self-certifying-identity.md), so no stored signature
  can drift from the card it signs; and
- the provider organization derives from the hierarchy `parent`, which
  [ADR#0025](./0025-agent-definition-data-ownership.md) assigns to the
  hierarchy plane.

### 2. `AgentConfiguration.execution_subject` declares whose authority a session carries

`AgentConfiguration` gains `ExecutionSubject execution_subject`, with the
values `AGENT_ONLY`, `DELEGATED_PRINCIPAL_ONLY`, and
`AGENT_OR_DELEGATED_PRINCIPAL`. Absent means `AGENT_ONLY`.

Whether a run carries a human principal changes what the Agent does and what
verification must cover, so the declaration is configuration-owned. Session
admission enforces it: a session that does not match the declared subject is
rejected, and under `AGENT_OR_DELEGATED_PRINCIPAL` the session records which
mode applied.

The declaration grants no authority. It names no principal and does not say
who may invoke the Agent, which
[ADR#0025](./0025-agent-definition-data-ownership.md) keeps on authentication
and policy. The identity binding itself stays on the identity plane under
[ADR#0036](./0036-agent-self-certifying-identity.md) and
[ADR#0037](./0037-agent-identity-governance.md). The configuration says only
which binding a session must present.

### 3. `AgentBudgetPeriod.SESSION` caps money per Session

`policy/agent_budgets` gains `AGENT_BUDGET_PERIOD_SESSION` beside the calendar
month and lifetime periods. It caps all spending attributable to one Session
from its start through its terminal state. Spending a Session delegates to
descendant Sessions counts against its root. Each Session's counter starts at
zero and is never shared with another Session.

A per-session money ceiling is policy, not configuration: it is a funder's
stance, it changes at the funder's cadence, and changing it must not mint a
revision. It therefore joins the existing budget policy rather than the
revision. Admission evaluates it in the same conjunction as the other periods,
and it neither resets nor replaces their counters.

### 4. `policy/agent_rate_limits` adds throughput ceilings

A new policy module `policy/agent_rate_limits` holds an
`AgentRateLimitPolicy` per Agent. The policy carries:

- `AgentRateLimit` entries, each keyed by a scope (`AGENT` or `SESSION`) and a
  meter (`TOKENS` or `MODEL_REQUESTS`), with a `maximum` and a fixed `window`;
  and
- an `AgentSessionCallLimit`, a cap on total model requests per Session with no
  window.

The module follows the shape of `policy/agent_budgets`: initialize, set, and
clear commands with matching events, and the same role as a provisioning
participant. Counters live in authoritative accounting history and never in
the policy stream, so setting, clearing, or replaying a policy cannot reset
them.

The platform had money ceilings only. Throughput is a separate axis, because a
cheap model can still exhaust an upstream quota or run a loop indefinitely, and
[ADR#0025](./0025-agent-definition-data-ownership.md) and
[ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md) already
place ceilings on the policy plane. A separate module keeps money and
throughput independently settable, since they are often owned by different
people.

### 5. `AgentProfile.documentation_url` is presentation data

The catalog profile gains `documentation_url`, which A2A cards also carry. It
accepts only credential-free absolute HTTPS URLs, under the same admission rule
as `Avatar.https_url`.

It belongs on the profile, not the revision. A documentation link changes
nothing any session does, so it fails the
[ADR#0025](./0025-agent-definition-data-ownership.md) verification test, and
updating it must not require a proposal.

### 6. Rejected or deferred

- **Guardrail bindings.** No guardrail resource exists for a binding to
  reference. A binding without a target is an unverifiable string.
- **A paused but not archived lifecycle state.** Pausing changes the registry
  lifecycle, which is a separate lifecycle decision rather than an interface or
  limit decision.
- **An operational kill-switch webhook.** Stopping a running Agent belongs to a
  runtime or session plane, and an outbound webhook only matters when the
  platform proxies an externally hosted Agent.
- **Per-agent trace-enforcement toggles.** Trace context is already mandatory
  under [ADR#0042](./0042-nats-trace-context-and-message-path-tracing.md). A
  toggle could only weaken a platform guarantee.
- **Pricing fields.** What a caller pays belongs to the settlement plane, not to
  the Agent's offer or to its spending ceilings.

## Consequences

- An Agent without an interface is not exportable as a callable catalog entry.
  Export is a consequence of a declaration, never a default.
- Offered skill keys are unique within an interface and stable across
  revisions, so a caller or catalog can follow one skill through the Agent's
  history.
- A session-scoped budget or rate limit never shares counters between
  Sessions, and a root Session's budget counter includes its descendants.
- Each scope and meter pair appears at most once in an `AgentRateLimitPolicy`.
- An absent budget or rate limit policy removes only that Agent-specific
  ceiling. It is never a zero cap and never bypasses another scope's limit or
  an authorization check.
- Configuration digests minted before this change remain valid, since the new
  fields are additive at fresh tags and absent values serialize to zero bytes.
- The catalog export layer of
  [ADR#0012](./0012-ard-compatible-discovery-catalog.md) must map
  `AgentInterface`, the Agent's name and description, and
  `AgentProfile.documentation_url` to the A2A card shape, and must produce the
  card's endpoints, security, signatures, and provider from their own planes.
- Session admission must enforce `execution_subject` and must validate parts
  against the interface's media types, including per-skill overrides. Until it
  does, both fields are reviewable declarations without enforcement.
- Authoritative accounting must attribute spend, tokens, and model requests by
  Session, including descendant Sessions under their root, before the session
  period or the session scope can admit or reject anything.

## References

- [ADR#0012: ARD-Compatible Discovery Catalog](./0012-ard-compatible-discovery-catalog.md)
- [ADR#0016: Protocol Buffers RPC over NATS micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0036: Agent Self-Certifying Cryptographic Identity](./0036-agent-self-certifying-identity.md)
- [ADR#0037: Agent Identity Governance](./0037-agent-identity-governance.md)
- [ADR#0042: NATS Trace Context and Message Path Tracing](./0042-nats-trace-context-and-message-path-tracing.md)
- [ADR#0062: Runtime-Owned Settings and Platform Declarations](./0062-runtime-owned-settings-and-platform-declarations.md)
- [ADR#0063: An Agent Declares the Connections It Needs](./0063-agent-connection-declarations.md)
- [Agent2Agent (A2A) specification](https://a2a-protocol.org/latest/specification/)
- [Agent platform research index](../research/agent-platform/index.md)
