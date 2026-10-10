---
number: "0066"
slug: project-resource-and-members-only-access
status: accepted
date: 2026-10-10
---

# ADR#0066: A Project Is Work Attached Below a Workspace, and Its Initial Access Is Recorded at Creation

## Context

People and agents need a place to organize a body of work: something with a
name, a stated goal, and an audience. The product surface that motivates this
asks three questions when a project is created: what are you working on, what
are you trying to achieve, and who can see it, either everyone in the
workspace or only invited members.

Nothing in the contract models this today. The hierarchy decisions this
platform builds on fix most of the vocabulary already:

- [Resource hierarchy via untyped recursive nodes](https://github.com/straw-hat-team/adr/blob/main/src/adrs/4761776210/README.md)
  gives each tenant one tree of untyped nodes. Words such as "project" are not
  node kinds the platform interprets. Permissions accumulate downward and
  limits bound downward.
- [Organization, workspace, folder, and project](https://github.com/straw-hat-team/adr/blob/main/src/adrs/1917971986/README.md)
  names the tenant root Organization, its direct children Workspaces, and
  deeper nodes Folders. Work attaches at a workspace or below, never at the
  root, and any level may widen its visibility to a wider audience or narrow
  it to invited members, with ancestors' limits capping descendants.
- [ADR#0025](./0025-agent-definition-data-ownership.md) already settles who
  owns what for Agents: the registry records `parent` at birth, the hierarchy
  owns moves, current placement, derived scope, and visibility, and the policy
  plane owns grants, shares, ceilings, and authorization.

Two pressures shape what remains.

**Private cuts against inheritance.** Under additive inheritance, everyone
who can see a workspace sees everything attached to it. "Only invited
members" therefore cannot be expressed as a grant. It is a limit, which the
hierarchy decision permits to bound downward, but no decision in this
repository yet says where that limit lives or when it takes effect.

**Creation must not leak.** If a project is created first and made private
afterwards, there is a window in which the whole workspace can discover it.
For a project the creator meant to keep private, that window is the failure.

## Decision

### 1. A Project is a resource attached through `parent`, not a hierarchy node

A Project has its own decider stream under
`proto/trogonai/projects/projects/v1alpha1`, following the command, event,
state, and fault layout of the Agent and Proposal modules. It owns:

| Fact | Owner | Changes by |
| --- | --- | --- |
| `project_id` | Project | Immutable |
| `display_name` (what are you working on) | Project | Rename |
| `goal` (what are you trying to achieve) | Project | Restating the goal |
| Lifecycle (active, archived) | Project | Archive, unarchive |
| Creation placement (`parent` at birth) | Project | Immutable fact |
| Initial access preset (at birth) | Project | Immutable fact |
| Current placement and moves | Hierarchy | Hierarchy operation |
| Current access, members, grants | Policy plane | Policy operation |

A project is never a node. It has fields and behavior; nodes are bare
identifiers. Work that belongs to a project later, such as tasks, references
the project by kinship (`ProjectId project`), not by hierarchy position.

### 2. `parent` is a `trogon.hierarchy.v1alpha1.NodeId` at a workspace or below

`ProjectCreated.parent` is typed `trogon.hierarchy.v1alpha1.NodeId`, as the
position-field decision prescribes. This requires the `trogon-proto` module
dependency at `v0.16.0` or later, where `NodeId` was introduced. The Agent
module's `string parent` predates that type; aligning it is a separate change
and is not a precedent for new modules.

Whether the node is a workspace or deeper, and not the tenant root, depends on
the live hierarchy. It is an admission-time rule checked before dispatch, in
the sense of [ADR#0064](./0064-schema-constraints-as-documentation.md)
section 6, and the Project fold does not read hierarchy facts.

### 3. The access choice is a preset, not the access model

Visibility is per audience and per capability, held as bindings in the
policy plane. What a person picks when creating a project is a preset over
those bindings, and the Project records only which preset was picked:

```text
ProjectAccessPreset
  INHERITED     whoever the parent level lets in
  MEMBERS_ONLY  a limit at the project: inherited work permissions stop
                here, and only principals holding a project membership pass
```

`MEMBERS_ONLY` is a limit attached at the project, in the downward-bounding
mood the hierarchy decision gives limits. It removes nothing from the
parent and grants nothing to anyone; membership is a separate policy
binding. Unspecified and unknown values reject.

`INHERITED` is what a product preselects when it asks the question, not
what the contract assumes when the answer is missing. Clients send the
preset explicitly; an omitted preset is rejected rather than resolved to
`INHERITED`, because silently inheriting would widen the audience of a
project whose creator may have meant to keep it private.

The enum starts with the presets the product offers today and is expected to
grow. Organization-wide and public presets are widening grants, which the
hierarchy decision lets accumulate downward, and adding them is a new enum
value, not a reshape. A preset is capped by its ancestors' limits at
admission: a project inside an invited-members folder cannot be created as
organization-wide or public. Finer distinctions, such as an audience that
may discover a project without reading it, live in policy bindings and never
in this enum.

The ceiling bounds work permissions only. Governance held at the tenant root,
audit and the ability to reassign or delete abandoned work, still reaches a
members-only project. A private project that no administrator can audit is a
project no one can recover when its last member leaves.

### 4. Initial access is a creation fact, and discovery fails closed

`ProjectCreated` records `initial_access_preset` beside `parent`, for the same
reason [ADR#0025](./0025-agent-definition-data-ownership.md) records
`parent` at birth: the creation request is the only moment the creator's
intent exists, and it must survive in the stream even though a different
plane owns every later change.

The policy plane applies the creation fact as its first binding for the
project. For `MEMBERS_ONLY`, that binding is the ceiling plus a membership for
the creating principal, so the creator is never locked out of their own
project. Until the policy plane has recorded the project's first binding, no
projection lists the project to anyone other than the creator. A project
whose access is not yet known is treated as members-only, never as inherited.

Later changes to access are policy operations. The Project stream records no
access-changed event, and a projection of current access reads the policy
plane, not the stream.

### 5. The creator is a recorded principal, not an owner

`ProjectCreated.creator` is the authenticated opaque principal that issued the
command, a human or an agent, recorded the way a proposal records its author.
It is provenance, not ownership. Per the hierarchy decision, who answers for
the project resolves from its node, and an `owner` field would be a stale copy
of that answer.

### 6. The command vocabulary

Following [ADR#0014](./0014-command-and-query-naming.md):

| Command | Event |
| --- | --- |
| `CreateProject` | `ProjectCreated` |
| `RenameProject` | `ProjectRenamed` |
| `RestateProjectGoal` | `ProjectGoalRestated` |
| `ArchiveProject` | `ProjectArchived` |
| `UnarchiveProject` | `ProjectUnarchived` |

There is no move command and no access command, because neither fact is the
Project's to change.

## Consequences

- The project surface maps one to one onto the contract: the name and goal
  are Project facts, and the visibility choice is the initial access preset.
- Choosing "only invited members" leaves no discoverable window, because
  discovery waits for the policy plane's first binding and defaults to
  members-only.
- Moving a project to another workspace or folder is a hierarchy operation.
  Policy attached to the destination applies from that moment, and a
  members-only ceiling travels with the project because it is attached to the
  project, not to the old node.
- Tenant administrators can audit and recover members-only projects without
  being members, and members-only does not hide a project from governance.
- Adopting `NodeId` requires bumping the `trogon-proto` dependency, and leaves
  the Agent module's `string parent` as a known divergence until it is
  aligned.
- The policy plane must model a resource-level ceiling and project
  memberships. This ADR fixes their semantics, not their storage or engine.

## Alternatives Considered

### A complete access model on the Project

Rejected. Encoding audiences and capabilities on the Project would freeze a
model that the policy plane must be free to grow, and it would duplicate
bindings that already govern folders and workspaces.

### Access as a mutable field on the Project

Rejected. It would make the Project stream a second owner of visibility,
which [ADR#0025](./0025-agent-definition-data-ownership.md) assigns to
the hierarchy and policy planes, and the two copies would diverge on the
first policy change made outside the Project.

### Private projects as their own hierarchy node

Rejected. Creating a child node per private project and granting members on
it still leaves the parent's audience able to see everything below, because
permissions accumulate downward. It also turns a resource into a position,
which the vocabulary decision rules out.

### Access applied only after creation

Rejected. A separate create-then-restrict sequence opens a window in which
the whole workspace can discover a project its creator meant to keep private.

## References

- [Resource hierarchy via untyped recursive nodes](https://github.com/straw-hat-team/adr/blob/main/src/adrs/4761776210/README.md)
- [Hierarchy position is referenced by a bare parent field](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6310044131/README.md)
- [Organization, workspace, folder, and project](https://github.com/straw-hat-team/adr/blob/main/src/adrs/1917971986/README.md)
- [Actor and Authority Taxonomy for Managed Systems](https://github.com/straw-hat-team/adr/blob/main/src/adrs/8779742261/README.md)
- [`trogon.hierarchy.v1alpha1.NodeId`](https://github.com/TrogonStack/trogon-proto/blob/main/proto/trogon/hierarchy/v1alpha1/node.proto)
- [ADR#0014: Command and Query Naming](./0014-command-and-query-naming.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0064: Invariants Are Declared in the Schema Before Anything Enforces Them](./0064-schema-constraints-as-documentation.md)
