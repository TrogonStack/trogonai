---
number: "0062"
slug: runtime-owned-settings-and-platform-declarations
status: draft
date: 2026-09-07
---

# ADR#0062: Runtime-Owned Settings and Platform Declarations

## Context

A general Agent definition must support implementations with different input
contracts. A Claude runtime adapter and a Codex runtime adapter need not expose
the same instructions, model controls, tool options, or configuration structure.
Making every option a required field on AgentConfiguration would restrict the
platform to implementations that happen to fit that shared shape.

The original `runtime` and `settings` boundary preserves those differences:
the runtime identifies the interpreter, and the settings payload carries that
interpreter's typed configuration. Its weakness is an insufficiently pinned
relationship between the two, not the absence of universal prompt or model
fields.

TrogonAI also owns resources whose meaning is independent of a native runtime's
option vocabulary. Skill versions, memory resources, and their lifecycle and
authorization rules should not acquire a different owner for each adapter.
Their platform declarations need an explicit boundary alongside native settings.

This decision reconciles the general definition in
[ADR#0025](./0025-agent-definition-data-ownership.md), the execution model in
[ADR#0031](./0031-agent-implementation-and-session-plan.md), the managed model
capability in [ADR#0032](./0032-model-route-and-credential-binding.md), and native
instruction ownership in
[ADR#0043](./0043-agent-instructions-ownership-and-shape.md). Their draft details
must follow this ownership boundary before contract rollout. It does not change
the accepted registry and proposal topology in
[ADR#0024](./0024-agent-platform-stream-topology.md).

## Decision

### 1. Each runtime owns one typed settings contract

AgentConfiguration binds one exact runtime implementation to one
`google.protobuf.Any` settings payload. The selected runtime implementation or
adapter defines the concrete protobuf message that payload carries, including
its supported fields, defaults, and validation rules. The platform validates
the runtime/settings pairing rather than interpreting every runtime's fields as
one common configuration language.

The runtime binding commits to an immutable implementation release and its
definition digest. That definition pins the accepted settings message type,
contract version, descriptor closure, and validation and interpretation
semantics. A familiar type name alone is insufficient. A mismatched payload,
unsupported contract version, or unknown behavior rejects admission. Type URLs
identify registered types; they are not instructions to fetch code or schemas.

The platform's own harness follows the same rule: it owns a concrete typed
settings message. Carrying that message in the common Any envelope does not
replace its schema with a map or permit arbitrary payloads. A registered
runtime's different settings type does not require another built-in arm on the
general Agent configuration.

The common payload is singular. Different runtimes need different accepted
types, which Any already supports. Repeated sections belong inside a runtime's
message when that runtime defines their cardinality, ordering, and interaction.
A shared `repeated Any` would introduce composition semantics for duplicates,
conflicts, and required components; that needs a separate concrete use case and
decision.

### 2. Keep platform-owned declarations typed

Runtime-owned settings do not absorb every datum associated with an Agent.
Platform declarations describe resources and interfaces whose meaning TrogonAI
owns. Native settings describe how the selected implementation behaves.

| Concern | Owner and revision boundary |
| --- | --- |
| Native instructions, prompt injection, model options, and runtime-specific configuration | The runtime's settings contract; a behavior change requires a new AgentRevision. |
| Platform skill versions | The skill resource owns content and lifecycle; AgentConfiguration owns exact skill pins. Publishing another skill version does not update an existing pin. |
| Platform memory | Memory owns content, policy, and lifecycle; AgentConfiguration declares dependencies by kind, while Session admission resolves instances under live hierarchy and policy. Memory writes do not mint Agent revisions. |
| Platform tools and delegates | Their resources own definitions; AgentConfiguration declares selectors or exact pins that Session admission resolves. A declaration grants no authority. |
| Platform caller variables | AgentConfiguration owns the declared caller interface where the platform supplies it; concrete bindings belong to the Session. Native-only variables remain in native settings. |
| Selectable labels and description | Revision-owned declarations; labels may affect selection and must not be writable through annotations. |
| Identity and annotations | Agent registry facts; annotations remain nonbehavioral metadata. |
| Grants, credentials, budgets, schedules, and routing policy | Their own control planes, evaluated independently of the Agent revision. |

These platform concerns may have typed fields in the common configuration.
This decision fixes ownership, not the final grouping or field numbers. Native
settings cannot contain a second authoritative copy of a platform declaration.
A runtime may have its own native skill or memory mechanism; that does not make
it the manager of a TrogonAI skill or memory resource.

### 3. Declared integrations require runtime support

Managing a platform resource and knowing how a runtime consumes it are separate
responsibilities. A runtime or its pinned adapter must support the resolved form
of every platform integration declared by that Agent. An Agent with no such
declarations does not acquire that capability requirement merely by existing.

For example, a platform skill pin identifies immutable content. The adapter
contract determines how that content reaches the runtime without changing its
meaning or silently substituting another version. A memory declaration selects
an authorized resource kind; it does not decide that every runtime receives
memory as prompt text, a file, or a tool.

Admission rejects unsupported integrations instead of ignoring configured
behavior. Required dependencies must resolve. Optional dependencies may be
absent according to their declared semantics, but optionality does not permit
ignoring an unsupported configuration contract. The Session records what was
actually resolved and supplied.

### 4. Platform inspection is a capability, not a second configuration

Some platform features need information from native settings. The supported,
pinned runtime adapter must provide and validate that information for the
feature that consumes it. Derived representations do not become independently
authored AgentConfiguration fields.

Managed model access is one such capability. An adapter using
[ADR#0032](./0032-model-route-and-credential-binding.md) must expose verifiable
exact primary and auxiliary model selections from the pinned native settings
and enforce the admitted route without substitution. The Session records the
derived selections and routes; the runtime-owned settings remain the source of
the declared behavior.

A runtime that manages models internally without exposing those guarantees is
not model-free. Its settings can be represented by the general Agent schema,
but it cannot be admitted to an execution mode that requires those guarantees.
Representability alone grants neither execution permission nor model access.

Proposal governance follows the same boundary. Platform declarations have
platform-defined differences. Typed differences within native settings require
a supported classifier bound to the exact runtime/settings contract. A fully
validated settings change without that classification is charter-class as a
whole; it cannot claim to be a learned-layer instruction-only edit. Failure to
decode or validate the settings rejects the candidate entirely.

### 5. Revisions bind settings and declarations together

The configuration digest commits to the exact runtime binding, its native
settings, and all revision-owned platform declarations. The reviewed candidate
remains the same immutable artifact at activation. Neither a new runtime
release nor a new native settings contract silently changes an existing
revision.

Native defaults may remain part of the runtime contract when the pinned release
fixes their meaning. Defaults that depend on mutable external state do not
establish an exact behavioral pin. Admission must resolve and record the facts
needed by its execution guarantees, or reject that execution mode when the
runtime cannot provide them.

## Consequences

- The platform can add runtime adapters without treating their option names as
  universal Agent fields or inventing unsupported native behavior.
- Skill and memory management remain coherent platform domains. Adapter
  support determines how their declared resources participate in execution.
- Model mediation and typed native governance need explicit adapter contracts.
  The general Agent schema alone does not establish those capabilities.
- Implementation, configuration, and Session contracts must reject unsupported
  combinations before execution. A generic envelope is not permission to
  forward arbitrary settings or ignore unknown behavior.
- Protobuf contracts and runtime implementations must follow this boundary;
  recording the decision does not imply that adapters or runtime enforcement
  already exist.

## References

- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0043: Agent Instructions Ownership and Shape](./0043-agent-instructions-ownership-and-shape.md)
