---
number: "0067"
slug: session-checkout-not-workspace
status: draft
date: 2026-10-10
---

# ADR#0067: A Session Binds to a Checkout, Not a Workspace

## Context

`proto/trogonai/session/sessions/v1alpha1/workspace.proto` defined
`WorkspaceRef`, the fact a session binds to at `SessionStarted`: a stable id,
a resolved location, and the revision the location was at. Fields carrying it
were named `workspace_id` and `workspace` across roughly twenty session and
workflow protos, and the comments described it as "the workspace."

[Organization, workspace, folder, and project](https://github.com/straw-hat-team/adr/blob/main/src/adrs/1917971986/README.md)
and [ADR#0066](./0066-project-resource-and-members-only-access.md) now fix a
second, unrelated meaning for the same word: Workspace is the product
hierarchy level directly below the tenant root, the level a Project attaches
at or below. That decision is accepted and the platform's hierarchy, policy,
and project surfaces are built on it.

The two meanings cannot coexist under one name. A session's `WorkspaceRef`
names a code checkout, something with a URI and a revision that a session
reads and writes files against; the hierarchy's Workspace names a node that
owns folders, projects, and visibility. A reader who sees `workspace_id` on
`SessionStarted` has no way to tell which one is meant, and the two will only
keep drifting further apart: a session's checkout has no folders, no
visibility preset, and no place in the tenant tree, and the hierarchy's
Workspace has no URI or revision.

This is a third, pre-existing sense, distinct from both of the senses already
on record:

- The monorepo sense in [ADR#0001](./0001-workspace-runtime-taxonomy.md) and
  `docs/glossary/workspace.md`: this repository itself, as a Cargo and
  polyglot workspace. Unaffected by this ADR; `rsworkspace/` keeps its name.
- The hierarchy sense in [ADR#0066](./0066-project-resource-and-members-only-access.md)
  and the external ADR above: a tenant's direct child node. Unaffected by
  this ADR; it is the meaning this ADR clears room for.
- The session sense this ADR renames: the code checkout a session is bound
  to.

`proto/trogonai/session` and `proto/trogonai/workflows` are both excluded
from Rust codegen in `buf.gen.yaml`, so no generated code and no compiled
caller exist yet for any of these fields. This is the cheapest the rename
will ever be.

## Decision

The session sense is renamed from workspace to checkout, everywhere it
appears in `proto/trogonai/session` and `proto/trogonai/workflows`:

- `workspace.proto` is renamed to `checkout.proto`.
- `WorkspaceRef` is renamed to `CheckoutRef`, keeping its three fields
  (`checkout_id`, `uri`, `revision`) and their field numbers.
- Fields of type `CheckoutRef` are renamed from `workspace` to `checkout_ref`
  (`CreateSession`, `ForkSession`, `RecoverSession`, `CreateChildSession`,
  `SessionStarted`, `RunConfiguration`), field numbers unchanged.
- Fields that carry a checkout's id as a plain string are renamed from
  `workspace_id` to `checkout_id` (list and salvage requests, cursors, and
  summary views across the sessions and workflow-runs query packages), field
  numbers unchanged.
- Comments and CEL validation messages referring to "the workspace" in this
  sense are reworded to "the checkout."

Nothing about the hierarchy Workspace, the monorepo workspace, or
`rsworkspace/` changes. "Workspace" remains the correct word for both of
those; it is only retired from the session's vocabulary.

This breaks source and JSON compatibility for `v1alpha1` session and workflow
messages (renamed message, field names, and JSON names). Field numbers and
wire types are unchanged, so binary payloads still decode. It ships as a
rename rather than field-number-additive deprecation because no generated
code or caller exists yet to break, and carrying the collision forward into
code generation would be more expensive to unwind than renaming now.

## Consequences

- A `workspace_id` or `WorkspaceRef` seen anywhere in `proto/trogonai/session`
  or `proto/trogonai/workflows` after this ADR is a bug, not an alternate
  spelling.
- `buf breaking` against `main` reports the rename (message removed, fields
  removed and added) for every touched file. This is expected and accepted
  for these two packages while they remain excluded from codegen; no
  compiled caller observes the break.
- Research and comparison documents that quote these proto names or fields
  are updated to match; documents describing another product's own
  `workspace_id` or `workspace`-named field keep that product's real name
  unchanged.
- Future contract-impact review for `proto/trogonai/session` and
  `proto/trogonai/workflows` can use "workspace" unambiguously for the
  hierarchy sense without checking which meaning a given mention is.

## Alternatives Considered

### Keep `WorkspaceRef` and rename the hierarchy level instead

Rejected. The hierarchy name is set by an external, already-accepted
cross-repository decision and by [ADR#0066](./0066-project-resource-and-members-only-access.md),
which other work already builds on. Renaming it would ripple through the
hierarchy, policy, and project
surfaces for a term that is correct on its own; the session's checkout is the
newer, smaller, still-uncompiled surface and the one actually suffering the
collision.

### Qualify instead of rename, e.g. `SessionWorkspaceRef`

Rejected. A qualifier narrows the collision without removing it: a reader
still has to learn that "session workspace" and "workspace" name unrelated
things, and every new session field would need the same qualifier to stay
safe. "Checkout" states what the field actually is, a code checkout, without
needing the adjacent concept's name to disambiguate it.

### Leave it until codegen is enabled for these packages

Rejected. The absence of generated code and compiled callers today is the
reason this rename is cheap. Waiting until `proto/trogonai/session` and
`proto/trogonai/workflows` are removed from `buf.gen.yaml`'s exclude list
would convert this into a change with real callers to migrate, for no
benefit to any decision made in the meantime.

## References

- [Organization, workspace, folder, and project](https://github.com/straw-hat-team/adr/blob/main/src/adrs/1917971986/README.md)
- [ADR#0001: Workspace Runtime Taxonomy](./0001-workspace-runtime-taxonomy.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ADR#0035: Session Store Decider Aggregate](./0035-session-store-decider-aggregate.md)
- [ADR#0066: A Project Is Work Attached Below a Workspace, and Its Initial Access Is Recorded at Creation](./0066-project-resource-and-members-only-access.md)
