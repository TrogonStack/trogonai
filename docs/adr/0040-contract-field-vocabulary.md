---
number: "0040"
slug: contract-field-vocabulary
status: draft
date: 2026-07-27
---

# ADR#0040: Contract Field Vocabulary: Identifiers, Handles, and Display Labels

## Context

One concept keeps recurring across domains: a human-facing label with no
enforced semantics. Today it is spelled three different ways.

The agents contract calls it `name`. `AgentProvisioned.name` carries it, and
the retired value object behind it validated only that the string was
non-blank: no syntax rule, no uniqueness rule. The one place uniqueness was
even mentioned was an assumed upstream precondition noted in a since-deleted
command comment, never a domain invariant, and a per-stream decider cannot
enforce cross-stream uniqueness anyway.

The sessions contract, the decider aggregate
[ADR#0035](./0035-session-store-decider-aggregate.md) establishes, calls the
same kind of concept `title`. `SessionRenamed.title` is documented as "the
session's display title": the same mutable label, spelled differently
because it lives in a different domain.

[ADR#0025](./0025-agent-definition-data-ownership.md)'s example data spells
it a third way again, `display_name`, but as a key inside the `annotations`
bag rather than as a field of its own: a stringly-typed convention that
carries the right name but the wrong structure.

Meanwhile `name` also appears where it is legitimate. `ToolUseBlock.name`
and `ToolCallRequested.name` are tool handles: keys the tool catalog
actually resolves to a tool definition. Those two fields are not the
ambiguity; they are why the ambiguity is dangerous. A reader cannot tell,
from the word `name` alone, whether a given occurrence resolves to
anything.

The platform's recorded stance is explicit over implicit. The genesis
`AgentProvisioned.revision` field is recorded explicitly, "so no consumer
derives it from convention," for exactly this reason: nothing is left to a
reader's inference when a contract can say it directly. A field called
`name` that nothing resolves makes an implicit promise that nothing keeps.
Left unpinned, each new domain re-invents the choice on its own, and the
drift compounds: three spellings today become four, five, and six as the
next domain faces the same field and guesses again.

Everything this decision touches is greenfield. Nothing described here is
deployed. A breaking rename costs once, now; a permanent vocabulary
exception costs every future reader, forever.

## Decision

### 1. The two-question test

Apply this test to any string field on a platform-owned record whose
purpose is to name or refer to something. Opaque and derived data,
digests, serialized payloads, and free-form content (`content_digest`,
`input_json`) name nothing; they are outside the test's scope and keep
their descriptive spellings.

- **Q1: Does any mechanism resolve this value?** If nothing looks the
  value up, joins on it, or treats it as a key into a catalog, the value is
  a display label. Spell it `display_name`. Stop here.
- **Q2 (only if Q1 is yes): Does the value denote the same referent
  forever, or whatever is currently bound to it?**
  - Rigid, opaque, and minted once at creation: the value is an
    identifier. Spell it `id`, nested inside its own record, or
    `<record>_id` when referenced flatly.
  - Legible, registry-owned, late-bound, and rebindable to a different
    referent over time: the value is a handle. Spell it `name`.

### 2. Spelling rules

The test above compiles to a small set of spelling rules:

- A record's own identity is `id`, nested inside that record, or
  `<record>_id` when referenced flatly. It never changes after creation.
- A reference to another entity by identity is `<referent>_id`.
  `parent_tool_use_id` and `tool_execution_id` already comply.
- Placement keeps its pre-existing spelling rule and is a recorded
  exception to the bullet above: bare `parent` means a placement node ref,
  and kinship is always `parent<Type>Id`, per the
  [agent-platform decision record](../research/agent-platform/decision-record.md)
  and the comment quoted on the field itself. A reader applying
  `<referent>_id` mechanically to `parent` would be undoing a deliberate
  convention, not fixing an oversight.
- A reference to another entity by handle is `<referent>_name`.
- A record's own display label is `display_name`. It is always an
  explicit field, never a key inside an `annotations` map or any other
  opaque bag. This revises the illustrative placement in
  [ADR#0025](./0025-agent-definition-data-ownership.md)'s example data,
  which put `display_name` inside `annotations`; that placement predates
  this decision and is superseded by it, consistent with the
  explicit-over-implicit stance recorded above.

### 3. Bare `name` is reserved

Bare `name` is not available for a display label under any circumstance.
Only a record that is itself the named entry in a registry that resolves
and enforces the handle may carry bare `name`. A label that nothing
resolves may not be called `name`, no matter how convenient the word feels
in the moment.

### 4. Wire-fidelity exemption

A message whose entire purpose is to mirror an external protocol verbatim
keeps that protocol's vocabulary, even where it would otherwise fail the
test above. `ToolUseBlock` mirrors the provider's emitted tool-use block
exactly, `{id, name, input}`, because faithful provider replay requires
reproducing what the provider actually sent, not a platform-preferred
respelling of it. `ToolUseBlock.name` therefore keeps `name`. The same
exemption covers any future record whose job is to mirror a provider or
external protocol rather than to express the platform's own contract.

### 5. Authorized greenfield renames

Applying this decision authorizes the following renames:

- `AgentProvisioned.name` becomes `AgentProvisioned.display_name`. Nothing
  resolves it; it is a label, not a handle.
- `SessionRenamed.title` becomes `SessionRenamed.display_name`. It is the
  same concept as the field above, under its third spelling; the field is
  `v1alpha1` and undeployed, so the rename costs nothing beyond this pass.
- `ToolCallRequested.name` becomes `ToolCallRequested.tool_name`. It is a
  platform-owned execution record referencing a tool by handle, so it
  takes the `<referent>_name` spelling from rule 2.

Tool invocation stays addressed by handle deliberately. `tool_name` must
not become an id: late binding to whatever the catalog currently resolves
for that name is the intended semantics, not an accident this ADR should
close.

### 6. Addressable agent names are deliberately absent

This decision does not give an agent a resolvable handle. Making the
agent's label a real, resolved handle would require a reservation
mechanism, and a per-stream decider cannot provide uniqueness across
streams on its own; that is its own future decision with its own
machinery, not a side effect of a vocabulary fix. This ADR names that
trigger and leaves it unpulled. Agent identity remains `agent_id`,
self-certifying per [ADR#0036](./0036-agent-self-certifying-identity.md).

## Consequences

- One greppable spelling exists per concept across every future domain.
  Choosing a field name becomes a decision-tree lookup, not a taste call.
- The word `name` regains teeth. Seeing it on a platform-owned record now
  means a registry resolves it; it is no longer a coin flip between handle
  and label.
- The rename cost above is paid exactly once, before deployment. The
  alternative was carrying a permanent vocabulary exception in every
  future reader of `AgentProvisioned`, `SessionRenamed`, and
  `ToolCallRequested`.
- Wire-fidelity records such as `ToolUseBlock` are a recorded, deliberate
  boundary rather than an unexplained inconsistency; a future reader can
  tell the two apart on sight.

## References

- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [ADR#0036: Agent Self-Certifying Cryptographic Identity](./0036-agent-self-certifying-identity.md)
- [AIP-122: Resource names](https://google.aip.dev/122)
- [AIP-148: Standard fields](https://google.aip.dev/148)
- [ADR index](./index.md)
