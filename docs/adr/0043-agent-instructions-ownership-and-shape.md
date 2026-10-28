---
number: "0043"
slug: agent-instructions-ownership-and-shape
status: draft
date: 2026-07-30
---

# ADR#0043: Agent Instructions Ownership and Shape

## Context

Earlier drafts of
[ADR#0025](./0025-agent-definition-data-ownership.md) exposed `instructions`
as a platform field on [AgentConfiguration](../glossary/agentconfiguration),
while the original wire contract used `runtime` and runtime-owned `settings`.
That disagreement raised three questions:

1. Ownership: does instruction content belong to the platform contract, to
   the runtime-owned settings payload, or to a resource outside the
   configuration entirely?
2. Shape: is the field a bare string, a tagged union (`oneof`), a list of
   structured blocks, or a wrapper message?
3. Mechanics: how do runtime-specific injection concerns (preset selection,
   append versus replace, prompt-section toggles) relate to the content?

The [agent instructions research corpus](../research/agent-instructions/index.md)
was gathered for this decision. Its findings, in brief: every surveyed
harness accepts standing instructions whose payload is unstructured
markdown text; the ecosystem's structured forms (rule lists) structure
activation, never content; chat-shaped prompt content appears only in
prompt-management products, not in agent definitions; and instruction
content converged into a portable standard (AGENTS.md) while injection
mechanics stayed per-tool.

One concrete example in the research corpus is the Claude Agent SDK's
`Options` typing shipped in `@anthropic-ai/claude-agent-sdk` 0.3.220. It exports:

```typescript
systemPrompt?: string | string[] | {
  type: 'preset';
  preset: 'claude_code';
  append?: string;
  excludeDynamicSections?: boolean;
};
```

Even the content half of that contract is runtime vocabulary: a plain
string, a list of prompt segments (the typing's own example threads a
dynamic cache boundary between them), or a named preset with an append
seam and a cache-shaping toggle. Codex's equivalent surface is a config file plus
concatenated, budgeted context documents; Gemini's is a replacement file
with template variables. There is no single content shape for a platform
field to mirror without loss.

Compatibility posture: these contracts are pre-adoption and breaking
changes are acceptable today, so this decision optimizes for honest
ownership modeling, and for not freezing an abstraction ahead of the
features that would consume it.

## Decision

### 1. Instruction content is runtime-owned inside settings; no platform instruction field ships

The platform declines to define a generic instruction abstraction now.
Instruction and prompt content live inside the runtime-owned `settings`
payload, in the runtime's native shape. Model options follow the same ownership
under [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md).
A Claude-runtime settings message preserves the
`systemPrompt` union verbatim; a runtime with a different prompt contract
mirrors its own. The settings rule extends unchanged: the runtime named
on the configuration defines the message type carried in `settings` and
validates its contents. Its exact implementation release and settings contract
must be pinned. Native defaults are valid only under the guarantees described in
[ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md); an
omitted value is not permission to acquire changing behavior from the host.

The reasoning that moved model selection into settings turns out to
apply after all, once the corpus evidence is read at the contract level
rather than the payload level. What a prompt IS to a runtime (one
string, a document list, a preset plus append) is a fact the runtime
owns. A platform-level markdown field could feed the plain-string form
of such contracts, but it can express only that one form: the segment
lists, preset selection, and append seams above have no platform-side
representation, so every adapter would either flatten to the weakest
shape or invent semantics (position, join rules, preset interaction)
that the platform never actually decided. Either way the platform field
is a lossy projection of the contract the runtime defines.

### 2. The generic abstraction is deferred, not designed

The general Agent configuration has no shared `Instructions` wrapper or common
prompt block list. Each runtime can define those structures inside its own
settings message when they are part of its native contract.

Platform features such as typed proposal differences, bench attribution, or
instruction governance may inspect native settings through a supported pinned
adapter. [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md)
defines that capability boundary. Inspection does not require a second,
independently authored instruction field on the Agent configuration.

### 3. Preserve native instructions alongside platform declarations

[ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md) reconciles
the ownership boundary in
[ADR#0025](./0025-agent-definition-data-ownership.md). Platform-managed skill
pins and memory dependencies can have typed declarations in AgentConfiguration;
instruction content and injection mechanics remain in the selected runtime's
settings. A platform resource declaration does not prescribe how every runtime
inserts that resource into its prompt or context.

A shared platform-owned wrapper, `Instructions { string text = 1 }`, remains
rejected:

- It privileges one projection, a single markdown document, of contracts
  that natively accept lists, presets, and files.
- Its platform benefits (typed instruction diffs, learned-layer
  classification without decoding runtime types, runtime-blind
  self-proposals) purchase machinery no shipped feature consumes yet.
- With breaking changes still acceptable, deferring is cheap, while
  un-shipping a platform abstraction after adoption is not.

Also rejected: moving native instructions to a selector-bound plane
(instruction changes must mint
revisions; a [session](../glossary/session) pins a revision and behavior
must not float behind it) or to a shared mutable instructions entity (an edit to
a shared document either becomes an implicit proposal against every
referencing agent or silently rewrites their behavior, the failure mode
the variables contract in
[ADR#0025](./0025-agent-definition-data-ownership.md) forbids).
Platform skill resources have a different boundary: an exact version pin keeps
existing revisions unchanged when another skill version is published.

## Invariants

- `settings` owns native instruction configuration and injection mechanics;
  `AgentConfiguration` carries no universal instruction field. Platform skill
  content remains in its separately versioned resource.
- Revision digests commit to native instruction settings and any exact platform
  skill pins, preserving the declared revision boundary.
- Instruction changes mint revisions like any configuration change. A typed
  instruction-only classification requires a supported classifier for the exact
  native settings contract; otherwise a validated settings change is
  charter-class as a whole.
- Conversation records belong to the Session. Native turn wrappers belong in
  runtime settings, while memory content keeps its own resource lifecycle.

## Consequences

- The general behavior envelope retains `runtime` and one typed `settings`
  payload. Typed declarations for platform-owned resources may accompany them;
  this decision does not limit AgentConfiguration to exactly two wire fields.
- Each runtime settings message models its native prompt contract
  verbatim (mirror the union, do not flatten it into one string).
- Typed native differences and bench attribution require runtime-specific
  inspection support under
  [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md).
  Unsupported decoding rejects a candidate; it cannot be excused by assigning
  a conservative change class.
- The research corpus remains the historical evidence base. Settings contract
  evolution must preserve exact type/version binding and reject unsupported
  behavior even when a protobuf field addition is wire-compatible.

## References

- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ADR#0062: Runtime-Owned Settings and Platform Declarations](./0062-runtime-owned-settings-and-platform-declarations.md)
- [Agent instructions research corpus](../research/agent-instructions/index.md)
- [Harness survey](../research/agent-instructions/harness-survey.md)
- [Prompt management shapes](../research/agent-instructions/prompt-management.md)
- [Agent platform decision record](../research/agent-platform/decision-record.md) (Q15, Q16, Q17, Q24)
- [Claude Agent SDK TypeScript reference](https://code.claude.com/docs/en/agent-sdk/typescript)
