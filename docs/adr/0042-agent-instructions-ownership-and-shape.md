---
number: "0042"
slug: agent-instructions-ownership-and-shape
status: draft
date: 2026-07-30
---

# ADR#0042: Agent Instructions Ownership and Shape

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
markdown; the ecosystem's structured forms (rule lists) structure only
activation (globs, triggers, always-on flags), never the content; the
unions that exist at SDK boundaries are string-or-preset and
string-or-callable, never string-or-messages; chat-shaped prompt content
appears only in prompt-management products, not in agent definitions; and
instruction content converged into a portable, Linux Foundation governed
standard (AGENTS.md) precisely because content travels across tools while
injection mechanics do not.

Compatibility posture: these contracts are pre-adoption and breaking
changes are acceptable today. This decision therefore optimizes for honest
ownership modeling rather than wire-compatibility insurance. The protobuf
evolution facts are still recorded under Consequences because they
determine which future changes are additive and which are one-way doors.

## Decision

### 1. Instruction content is charter content inside AgentConfiguration

This confirms [ADR#0025](./0025-agent-definition-data-ownership.md):
instructions is the agent's own fact. The ownership test that governs the
registry (is this the agent's own fact, or someone else's stance toward
the agent?) places the agent's authored voice inside its configuration,
where proposals diff it, activation digests commit to it, and the bench
measures it.

Two relocations were considered and rejected.

**Not runtime-owned settings.** The test that moved model selection into
the runtime's settings does not transfer. Model vocabulary is
runtime-owned and optional: a runtime that exposes no model selection
carries none, and no platform field is left empty to interpret. Instruction
content fails both prongs: every surveyed runtime consumes markdown
instruction content, and the content is authored by humans and by the
agent itself, not defined by the runtime. Burying it in the `Any` would
also make an instruction edit indistinguishable from any other settings
change, which destroys the typed difference and derived change class that
[ADR#0025](./0025-agent-definition-data-ownership.md) builds proposals on,
prevents learned-layer classification without per-runtime differs, and
forces an agent proposing an edit to its own instructions to understand
its runtime's settings schema.

**Not outside AgentConfiguration.** Two exteriorizations were examined:

- A selector-bound plane beside the agent, like policy or evaluation.
  Rejected because instruction changes must mint revisions: a
  [session](../glossary/session) pins a revision, and if instructions
  floated on a plane, pinned behavior would drift, the revision digest
  would no longer commit to behavior, and "which behavior ran" would
  become a pair of version axes that every evaluation and comparison must
  carry. Planes hold other parties' stances toward the agent; the agent's
  own voice on a plane inverts that rule.
- A separate registry entity (a shared instructions document) referenced
  from the configuration. Rejected for v1 because a shared mutable
  document couples agents: an edit either becomes an implicit proposal
  against every referencing agent or it silently rewrites their behavior,
  the exact failure mode the variables contract in
  [ADR#0025](./0025-agent-definition-data-ownership.md) forbids. Reuse at
  authoring time is the template system's job, which the
  [decision record](../research/agent-platform/decision-record.md) (Q16)
  already names as the largest undesigned area. The genuine benefits of
  externalized content, size and deduplication, are reachable later as an
  additive content-address variant inside the wrapper without moving
  ownership; see Consequences.

### 2. The shape is a singular wrapper message carrying markdown text

```proto
message AgentConfiguration {
  string runtime = 1 [features.field_presence = LEGACY_REQUIRED];
  google.protobuf.Any settings = 2;
  Instructions instructions = 3;
}

// Instructions is the charter instruction content: one authored markdown
// document (design Q17: the layers that are not this document live on
// their own surfaces). A message rather than a bare string so growth is
// additive; string-to-message on one field number is the one silently
// misparsing proto migration, so the container is chosen once, here.
message Instructions {
  string text = 1;
}
```

- `text` is markdown, the payload shape of every surveyed harness. Absent
  `instructions` means the runtime runs its default prompt, mirroring the
  absent-settings rule.
- **No `oneof`.** The corpus contains no alternative representation of
  agent instruction content to model. The unions that exist are injection
  mechanics (string or preset-plus-append) or host-language escape hatches
  (string or callable), and role-tagged message lists appear only in
  prompt management. If an alternative representation ever becomes real,
  moving the single `text` field into a new `oneof` is a wire-safe,
  digest-stable change.
- **No `repeated`.** The ecosystem's rule lists structure activation, not
  content, and this architecture already owns those activation homes:
  description-triggered content is skills, cadence-bound injection is turn
  wrappers, generated snippets are the memory plane. A top-level repeated
  field also can never gain list-level metadata without a second field,
  while a singular message can grow a list inside it if one is ever
  justified.
- **Wrapper over bare string.** With breaking changes acceptable the
  difference is modest, but every anticipated growth (content metadata, a
  block list, a content-address variant) lands as an additive field inside
  the wrapper, and the one migration protobuf cannot survive on a fixed
  field number is bare string to message. The container decision is a
  one-way door, so it is taken now; everything else is deferred.

### 3. Injection mechanics are runtime-owned settings

How content reaches the model is runtime vocabulary and lives in the
runtime's typed settings message: preset or base-prompt choice, append
versus full replacement, dynamic-section exclusion, built-in block
toggles. The runtime adapter composes the two surfaces: platform
`instructions.text` is what the agent says; runtime settings decide how
the harness ingests it (a system-prompt append, a context file, a config
key). Where the assembled context places the content is the assembly
specification's concern (decision record Q17), not a charter field.

## Invariants

- The revision digest commits to instruction content: today by value,
  and through a content digest if an address variant is ever added.
- Instruction changes are learned-layer changes: they arrive by proposal,
  mint revisions, and never mutate an activated configuration.
- The platform diffs instructions without decoding any runtime settings
  type.
- Runtime settings never carry instruction content; `Instructions` never
  carries injection mechanics.
- Role-tagged conversation content is out of charter scope; conversation
  positions belong to turn wrappers and the memory plane.

## Consequences

- `AgentConfiguration` gains `Instructions instructions = 3` when charter
  content is implemented; `AgentProvisioned` carries it transitively
  inside the configuration it already embeds.
- Proposals read a typed instructions difference directly; change-class
  derivation needs no runtime knowledge.
- Each runtime's settings message defines its injection knobs in its
  native vocabulary, and an empty settings payload keeps meaning "runtime
  defaults" for mechanics exactly as it does today.
- Recorded evolution facts for when compatibility starts to matter:
  adding fields to `Instructions` is unconditionally safe; moving the
  single `text` field into a new `oneof` is wire-safe and byte-stable for
  existing values; singular-to-repeated is wire-compatible for string,
  bytes, and message fields; bare string to message on one field number
  is not wire-safe, which is why the wrapper exists from the start.
- Anticipated growth, all additive inside `Instructions`, none scheduled:
  a `repeated` block list with activation metadata only if skills, turn
  wrappers, and the memory plane prove insufficient; a content-address
  variant (`ref` plus a content `Digest`) if content outgrows event
  transport budgets. Observed ecosystem budgets top out at 32 KiB
  for concatenated project docs and 256,000 characters for a single
  instruction string, well inside the platform's event payload ceiling
  today.
- The research behind this decision is frozen in the
  [agent instructions corpus](../research/agent-instructions/index.md);
  where later findings differ, this record is amended rather than the
  corpus rewritten.

## References

- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [Agent instructions research corpus](../research/agent-instructions/index.md)
- [Harness survey](../research/agent-instructions/harness-survey.md)
- [Prompt management shapes](../research/agent-instructions/prompt-management.md)
- [Agent platform decision record](../research/agent-platform/decision-record.md) (Q15, Q16, Q17, Q24)
- [AGENTS.md standard](https://agents.md)
