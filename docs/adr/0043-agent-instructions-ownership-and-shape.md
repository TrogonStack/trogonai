---
number: "0043"
slug: agent-instructions-ownership-and-shape
status: draft
date: 2026-07-30
---

# ADR#0043: Agent Instructions Ownership and Shape

## Context

[ADR#0025](./0025-agent-definition-data-ownership.md) assigns `instructions`
to [AgentConfiguration](../glossary/agentconfiguration) in its conceptual
model and classifies instruction edits as learned-layer changes, but the
shipped `AgentConfiguration` message carries only `runtime` and the
runtime-owned `settings`. No decision yet fixes where instruction content
lives on the wire or what shape it takes. Three questions are open:

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

The deciding evidence is the shape of the runtime contract this platform
integrates first. The Claude Agent SDK exports, per the `Options` typing
shipped in `@anthropic-ai/claude-agent-sdk` 0.3.220 (the published
reference page still documents only the string and preset forms):

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
payload, in the runtime's native shape, exactly as model selection
already does. A Claude-runtime settings message mirrors the
`systemPrompt` union verbatim; a runtime with a different prompt contract
mirrors its own. The settings rule extends unchanged: the runtime named
on the configuration defines the message type carried in `settings` and
validates its contents, and absent means the runtime runs with its
defaults, including its default prompt.

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

No `Instructions` wrapper, no `oneof`, no repeated block list, and no
platform field left empty to interpret. What would force a revisit is a
platform feature that must read instructions across runtimes: typed
proposal differences and change classes over instruction edits, bench
attribution keyed to instruction content, or cross-runtime instruction
governance. When one of those becomes real work, a successor ADR decides
the abstraction from the corpus evidence instead of ahead of it.

### 3. The conflict with [ADR#0025](./0025-agent-definition-data-ownership.md) is recorded, not resolved

[ADR#0025](./0025-agent-definition-data-ownership.md)'s conceptual model
places instructions in AgentConfiguration and classifies instruction
edits as learned-layer changes. The shipped contract now diverges for
instructions exactly as it does for model selection, and the same
consequence applies: any step that reads instructions as typed platform
content has an unmet precondition until a later decision supplies one.

Considered and rejected for now, the platform-owned wrapper
(`Instructions { string text = 1 }`, a previous draft of this ADR that
briefly existed on this branch and never merged):

- It privileges one projection, a single markdown document, of contracts
  that natively accept lists, presets, and files.
- Its platform benefits (typed instruction diffs, learned-layer
  classification without decoding runtime types, runtime-blind
  self-proposals) purchase machinery no shipped feature consumes yet.
- With breaking changes still acceptable, deferring is cheap, while
  un-shipping a platform abstraction after adoption is not.

Also rejected, unchanged from the earlier analysis: exteriorizing
instructions to a selector-bound plane (instruction changes must mint
revisions; a [session](../glossary/session) pins a revision and behavior
must not float behind it) or to a shared instructions entity (an edit to
a shared document either becomes an implicit proposal against every
referencing agent or silently rewrites their behavior, the failure mode
the variables contract in
[ADR#0025](./0025-agent-definition-data-ownership.md) forbids).

## Invariants

- `settings` is the single home for instruction content and injection
  mechanics alike; `AgentConfiguration` carries no instruction field.
- Revision digests commit to instruction content transitively through
  the settings bytes; pinning and reproducibility are unaffected.
- Instruction changes mint revisions like any configuration change; what
  the platform cannot yet do is classify or diff them without decoding
  the runtime's settings type, and that limitation is accepted, not
  accidental.
- Conversation-shaped content stays out of the agent record; turn
  wrappers and the memory plane own those positions when they land.

## Consequences

- `AgentConfiguration` stays two fields. The `settings` comment in
  `agent.proto` now names instruction and prompt content as runtime-owned
  explicitly, so the next reader does not re-open the question by
  omission.
- Each runtime settings message models its native prompt contract
  verbatim (mirror the union, do not flatten it into one string).
- Proposal machinery treats an instruction edit as an opaque settings
  change for now; typed instruction differences, learned-layer
  classification, and bench attribution over instruction content are
  future work gated on a successor decision, tracked the same way
  [ADR#0025](./0025-agent-definition-data-ownership.md) tracks the
  model-selection conflict.
- The research corpus stays frozen as the evidence base for that
  revisit, and the protobuf evolution facts recorded with it apply on
  the day a platform field is ever introduced: adding fields is safe on
  the binary wire format, though ProtoJSON parsers reject unknown fields
  unless configured to discard them (relevant here because the
  generated bindings carry ProtoJSON support and
  [ADR#0041](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md) puts JSON
  bodies on the wire); moving one existing explicit-presence field into
  a newly created `oneof` is wire-safe, while moving fields into an
  existing `oneof` is not; and bare string to message on one field
  number is never safe.

## References

- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [Agent instructions research corpus](../research/agent-instructions/index.md)
- [Harness survey](../research/agent-instructions/harness-survey.md)
- [Prompt management shapes](../research/agent-instructions/prompt-management.md)
- [Agent platform decision record](../research/agent-platform/decision-record.md) (Q15, Q16, Q17, Q24)
- [Claude Agent SDK TypeScript reference](https://code.claude.com/docs/en/agent-sdk/typescript)
