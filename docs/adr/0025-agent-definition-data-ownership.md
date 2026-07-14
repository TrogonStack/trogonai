---
number: "0025"
slug: agent-definition-data-ownership
status: draft
date: 2026-07-13
---

# ADR 0025: Agent Definition Data Ownership

## Context

[ADR 0024](./0024-agent-platform-stream-topology.md) separates the agent
registry record from the proposal workflow. Activation mints an immutable
revision, sessions pin a revision, and rejected or withdrawn proposals never
enter the registry history. That topology deliberately does not answer the
next question: what data is part of the agent, what data is part of an
activated revision, and what merely influences one execution?

[ADR 0024](./0024-agent-platform-stream-topology.md) and this decision form
one prerequisite architecture boundary for the agent lifecycle
implementation. Neither decision is sufficient without the other.

The [agent-platform research](../research/agent-platform/index.md) found one
stable decomposition across otherwise different products: a durable
definition, an execution or session, and memory are separate resources.
Managed products version the definition and resolve it into a session, while
memory, environment, credentials, and runtime state follow independent
lifecycles. The [decision record](../research/agent-platform/decision-record.md)
then refined the platform boundary further: the agent owns its facts and
dependency declarations; another party's permission, judgment, funding,
scheduling, routing, or observation stays beside the agent on its own plane.

The research also exposes why a field inventory alone is insufficient:

- [Jido](../research/agent-platform/products/jido.md) calls its mutable
  in-process state validator a `schema`.
- [ADK](../research/agent-platform/products/adk-a2a.md) uses input and output
  schemas for the payload handled by one invocation.
- Tool schemas describe a reusable tool's invocation interface.
- Prompt variables describe values that a caller must bind when starting a
  session.
- Rubrics judge whether an outcome is good, not whether its data has the
  expected shape.

Putting any of these behind a generic `schema` field would erase who owns the
contract, when it binds, and which change must mint a revision. The same
problem appears outside schemas. A model default, a live tool grant, a memory
selection, a budget ceiling, and an observed success rate can all influence a
run, but they have different authors, consistency rules, security properties,
and change cadences.

The first lifecycle contract draft makes this ambiguity visible. It models a
partial charter and a digest for a larger bundle, while instructions, turn
wrappers, variables, skills, and description appear only in a changed-field
inventory. Without a complete logical `BehaviorBundle` and an explicit
`AgentRevision` binding, the digest has no normative content, the
changed-field inventory can drift from the artifact it describes, and the
lifecycle contract can accidentally become the definition contract.

This decision must preserve five properties:

1. An agent can evolve its behavior without acquiring authority over policy,
   secrets, or evaluation.
2. A session can answer which definition it ran and what the model actually
   saw.
3. Revisions change only when agent behavior changes, not when memory,
   policy, work, or observations change.
4. Reusable contracts stay with the resource that owns them instead of being
   copied into every agent.
5. Verification results remain meaningful because every behavior-defining
   input is versioned or resolved into a recorded session fact.

## Decision

Place each datum with the resource whose invariants and change cadence it
describes.

A datum belongs to a `BehaviorBundle` only when all three statements are
true:

1. It is the agent's own behavior or dependency declaration.
2. Every session pinned to that revision should begin with the same
   declaration.
3. Changing it should invalidate behavioral verification and require a new
   proposal and activation.

If the datum expresses another resource owner's stance, one work item
supplies it, a session resolves it, or execution merely observes it, the
datum does not belong to the revision. A human may author agent instructions;
authorship alone does not change which resource owns the declaration.

### 1. Use this canonical logical model

This is the normative implementation handoff. It defines logical records and
ownership, not protobuf field spelling, physical storage, or event envelopes.
Large immutable values may live outside an event, but the event must preserve
their reference and digest. Identifier types, field cardinalities, canonical
serialization, and command or event envelopes remain contract-design work;
that work may not move data across the ownership boundaries decided here.

The distinction hidden by the earlier draft is explicit in this model: a
BehaviorBundle exists before activation, while an AgentRevision does not.
AgentRevision is the numbered binding created by provisioning or activation.

