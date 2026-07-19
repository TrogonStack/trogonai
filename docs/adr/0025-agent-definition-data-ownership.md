---
number: "0025"
slug: agent-definition-data-ownership
status: draft
date: 2026-07-13
---

# ADR#0025: Agent Definition Data Ownership

## Context

[ADR#0024](./0024-agent-platform-stream-topology.md) separates the agent
registry record from the proposal workflow. Activation mints an immutable
revision, [sessions](../glossary/session) pin a revision, and rejected or withdrawn proposals never
enter the registry history. That topology deliberately does not answer the
next question: what data is part of the agent, what data is part of an
activated revision, and what merely influences one execution?

[ADR#0024](./0024-agent-platform-stream-topology.md) and this decision form
one prerequisite architecture boundary for the agent lifecycle
implementation. Neither decision is sufficient without the other.

The [agent-platform research](../research/agent-platform/index.md) found one
stable decomposition across otherwise different products: a durable
definition, an execution or session, and memory are separate resources.
Managed products version the definition and resolve it into a session, while
memory, environment, credentials, implementation state, and hosting state
follow independent lifecycles. The [decision record](../research/agent-platform/decision-record.md)
then refined the platform boundary further: the agent owns its facts and
dependency declarations; another party's permission, judgment, funding,
scheduling, routing, or observation stays beside the agent on its own plane.

This [ADR](../glossary/adr) defines the agent entity and its configuration, and nothing else.
The scope follows the strongest infrastructure model in the study,
[Bedrock AgentCore](../research/agent-platform/products/bedrock-agentcore.md):
a bare entity made of identity and immutable numbered revisions, with
everything else detachable; the newest revision is what runs, and reverting
mints a new revision from a prior configuration. Sessions, memory, work, and
tools appear here only as boundary references; their own shapes are separate,
later decisions.

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
problem appears outside schemas. An exact [model selection](../glossary/modelselection), a live tool grant,
a memory selection, a budget ceiling, and an observed success rate can all
influence a run, but they have different authors, consistency rules, security
properties, and change cadences.

The first lifecycle contract draft makes this ambiguity visible. It models a
partial charter and a digest for a larger configuration, while instructions, turn
wrappers, variables, skills, and description appear only in a changed-field
inventory. Without a complete logical `AgentConfiguration` and an explicit
`AgentRevision` binding, the digest has no normative content, the
changed-field inventory can drift from the artifact it describes, and the
lifecycle contract can accidentally become the definition contract.

This decision must preserve four properties:

1. An agent can evolve its behavior without acquiring authority over policy,
   secrets, or evaluation.
2. Revisions change only when agent behavior changes, not when memory,
   policy, work, or observations change.
3. Reusable contracts stay with the resource that owns them instead of being
   copied into every agent.
4. Every execution is traceable to the exact revision it ran, by reference
   and digest, so verification results remain meaningful.

## Decision

Place each datum with the resource whose invariants and change cadence it
describes.

A datum belongs to an `AgentConfiguration` only when all three statements are
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

### 1. Model the agent as four records

This is the normative implementation handoff, and it is deliberately small:
the agent is four records. The model defines logical records and ownership,
not [protobuf](../glossary/protocol-buffers) field spelling, physical storage, or [event envelopes](../glossary/event-envelope). Large
immutable values may live outside an [event](../glossary/event), but the event must preserve
their reference and digest. Identifier types, field cardinalities, canonical
serialization, and [command](../glossary/command) or event envelopes remain contract-design work;
that work may not move data across the ownership boundaries decided here.

The distinction hidden by the earlier draft is explicit in this model: an
AgentConfiguration exists before activation, while an AgentRevision does not.
AgentRevision is the numbered binding created by provisioning or activation.

```text
Agent
  agent_id
  name
  parent -> Hierarchy node
    (placement; recorded at provisioning, moves are hierarchy operations)
  latest_revision -> AgentRevision
    (derived: the highest minted revision, what new sessions run by
     default; not a stored pointer)
  lifecycle_state
  annotations

AgentRevision
  agent_id
  revision_number
  configuration_ref -> AgentConfiguration
  configuration_digest
  source = provisioning | proposal_id | revert_of_revision

AgentConfiguration
  description
  agent_implementation -> AgentImplementation
    implementation_version_ref -> AgentImplementationVersion
      (exact typed implementation kind + immutable version)
    implementation_definition_digest
    implementation_configuration
      (typed for that exact implementation version)
    implementation_configuration_digest
  primary_model_selection -> ModelSelection
    model_version_ref -> ModelVersion
    model_definition_digest
    deterministic_parameters
  auxiliary_model_selections
    role + ModelSelection
  instructions
  turn_wrappers
  variables_schema
  exact_skill_pins
  required_tool_declarations + optional_tool_declarations
  required_delegate_declarations + optional_delegate_declarations
  required_memory_declarations + optional_memory_declarations
    (selectors by kind, never an instance reference)
  selectable_labels

Proposal
  proposal_id
  agent_id
  base_revision -> AgentRevision
  candidate_configuration_ref -> AgentConfiguration
  candidate_configuration_digest
  canonical_typed_difference | difference_ref + difference_digest
  derived_change_class
  author
  evidence
  terminal? = verdict | withdrawal
  verdict = decision + verifier + rationale
            + evaluation_result_refs + evaluation_result_digests
  withdrawal = author
  supersedes?
```

`parent` is the only placement fact and everything contextual derives from
it. AgentProvisioned records it at birth; after that, moves are hierarchy
operations, so the hierarchy domain owns the current value and the Agent
record projects it. Ownership and authority are policy-plane bindings on
the hierarchy, inherited and evaluated live at each protected action, never
registry fields. The customer an agent belongs to is fixed by its creation
placement and never changes; whether customers share infrastructure or
receive dedicated cells is a deployment decision, and no location or
physical topology appears in this model.

`Agent` owns no selectable label map. `AgentConfiguration.selectable_labels`
owns every selector key, including `family`, because changing one can alter
routing, evaluation binding, and comparison groups. An Agent [projection](../glossary/projection) may
expose the latest revision's labels, and a session resolves labels from its
pinned revision, but neither projection is an independent writable copy.

```text
Proposal.candidate_configuration_ref -> AgentConfiguration
AgentRevision.configuration_ref      -> the exact same AgentConfiguration
```

Activation assigns the revision number and records provenance. It never
rebuilds the candidate. `configuration_digest` is the revision digest and
remains unchanged through activation; the revision number and provenance are
not part of the configuration digest. The digest commits transitively to the
exact implementation version, implementation definition, typed implementation
configuration, primary model, auxiliary models, deterministic parameters, and
every other configuration-owned artifact.

### 2. Keep adjacent resources behind references

This ADR does not define the shape of sessions, memory, work, or tools. For
each adjacent resource it decides only three things: the resource is
separate, which datum it owns, and how it references the agent. Their full
contracts are future decisions in their own domains, and those decisions may
not move data across the boundary fixed here.

- **Session.** A session pins exactly one AgentRevision at start, by reference
  and digest. Its immutable [SessionExecutionPlan](../glossary/sessionexecutionplan) copies the revision's exact
  implementation version and definition digest, implementation configuration
  digest, and primary and auxiliary ModelSelection values. It never selects a
  newer implementation or another model. The plan resolves only external facts:
  [ResolvedModelRoute](../glossary/resolvedmodelroute) values that bind those exact models to authorized
  credential routes, variable bindings, tool versions, memory selections, work
  input, and other dependencies under
  [ADR#0031](./0031-agent-implementation-and-session-plan.md).
  Session-owned [ExecutionAttempt](../glossary/executionattempt) facts separately record behavior-neutral
  hosting, placement, restart, and attestation evidence without changing the
  plan.
  The AgentConfiguration is not the literal model input; the pinned
  [AgentImplementation](../glossary/agentimplementation) adapter assembles input from the configuration plus
  session-resolved facts, and the session must record enough references and
  digests to reconstruct what the model saw.
  The revision digest answers "what did the agent declare?"; any
  session-side digest answers "what did the model see?"; they are never the
  same field. Activation never changes an in-flight session's pin.
- **Memory.** Its own resource and lifecycle under hierarchy and policy.
  The configuration declares memory dependencies by kind selector, never by
  instance; a session resolves those declarations against its hierarchy
  context under live policy, possibly to several memories or none, and
  records each selection as reference and digest. Memory writes never mint
  revisions, and sharing an agent never moves memory across hierarchy or
  policy contexts. Memory's internal structure is a memory-domain decision.
- **ToolDefinition.** A versioned reusable tool that owns its input schema.
  An AgentConfiguration declares tool dependencies by selector or exact pin; it
  never copies a tool's schema. Versions resolve at session start.
- **WorkContract.** A versioned contract that owns the input and result
  schemas for one kind of work, separate from the agent because one agent
  performs many kinds of work and many agents can satisfy one contract. A
  session resolves and pins the selected contract; the work resource owns
  the payload, and the session records the work input as an immutable
  reference and digest.

Everything else remains with its existing owner:

- hierarchy owns `parent`, placement, moves, derived scope, and visibility;
- policy and security own grants, access shares, credential bindings, secret
  material, budgets, ceilings, and authorization;
- evaluation and Outcome own rubrics, bindings, verdict policy, and scores;
- scheduling and routing own triggers, channels, destinations, and routing;
- work and environment resources own work payloads and workspace contents;
- the pinned `AgentImplementation` owns loop-state semantics, while the
  session owns transcripts and recorded implementation checkpoints;
- the Session launch and deployment planes own process, container, isolation,
  network, filesystem, hosting authentication, and execution-lifecycle facts;
  and
- skill resources own skill content, while an AgentConfiguration owns exact pins.

The revision boundary is therefore mechanical:

| Change | Owner and effect | New AgentRevision? |
| --- | --- | --- |
| Instructions, wrappers, description | AgentConfiguration proposal | Yes |
| AgentImplementation kind, version, typed configuration, or digest | AgentConfiguration proposal | Yes |
| Primary or auxiliary model selections, parameters, or `variables_schema` | AgentConfiguration proposal | Yes |
| Skill pins or tool/delegate/memory declarations | AgentConfiguration proposal | Yes |
| Selectable labels | AgentConfiguration proposal | Yes |
| Skill content | Skill resource; a revision changes only when its pin changes | No by itself |
| Memory content | Memory write | No |
| Variable bindings, work input, or resolved dependencies | Session start facts | No |
| Exact implementation and model pins copied from AgentRevision | Session start record | No |
| ResolvedModelRoute or behavior-neutral hosting fact | Session or deployment fact | No |
| Transcript, actions, resolved delegates, or outcomes | Session execution record | No |
| Tool schema or work schema | ToolDefinition or WorkContract version | No |
| Parent, placement, scope, or visibility | Hierarchy operation | No |
| Ownership or authority | Policy plane binding on hierarchy | No |
| Annotations | Agent metadata operation | No |
| Agent id | Immutable Agent identity | No |
| Name | Immutable Agent fact | No |
| Grants, secrets, budgets, evaluation, routing, or schedules | External plane | No |

#### Worked example: one proposal becomes revision 2

The following values use the logical names above. They are not proposed
protobuf field names.

| Record | Concrete binding |
| --- | --- |
| Proposal `prop-7f3a` | candidate `configuration-pr-reviewer-v2` |
| AgentRevision 2 | configuration `configuration-pr-reviewer-v2` |

<details>
<summary>Complete logical example data</summary>

```yaml
agent:
  agent_id: agent-pr-reviewer
  name: pr-reviewer
  parent: project-example
  latest_revision:
    revision_number: 2
  lifecycle_state: active
  annotations:
    display_name: Pull Request Reviewer

agent_configurations:
  - configuration_ref: configuration-pr-reviewer-v1
    configuration_digest: "sha256:0101010101010101010101010101010101010101010101010101010101010101"
    content:
      description: Reviews pull requests for correctness and maintainability.
      agent_implementation:
        implementation_version_ref:
          kind: managed
          version: 3
        implementation_definition_digest: "sha256:0202020202020202020202020202020202020202020202020202020202020202"
        implementation_configuration:
          context_strategy: rolling-summary-v2
          max_tool_iterations: 24
        implementation_configuration_digest: "sha256:0303030303030303030303030303030303030303030303030303030303030303"
      primary_model_selection:
        model_version_ref:
          model_id: model-reviewer
          version: 1
        model_definition_digest: "sha256:0404040404040404040404040404040404040404040404040404040404040404"
        deterministic_parameters:
          temperature: 0.2
          max_output_tokens: 4096
      auxiliary_model_selections: []
      instructions:
        - Report correctness defects with file and line evidence.
        - Separate blocking findings from suggestions.
      turn_wrappers:
        - wrapper-repository-context-v1
      variables_schema:
        review_depth:
          type: string
          allowed_values: [standard, strict]
          required: true
        output_language:
          type: string
          required: false
      exact_skill_pins:
        - skill_id: skill-code-review
          version: 3
          content_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
      required_tool_declarations:
        - selector:
            name: repository-read
      optional_tool_declarations:
        - selector:
            name: ci-status
      required_delegate_declarations: []
      optional_delegate_declarations:
        - selector:
            labels:
              family: security-review
      required_memory_declarations: []
      optional_memory_declarations:
        - selector:
            kind: project-conventions
      selectable_labels:
        family: code-review
        language: any

  - configuration_ref: configuration-pr-reviewer-v2
    configuration_digest: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    content:
      description: Reviews pull requests for correctness and maintainability.
      agent_implementation:
        implementation_version_ref:
          kind: managed
          version: 3
        implementation_definition_digest: "sha256:0202020202020202020202020202020202020202020202020202020202020202"
        implementation_configuration:
          context_strategy: rolling-summary-v2
          max_tool_iterations: 24
        implementation_configuration_digest: "sha256:0303030303030303030303030303030303030303030303030303030303030303"
      primary_model_selection:
        model_version_ref:
          model_id: model-reviewer
          version: 1
        model_definition_digest: "sha256:0404040404040404040404040404040404040404040404040404040404040404"
        deterministic_parameters:
          temperature: 0.2
          max_output_tokens: 4096
      auxiliary_model_selections: []
      instructions:
        - Report correctness defects with file and line evidence.
        - Treat missing regression coverage as blocking when behavior changes.
        - Separate blocking findings from suggestions.
      turn_wrappers:
        - wrapper-repository-context-v1
      variables_schema:
        review_depth:
          type: string
          allowed_values: [standard, strict]
          required: true
        output_language:
          type: string
          required: false
      exact_skill_pins:
        - skill_id: skill-code-review
          version: 4
          content_digest: "sha256:1515151515151515151515151515151515151515151515151515151515151515"
      required_tool_declarations:
        - selector:
            name: repository-read
      optional_tool_declarations:
        - selector:
            name: ci-status
      required_delegate_declarations: []
      optional_delegate_declarations:
        - selector:
            labels:
              family: security-review
      required_memory_declarations: []
      optional_memory_declarations:
        - selector:
            kind: project-conventions
      selectable_labels:
        family: code-review
        language: any

agent_revisions:
  - agent_id: agent-pr-reviewer
    revision_number: 1
    configuration_ref: configuration-pr-reviewer-v1
    configuration_digest: "sha256:0101010101010101010101010101010101010101010101010101010101010101"
    source: provisioning

  - agent_id: agent-pr-reviewer
    revision_number: 2
    configuration_ref: configuration-pr-reviewer-v2
    configuration_digest: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    source: prop-7f3a

proposal:
  proposal_id: prop-7f3a
  agent_id: agent-pr-reviewer
  base_revision:
    agent_id: agent-pr-reviewer
    revision_number: 1
    configuration_ref: configuration-pr-reviewer-v1
    configuration_digest: "sha256:0101010101010101010101010101010101010101010101010101010101010101"
  candidate_configuration_ref: configuration-pr-reviewer-v2
  candidate_configuration_digest: "sha256:1111111111111111111111111111111111111111111111111111111111111111"
  canonical_typed_difference:
    - field: instructions
      operation: replace
      before_digest: "sha256:1212121212121212121212121212121212121212121212121212121212121212"
      after_digest: "sha256:1313131313131313131313131313131313131313131313131313131313131313"
    - field: exact_skill_pins.skill-code-review
      operation: replace
      before_version: 3
      before_content_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
      after_version: 4
      after_content_digest: "sha256:1515151515151515151515151515151515151515151515151515151515151515"
  derived_change_class: learned-layer
  author: principal-curator
  evidence:
    - outcome_ref: outcome-review-missed-test-42
      outcome_digest: "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
  terminal:
    verdict:
      decision: approved
      verifier: principal-verifier
      rationale: Candidate catches the missed regression without new authority.
      evaluation_result_refs:
        - evaluation-result-108
      evaluation_result_digests:
        - "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
  supersedes: prop-b804
```

</details>

The equal `configuration_ref` and `configuration_digest` values are the load-bearing part of
the example: Proposal `prop-7f3a` was verified against exactly the bytes that
AgentRevision 2 later references. Activation added the number and provenance;
it did not rebuild the candidate.

After this activation:

| Change | Result |
| --- | --- |
| Write a new project convention to Memory | Memory changes; no revision 3 |
| Revoke a tool authorization | Next call is denied live; no revision 3 |
| Publish a new WorkContract version | Work plane changes; no revision 3 |
| Publish skill-code-review v5 | No revision 3 until a proposal changes the pin |
| Propose different instructions or a new skill pin | New configuration; activation may mint revision 3 |
| Publish AgentImplementationVersion 4 | No revision 3 until a proposal changes the exact pin |
| Propose implementation version 4, different implementation config, or another model | Charter-class configuration change; activation may mint revision 3 |
| Move the same pinned implementation to another conforming host | Deployment changes; no revision 3 |

#### Boundary rationale

The Agent registry owns stable identity and lifecycle. Hierarchy owns
placement and derives visibility; Agent projections may expose `parent`,
but the registry [stream](../glossary/stream) and revision do not own it. Ownership and authority
are policy bindings on the hierarchy, evaluated live: a move requires
rights on both source and destination, and taking authority over an agent
is a policy change, never a registry write. There is no independent `scope`
field. Annotations are opaque record metadata and never affect selection,
model input, implementation behavior, hosting behavior, or the configuration
digest.

`AgentConfiguration.agent_implementation` binds the exact typed
implementation kind, immutable AgentImplementationVersion reference and
definition digest, typed per-Agent implementation configuration, and
implementation-configuration digest. It is behavior, not placement. A newer
implementation version is never selected automatically. Changing the
implementation kind, version, definition digest, configuration, or
configuration digest creates a candidate AgentConfiguration and requires the
normal Proposal, verification, and activation workflow. The same Agent may
therefore evolve its implementation through controlled revisions without
mutating an existing revision or replacing the stable Agent identity.

The stable Agent contains neither an implementation binding nor a model
selection. Both are behavior facts owned by its immutable configurations.

`primary_model_selection` and every `auxiliary_model_selections` entry are
equally exact configuration facts. An AgentImplementationVersion may declare
which typed model roles it requires and validate their compatibility, but the
AgentConfiguration contains the exact model and deterministic parameters for
every role, including its ModelVersion reference and definition digest. Session
admission does not add a hidden model role, replace a model, or merge in a newer
implementation default.

The SessionExecutionPlan copies these implementation and model pins from the
AgentRevision and resolves a `ResolvedModelRoute` only to authorize each exact
model through an exact provider driver, provider connection, and credential
binding. Attempt-scoped model-access grants remain live authorization outside
the immutable plan. The provider driver is a Session fact because it adapts an
already-pinned model to an external provider rather than defining Agent
behavior. Its exact pin remains part of verification evidence. The plan also
resolves external dependencies and inputs under
[ADR#0031](./0031-agent-implementation-and-session-plan.md). It never resolves
the agent's implementation or model identity.

Process, container, microVM, remote host, placement, isolation, network,
filesystem, lifecycle, cancellation, recovery, and hosting authentication are
Session launch or deployment facts. They do not enter AgentConfiguration and
do not mint a revision while they remain behaviorally transparent. If a host
setting changes model-visible input, tools, loop decisions, model selection,
or terminal semantics, it is not merely hosting. The behaviorally relevant
setting must become part of the exact AgentImplementation definition or typed
AgentConfiguration before use.

Verification evidence records what it actually exercised: AgentRevision and
configuration digests, the exact implementation version and configuration
digests, exact model selections, ResolvedModelRoute values, tool versions, and
memory snapshots. A revision's score is always interpretable as "under these
resolved facts", never as a context-free claim.

An AgentConfiguration is content-addressed and never edited. Its digest
commits transitively to every configuration-owned artifact, so implementation
and skill pins commit to immutable content rather than only mutable version
labels.

Proposal facts explain why a candidate did or did not become a revision; they
are not behavior content. Change classification comes from the canonical
difference between the pinned base and immutable candidate. Caller-supplied
changed fields may express intent but cannot be authoritative. A verdict
binds the candidate already fixed by ProposalOpened without duplicating its
reference or digest. It references evaluation results rather than copying
them.

Activation requires the Proposal base to match the Agent's latest
AgentRevision. A stale Proposal must be rebased, differenced, digested, and
verified again.

Using the illustrative names from
[ADR#0024](./0024-agent-platform-stream-topology.md), the lifecycle contracts
preserve the model:

- AgentProvisioned records Agent facts and the complete immutable reference
  and digest for revision 1.
- ProposalOpened records the base, candidate, typed difference, and evidence,
  but no revision number.
- RevisionActivated records Proposal provenance plus the candidate reference
  and digest.
- Reverting is RevisionActivated minting a new revision from a previously
  activated configuration; digest equality proves the revert exact.
- AgentArchived blocks activation but never deletes referenced artifacts.

Actor identities on commands and events are provenance facts, not behavior
content. Authentication owns principal kind and authorization policy.

Routing a new session uses `selectable_labels` from the latest revision's
AgentConfiguration; an existing session keeps the labels of its pinned revision.
Recorded capabilities never become entitlements: policy remains live at
every protected action and is evaluated against the session's pinned
behavior facts.

### 3. Treat charter and learned layer as governance classes

The charter and learned layer classify changes; they are not two mutable
documents and do not create competing sources of truth. Every activation
still produces an `AgentRevision` bound to one complete `AgentConfiguration`.

- Learned-layer changes include instructions, turn wrappers, description,
  skill pins, optional tool, delegate, or memory declarations, and
  selectable labels that do not alter a well-known routing or comparison
  group.
- Charter-class changes include AgentImplementation kind, version, definition
  digest, typed configuration, or implementation-configuration digest; primary
  and auxiliary model selections and deterministic model parameters; required
  tool, delegate, or memory declarations; well-known grouping labels such as
  `family`; and the `variables_schema`.
- Name is an immutable agent fact. Implementation and model evolution use
  proposals for new revisions. Placement changes are hierarchy or deployment
  operations and authority changes are policy operations, never proposals for
  behavior revisions.

For v1, the variable schema is charter-class because it is an interface
offered to session callers. Adding or removing a binding, changing a type, or
changing requiredness can make previously valid session starts invalid. The
agent may freely evolve how existing variables are used inside learned-layer
instructions, but it may not silently rewrite the caller's contract.

This refines the research record's earlier classification of all variable
changes as learned-layer changes.

### 4. Put every schema on the contract it validates

There is no generic `schema` field on Agent, AgentRevision, or AgentConfiguration.

The product corpus does not converge on structured work schema placement.
ADK, OpenAI, and Vercel attach structured input or result configuration to an
agent-like code object, while
[Devin](../research/agent-platform/products/devin.md) binds its structured
result schema when creating a session. This platform chooses a separate
WorkContract because work is reusable across agents and an agent is reusable
across kinds of work. That is a TrogonAI ownership decision, not an industry
invariant.

- `AgentConfiguration.variables_schema` declares the names, types, and requiredness
  of session-start values referenced by the configuration. Variable values
  belong to the session.
- `ToolDefinition.input_schema` declares the input accepted by that version
  of a reusable tool.
- `WorkContract.input_schema` and `WorkContract.result_schema` declare the
  structured input and successful result for one kind of work.
- An AgentImplementation loop-state or checkpoint schema belongs to the exact
  implementation version and the session contract. Jido's state schema is
  this kind of contract, not evidence for a generic Agent schema.
- Rubrics and quality scores remain in EvaluationBinding and
  OutcomeRecorded. Shape validity is not outcome quality.

Use explicit names and versioned references or digests. Prefer
`result_schema` to an overloaded `output_schema`. Generic conversational
sessions may omit work input and result schemas and use text input and result.

Validate variable bindings and work input before creating the session.
Validate tool arguments at the Tool boundary. Validate the final result
before recording successful completion.

### 5. Keep external stances and observations beside the agent

The following data never enters an AgentConfiguration:

- grants, access shares, credential bindings, secret references, and secret
  material, consistent with
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md);
- budget, token, turn, concurrency, and delegation ceilings;
- rubrics, evaluation bindings, verdict policy, and outcome scores;
- schedules, triggers, channel bindings, delivery destinations, and routing
  policy;
- ownership policy and authorization principal kinds;
- inbound identity: who may invoke the agent is authentication and policy,
  never configuration;
- behavior-neutral hosting configuration: processes, containers, remote hosts,
  placement, isolation, filesystem, network, environment variables, hosting
  authentication, and lifecycle belong to Session launch or deployment;
- success rates, usage counts, cost, priority, and fitness projections;
- environment contents, workspace contents, memory records, transcripts,
  implementation checkpoints, and hosting state; and
- work payloads, WorkContracts, and tool invocation schemas.

An AgentConfiguration declares what it needs. External planes decide what it may
use, what work it receives, how it is judged, and what happened. Changes on
those planes do not mint agent revisions. If observed evidence justifies a
behavior change, a curator turns that evidence into a Proposal through the
[ADR#0024](./0024-agent-platform-stream-topology.md) workflow.

Definitions pin; authorization never pins. Grants and revocations are
evaluated live at each protected action, and credential rotation remains a
live security operation rather than a behavior revision. Credential values
are never model context: a dependency declaration may say that a tool is
needed, while grants, credential bindings, and secret resolution authorize
and equip the tool call outside the prompt.

## Consequences

- The definition contract stays small enough to hand to another engineer:
  four records, one membership test, and one revision-boundary table.
- The agent platform needs typed contracts for Agent, AgentConfiguration,
  AgentRevision, Proposal, the exact AgentImplementation binding, exact model
  selections, and `variables_schema`. A partial charter plus an opaque
  content digest is not a complete definition contract.
- The definition records alone answer which agent existed, which revision
  ran, and which proposal justified it. What the model saw and what was
  authorized are answered by the session and policy domains through the
  references this boundary requires them to record.
- Session, memory, and work contracts are deliberately not designed here.
  Each gets its own decision when its domain is built, and none of those
  decisions may move data across the ownership boundary fixed here.
- Registry and proposal events may remain small. Large prompt and skill
  content can live outside the event stream when canonical serialization,
  immutable retrieval, and digest verification are defined.
- Proposal change class and changed fields must come from the typed canonical
  revision difference. They cannot depend only on duplicate metadata supplied
  independently of the candidate artifact.
- Activation rejects a proposal based on a revision that is no longer the
  latest.
  Rebase changes the candidate and therefore requires a new digest and
  verification.
- Tool and work schemas evolve once at their owning resource. Agents and
  sessions pin versioned references instead of copying those schemas and
  allowing them to drift.
- Memory, policy, and evaluation can evolve without polluting revision
  history. This adds joins to read models, but each resource or plane now
  enforces one owner's invariants and changes at its natural cadence.
- Treating variable-schema changes as charter-class slows autonomous changes
  to that interface, but prevents the evolution loop from silently breaking
  callers.
- Exact prompt ordering, provider-specific message roles, compression
  algorithms, and wrapper cadence remain decisions of the implementation
  adapter pinned by AgentConfiguration. This ADR fixes ownership and binding
  time rather than one universal prompt format.
