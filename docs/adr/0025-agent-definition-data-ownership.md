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

ADR 0024 and this decision form one prerequisite architecture boundary for
the agent lifecycle implementation. Neither decision is sufficient without
the other.

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
inventory. Without a complete logical `AgentRevision`, the digest has no
normative content, the changed-field inventory can drift from the artifact it
describes, and the lifecycle contract can accidentally become the definition
contract.

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

A datum belongs to an `AgentRevision` only when all three statements are
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

### 1. Keep five ownership boundaries explicit

**The Agent registry record owns stable identity and lifecycle.** It owns the
agent id, tenant boundary, name used as the provisioning idempotency key,
accountable owner reference, immutable runtime constraint, active revision
pointer, and lifecycle state. Owner transfer is a dedicated audited operation,
not revision content. Hierarchy owns placement and derives scope or
visibility; Agent projections may expose `parent`, but the registry stream and
revision do not own that relationship. There is no independent `scope` field.
Opaque annotations are record metadata, cannot participate in selectors,
model input, or runtime behavior, and do not affect the behavior digest.

The runtime constraint participates in every revision digest by value or by
an immutable reference to the Agent-owned fact, so a revision and its
verification evidence remain self-describing. Changing runtime creates a
sibling agent, not another revision of the same agent.

**An AgentRevision owns one complete, immutable behavior declaration.** Its
logical bundle contains:

- the description used by model-driven routing;
- the model default and deterministic model parameters;
- authored instructions and declared turn wrappers;
- a typed `variables_schema` for session-start bindings;
- exact skill version pins;
- required and optional tool dependency declarations;
- required and optional delegate dependency declarations; and
- selectable labels that affect routing, grouping, or binding resolution.

An activated revision is content-addressed and never edited. Revision-owned
data may live in a content store rather than inside the registry event, but
physical placement does not change semantic ownership. The registry event
must carry an immutable reference and digest for the complete canonical
bundle.

The revision digest commits transitively to the resolved content of every
revision-owned artifact. A skill pin therefore contributes an immutable
skill-content digest, not only a version label that could later resolve to
different bytes.

**A Proposal owns change-in-flight facts.** It owns the pinned base revision
and digest, candidate bundle reference and digest, author, evidence,
canonical typed difference, change class, verdict, withdrawal, and
supersession metadata. These facts explain why a candidate did or did not
become a revision; they are not agent behavior. Activation records the
proposal provenance and assigns the revision number as required by ADR 0024.

Change classification must be derived from the canonical difference between
the proposal's pinned base revision and the immutable candidate captured by
ProposalOpened. A caller-supplied list may express intent, but it cannot be
the authoritative description of fields that changed. The proposal must
preserve the derived typed difference, or an immutable digest and reference
to it, so later readers can reproduce the classification. A verdict binds the
exact candidate reference and digest it judged.

The Proposal verdict owns approval or rejection and its rationale. The
evaluation plane owns rubrics and bindings, while Outcome records own measured
scores. A verdict references those immutable results by id and digest rather
than copying evaluation-owned data into the Proposal.

Activation requires the proposal's base revision and digest to match the
Agent's current active revision. A stale proposal must be rebased onto the
new active revision, receive a new canonical difference and digest, and be
verified again. Approval of one base does not authorize a different
candidate assembled after another activation.

Using ADR 0024's illustrative names, the lifecycle contracts preserve these
ownership boundaries:

- AgentProvisioned records stable Agent facts and the complete immutable
  reference and digest for active revision 1, without embedding a second
  partial definition beside the canonical artifact.
- ProposalOpened records the base, candidate, typed difference, and evidence,
  but no revision number.
- RevisionActivated records the exact approved Proposal and candidate digest;
  activation cannot substitute or rebuild the candidate.
- RevisionRolledBack selects an existing immutable revision artifact rather
  than creating another copy.
- AgentArchived prevents later activation but never deletes artifacts still
  referenced by revisions, proposals, sessions, verdicts, or outcomes.

Actor principal identities on commands and events are audit and provenance
facts owned by those events, not revision content. Authentication owns
principal kind and authorization policy.

**A Session owns resolved execution facts.** At session start, defaults and
declarations become facts: the pinned revision, actual runtime and model,
bound variable values, resolved work contract, session-local overrides,
selected memory and workspace context, resolved tool definitions, and the
context-assembly specification. The session ledger then owns the append-only
transcript, context projections, tool and delegation activity, and references
to outcomes.

The session may record the effective toolset at start, verification, and tool
call time, but authorization remains live. A recorded capability set never
turns a grant into a pinned session entitlement.

Tool and delegate declarations are selectors unless an explicit exact pin is
requested. The SessionContract pins the concrete ToolDefinition versions
resolved for session start. A delegate selector resolves when delegation
occurs, and the delegation event records the concrete agent and revision that
received the work. Verification records the concrete tool versions,
effective toolset, and delegate revisions it exercised. Grants and
revocations remain live in every case.

Selector evaluation respects revision pinning. Routing a new Session uses
the Agent's active revision labels. Policy and evaluation applied to an
existing Session use that Session's pinned revision labels, even after a
later activation changes the active labels. Live policy is re-evaluated
against pinned behavior facts; activation never rewrites the identity of an
in-flight execution.

**Memory owns learned state that changes independently of behavior
revisions.** Episodic memory, user preferences, project conventions, and
domain models live in scoped memory resources. A session records the memory
selection or snapshot it received. A memory write never edits a pinned
revision, and sharing an agent never permits one scope's memory to enter
another scope's session.

### 2. Treat charter and learned layer as governance classes

The charter and learned layer classify changes; they are not two mutable
documents and do not create competing sources of truth. Every activation
still produces one complete `AgentRevision`.

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

There is no generic `schema` field on Agent or AgentRevision.

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

- `AgentRevision.variables_schema` declares the names, types, and requiredness
  of session-start values referenced by the behavior bundle. Variable values
  belong to the Session.
- `ToolDefinition.input_schema` declares the input accepted by that version
  of a reusable tool. An AgentRevision declares required or optional tool
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

Validate variable bindings and work input before creating the Session.
Validate tool arguments at the Tool boundary. Validate the final result
before recording successful completion.

### 4. Assemble model input in the Session

The AgentRevision is not the literal model input. The runtime adapter builds
model input deterministically from separately owned layers:

1. A cached stable prefix from revision-owned instructions and the pinned
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

The following data never enters an AgentRevision:

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

An AgentRevision declares what it needs. External planes decide what it may
use, what work it receives, how it is judged, and what happened. Changes on
those planes do not mint agent revisions. If observed evidence justifies a
behavior change, a curator turns that evidence into a Proposal through the
ADR 0024 workflow.

Definitions pin; authorization never pins. Grants and revocations are
evaluated live at each protected action. Credential rotation remains a live
security operation rather than a behavior revision.

## Consequences

- The platform can answer five different audit questions without conflating
  their sources: which agent existed, which revision ran, which proposal
  justified it, what the model saw, and what was authorized when an action
  occurred.
- The agent platform needs typed contracts for AgentRevision,
  `variables_schema`, WorkContract, and SessionContract. A partial charter
  plus an opaque content digest is not a complete definition contract.
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