```text
Agent
  agent_id
  tenant_id
  name
  accountable_owner
  runtime_constraint
  active_revision -> AgentRevision
  lifecycle_state
  annotations

AgentRevision
  agent_id
  revision_number
  bundle_ref -> BehaviorBundle
  bundle_digest
  source = provisioning | proposal_id

BehaviorBundle
  runtime_constraint_commitment -> Agent.runtime_constraint
    (derived, immutable, not proposable)
  description
  model_default + deterministic_parameters
  instructions
  turn_wrappers
  variables_schema
  exact_skill_pins
  required_tool_declarations + optional_tool_declarations
  required_delegate_declarations + optional_delegate_declarations
  selectable_labels

Proposal
  proposal_id
  agent_id
  base_revision -> AgentRevision
  candidate_bundle_ref -> BehaviorBundle
  candidate_bundle_digest
  canonical_typed_difference | difference_ref + difference_digest
  derived_change_class
  author
  evidence
  terminal? = verdict | withdrawal
  verdict = decision + verifier + rationale
            + evaluation_result_refs + evaluation_result_digests
  withdrawal = author
  supersedes?

WorkContract
  versioned_ref
  input_schema
  result_schema

ToolDefinition
  versioned_ref
  input_schema

SessionContract
  session_id
  agent_id
  pinned_revision -> AgentRevision
  actual_runtime + actual_model
  variable_values
  resolved_work_contract? -> versioned WorkContract
  work_item_ref?
  work_input? = immutable_ref_or_snapshot + digest
  session_overrides
  memory_selection_or_snapshot -> Memory
  workspace_context
  resolved_tool_definitions -> ToolDefinition[]
  context_assembly_specification

SessionLedger
  transcript
  context_projections
  model_call = resolved_input
               | input_digest + immutable_refs
                 + assembly_digest + projection_version
  tool_activity
  delegation_activity
  outcome_refs

Memory
  memory_id
  parent -> Hierarchy
  episodic_memory
  user_preferences
  project_conventions
  domain_models
```

The immediate agent-lifecycle contract is `Agent` + `BehaviorBundle` +
`AgentRevision` + `Proposal`. The remaining records are separate resources
joined through references. They are listed here to fix the boundary, not to
pull their state into the Agent or Proposal aggregates.

```text
Proposal.candidate_bundle_ref -> BehaviorBundle
AgentRevision.bundle_ref      -> the exact same BehaviorBundle
```

Activation assigns the revision number and records provenance. It never
rebuilds the candidate. `bundle_digest` is the revision digest and remains
unchanged through activation; the revision number and provenance are not part
of the behavior digest.

Everything outside that model remains with its existing owner:

- hierarchy owns `parent`, placement, moves, derived scope, and visibility;
- policy and security own grants, access shares, credential bindings, secret
  material, budgets, ceilings, and authorization;
- evaluation and Outcome own rubrics, bindings, verdict policy, and scores;
- scheduling and routing own triggers, channels, destinations, and routing;
- work and environment resources own work payloads and workspace contents;
- the runtime and Session own runtime checkpoints; and
- skill resources own skill content, while a BehaviorBundle owns exact pins.

The revision boundary is therefore mechanical:

| Change | Owner and effect | New AgentRevision? |
| --- | --- | --- |
| Instructions, wrappers, description | BehaviorBundle proposal | Yes |
| Model defaults, parameters, or `variables_schema` | BehaviorBundle proposal | Yes |
| Skill pins or tool/delegate declarations | BehaviorBundle proposal | Yes |
| Selectable labels | BehaviorBundle proposal | Yes |
| Skill content | Skill resource; a revision changes only when its pin changes | No by itself |
| Memory content | Memory write | No |
| Variable values, overrides, or resolved tools | SessionContract | No |
| Transcript, actions, resolved delegates, or outcomes | SessionLedger | No |
| Tool schema or work schema | ToolDefinition or WorkContract version | No |
| Parent, placement, scope, or visibility | Hierarchy operation | No |
| Owner | Dedicated Agent lifecycle operation | No |
| Annotations | Agent metadata operation | No |
| Agent id or tenant | Immutable Agent identity | No |
| Name or runtime constraint | Immutable; runtime change creates a sibling Agent | No |
| Grants, secrets, budgets, evaluation, routing, or schedules | External plane | No |

At execution time, the boundaries compose as follows:

```text
model input =
  BehaviorBundle-owned stable prefix
  + SessionContract bindings and resolved facts
  + current Session context
  + on-demand content

authorized action =
  requested action
  + live policy, grants, and credential resolution
```

Live authorization is never frozen into the SessionContract or placed in
model input.

#### Boundary rationale

The Agent registry owns stable identity and lifecycle. Owner transfer is a
dedicated audited operation, not revision content. Hierarchy owns placement
and derives visibility; Agent projections may expose `parent`, but the
registry stream and revision do not own it. There is no independent `scope`
field. Annotations are opaque record metadata and never affect selection,
model input, runtime behavior, or the behavior digest.

The runtime constraint participates in every revision digest by value or an
immutable reference to the Agent-owned fact, keeping verification evidence
self-describing. Changing runtime creates a sibling Agent rather than a new
revision.

A BehaviorBundle is content-addressed and never edited. Its digest
commits transitively to every bundle-owned artifact, so a skill pin commits
to immutable skill content rather than only a mutable version label.

Proposal facts explain why a candidate did or did not become a revision; they
are not behavior content. Change classification comes from the canonical
difference between the pinned base and immutable candidate. Caller-supplied
changed fields may express intent but cannot be authoritative. A verdict
binds the candidate already fixed by ProposalOpened without duplicating its
reference or digest. It references evaluation results rather than copying
them.

Activation requires the Proposal base to match the Agent's current active
AgentRevision. A stale Proposal must be rebased, differenced, digested, and
verified again.

Using the illustrative names from
[ADR 0024](./0024-agent-platform-stream-topology.md), the lifecycle contracts
preserve the model:

- AgentProvisioned records Agent facts and the complete immutable reference
  and digest for revision 1.
- ProposalOpened records the base, candidate, typed difference, and evidence,
  but no revision number.
- RevisionActivated records Proposal provenance plus the candidate reference
  and digest.
- RevisionRolledBack selects an existing AgentRevision.
- AgentArchived blocks activation but never deletes referenced artifacts.

Actor identities on commands and events are provenance facts, not behavior
content. Authentication owns principal kind and authorization policy.

The SessionContract captures resolved start facts, while the SessionLedger
owns append-only execution facts. Tool and delegate declarations are
selectors unless explicitly pinned. ToolDefinition versions resolve at
Session start, delegate selectors resolve when delegation occurs, and the
ledger records the concrete delegated Agent and revision.

Routing a new Session uses the Agent's active revision labels. Policy and
evaluation for an existing Session use its pinned revision labels, even after
activation changes the active labels. Verification records the concrete tool
versions, effective toolset, and delegate revisions it exercised.

Recorded capabilities do not become entitlements. Policy remains live at
every protected action and is evaluated against the Session's pinned behavior
facts. Activation never changes an in-flight Session's identity.

Memory changes independently of behavior revisions. A Session records its
memory selection or snapshot. Sharing an Agent never permits Memory selected
from one hierarchy and policy context to enter another Session.

### 2. Treat charter and learned layer as governance classes

The charter and learned layer classify changes; they are not two mutable
documents and do not create competing sources of truth. Every activation
still produces an `AgentRevision` bound to one complete `BehaviorBundle`.

- Learned-layer changes include instructions, turn wrappers, description,
  skill pins, optional tool or delegate declarations, and selectable labels
  that do not alter a well-known routing or comparison group.
- Charter-class changes include the model default and deterministic model
  parameters, required tool or delegate declarations, well-known grouping
  labels such as `family`, and the `variables_schema`.
- Name and runtime are immutable agent facts. Placement and owner changes use
  dedicated operations rather than proposals for behavior revisions.

For v1, the variable schema is charter-class because it is an interface
offered to session callers. Adding or removing a binding, changing a type, or
changing requiredness can make previously valid session starts invalid. The
agent may freely evolve how existing variables are used inside learned-layer
instructions, but it may not silently rewrite the caller's contract.

This refines the research record's earlier classification of all variable
changes as learned-layer changes.

### 3. Put every schema on the contract it validates

There is no generic `schema` field on Agent, AgentRevision, or BehaviorBundle.

The product corpus does not converge on structured work schema placement.
ADK, OpenAI, and Vercel attach structured input or result configuration to an
agent-like code object, while
[Devin](../research/agent-platform/products/devin.md) binds its structured
result schema when creating a session. This platform chooses a separate
WorkContract because work is reusable across agents and an agent is reusable
across kinds of work. That is a TrogonAI ownership decision, not an industry
invariant.

This ADR introduces WorkContract as an independently versioned contract and
SessionContract as the immutable resolution of a WorkContract for one
Session. Their command and event lifecycles remain future implementation
work; their ownership and binding time are decided here.

- `BehaviorBundle.variables_schema` declares the names, types, and requiredness
  of session-start values referenced by the behavior bundle. Variable values
  belong to the Session.
- `ToolDefinition.input_schema` declares the input accepted by that version
  of a reusable tool. A BehaviorBundle declares required or optional tool
  dependencies; it does not copy the tool's schema.
- `WorkContract.input_schema` and `WorkContract.result_schema` declare the
  structured input and successful result for one kind of work. They do not
  belong to the agent because one agent can perform multiple kinds of work,
  and multiple agents can satisfy the same work contract.
- A `SessionContract` resolves and pins the selected WorkContract and the
  concrete versioned references needed by that execution.
- A runtime-state or checkpoint schema belongs to the runtime and Session
  implementation. Jido's state schema is this kind of contract, not evidence
  for a generic Agent schema.
- Rubrics and quality scores remain in EvaluationBinding and
  OutcomeRecorded. Shape validity is not outcome quality.

Use explicit names and versioned references or digests. Prefer
`result_schema` to an overloaded `output_schema`. Generic conversational
sessions may omit work input and result schemas and use text input and result.

The work resource owns its payload. SessionContract records an immutable work
item and input reference, snapshot, or digest so the execution remains
auditable without moving ownership into the Agent.

Validate variable bindings and work input before creating the Session.
Validate tool arguments at the Tool boundary. Validate the final result
before recording successful completion.

### 4. Assemble model input in the Session

The BehaviorBundle referenced by AgentRevision is not the literal model
input. The runtime adapter builds model input deterministically from
separately owned layers:

1. A cached stable prefix from BehaviorBundle instructions and the pinned
   skill index.
2. Session-start bindings such as variable values, the WorkContract, memory
   and workspace context, and resolved tool descriptions.
3. A volatile tail for platform-owned time, events, audience information,
   and steering that must remain fresh.
4. On-demand loads such as skill bodies and fetched content.

The revision declares behavior, the runtime adapter enacts the assembly, and
the session ledger records it. Every model call records the resolved input or
a digest plus immutable references and the projection version needed to
reconstruct what the model saw. Compression and elision change the context
projection, not transcript history, and are ledgered as such.

The revision digest answers "what did the agent declare?" The session
assembly digest answers "what did the model see?" They must not be the same
field or be treated as interchangeable.

Credential values are never model context. A dependency declaration may say
that a tool is needed, while grants, credential bindings, and secret
resolution authorize and equip the tool call outside the prompt.

### 5. Keep external stances and observations beside the agent

The following data never enters a BehaviorBundle:

- grants, access shares, credential bindings, secret references, and secret
  material, consistent with
  [ADR 0023](./0023-secret-management-and-key-custody-direction.md);
- budget, token, turn, concurrency, and delegation ceilings;
- rubrics, evaluation bindings, verdict policy, and outcome scores;
- schedules, triggers, channel bindings, delivery destinations, and routing
  policy;
- ownership policy and authorization principal kinds;
- success rates, usage counts, cost, priority, and fitness projections;
- environment contents, workspace contents, memory records, transcripts, and
  runtime checkpoints; and
- work payloads, WorkContracts, and tool invocation schemas.

A BehaviorBundle declares what it needs. External planes decide what it may
use, what work it receives, how it is judged, and what happened. Changes on
those planes do not mint agent revisions. If observed evidence justifies a
behavior change, a curator turns that evidence into a Proposal through the
[ADR 0024](./0024-agent-platform-stream-topology.md) workflow.

Definitions pin; authorization never pins. Grants and revocations are
evaluated live at each protected action. Credential rotation remains a live
security operation rather than a behavior revision.

## Consequences

- The platform can answer five different audit questions without conflating
  their sources: which agent existed, which revision ran, which proposal
  justified it, what the model saw, and what was authorized when an action
  occurred.
- The agent platform needs typed contracts for Agent, BehaviorBundle,
  AgentRevision, `variables_schema`, WorkContract, and SessionContract. A
  partial charter plus an opaque content digest is not a complete definition
  contract.
- Registry and proposal events may remain small. Large prompt and skill
  content can live outside the event stream when canonical serialization,
  immutable retrieval, and digest verification are defined.
- Proposal change class and changed fields must come from the typed canonical
  revision difference. They cannot depend only on duplicate metadata supplied
  independently of the candidate artifact.
- Activation rejects a proposal based on a revision that is no longer active.
  Rebase changes the candidate and therefore requires a new digest and
  verification.
- Tool and work schemas evolve once at their owning resource. Agents and
  sessions pin versioned references instead of copying those schemas and
  allowing them to drift.
- Memory, policy, and evaluation can evolve without polluting revision
  history. This adds joins to read models, but each resource or plane now
  enforces one owner's invariants and changes at its natural cadence.
- Session creation gains explicit validation and resolution work. In return,
  failures in variable bindings, work input, required dependencies, and
  contract resolution happen before tokens or tool calls are spent.
- Treating variable-schema changes as charter-class slows autonomous changes
  to that interface, but prevents the evolution loop from silently breaking
  callers.
- Exact prompt ordering, provider-specific message roles, compression
  algorithms, and wrapper cadence remain runtime-adapter decisions. This ADR
  fixes ownership, binding time, and audit requirements rather than one
  universal prompt format.
