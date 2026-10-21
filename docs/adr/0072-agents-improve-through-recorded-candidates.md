---
number: "0072"
slug: agents-improve-through-recorded-candidates
status: draft
date: 2026-10-21
---

# ADR#0072: Agents Improve by Running Recorded Candidates, Scored Blind, and Promoted Only Through Proposals

## Context

[ADR#0025](./0025-agent-definition-data-ownership.md) settles that an Agent's
configuration is immutable per revision and that change arrives as a proposal
that mints a new revision. [ADR#0031](./0031-agent-implementation-and-session-plan.md)
adds that a Session runs one immutable plan and that V1 has no Session override
of admitted values. Together they leave an agent that wants to improve with
only one move: propose a change and hope. There is no way to try a change on
the same work, measure it, and carry the measurement into the judgment.

The platform needs that loop, and it needs the loop to stay honest:

- **A trial must be recorded, not improvised.** If the edit that was tried is
  not a durable fact, a score cannot be tied to it and a promotion cannot point
  back at what was measured.
- **The improver must not see the yardstick.** An agent that can read its own
  rubric, its scores, or the identity of the candidate behind a run can
  optimize the measurement instead of the work.
- **A trial must not be able to reach the yardstick over the network, and
  nothing may widen what an agent may reach merely by being declared.**
- **Who may judge a change is a policy question.** The proposal verdict has a
  single immutable decision, and nothing says whether an agent may record it.

The product principle behind every decision here is event sourcing: immutable
facts in events, typed pointers pinned by digest everywhere, and temporality
reconstructed from the event history, never by mutating a record. What was true
when is answered by folding a stream up to a position, so nothing here is an
edited row.

## Decision

### 1. A candidate is its own immutable stream

`agents/candidates` holds one stream per candidate. `RecordCandidate` creates it
under a NoStream precondition and emits `CandidateRecorded`; there is no edit
command. A different edit is a different candidate.

The recorded content is a `Candidate`: a pinned `base_revision_ref`
(`AgentRevisionRef`), an optional `SettingsEdit`, `PlatformEdit`s, and
`StagedSkill`s. Staged skill bytes are not inline: each carries a `ClaimCheck
content_ref`, a locator plus the digest the bytes are verified against. Every
edit must fall inside the `EditableSurface` of the base revision's
implementation, committed by its definition digest, and within its caps; an
edit outside it is refused when the candidate is recorded.

A candidate is never a revision and is never activated. A revision is minted
only by a proposal and its activation.

Everything else points at a candidate through `CandidateRef { candidate_id,
content_digest }`. The digest pins the exact recorded bytes, so a reference can
never name different content than the one that was measured. The agents package
owns the full type. The session package keeps its own `CandidateRef` and
`CandidateId` carrying the same values, because the session package must not
import the agents package.

### 2. Sessions, workflows, and proposals point at the candidate

- The Session execution plan pins `candidate_ref` alongside `agent_revision_ref`
  ([ADR#0031](./0031-agent-implementation-and-session-plan.md) Decision 2). The
  plan is not defined in this change; the contract records only that the plan
  is where the pin lives.
- `SessionStarted`, `ForkSession`, and `SessionForked` carry `candidate_ref` as
  the projection surface, as `checkout_ref` is for the working directory: the
  plan stays authoritative, and absent means the registered revision ran
  unchanged. `FORK_REASON_PROBE` runs a different plan over the same context
  prefix.
- A workflow `AgentStep` takes `candidate_ref` as a `ValueReference` whose
  registered contract is `CandidateRef`, so an upstream step's output can feed
  the next step.
- `ProposalDefinition.origin_candidate_ref` points a promotion back at the
  candidate that was measured. A library skill promotion carries the same
  pointer next to its `ProposalId`.

### 3. Network egress is a policy binding, and absence is deny

`policy/egress_permissions` mirrors `policy/delegation_permissions`: initialize,
set, and clear commands and events, attached to a `PolicySubject`, with permits
accumulating downward through ancestor nodes
([ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
Decision 2). A Session with no permission on its Agent or any ancestor node has
no network egress at all, and a connection declaration grants nothing
([ADR#0063](./0063-agent-connection-declarations.md) Decision 5).

An `EgressDestination` is an exact identity, never a pattern: no wildcard, no
suffix, no range. No declared-connection identifier exists in the agents
contracts today, so the only arm is an exact `EgressHost { host, port }`; an
arm naming a declared connection is an additive change when one exists.
Evaluation storage and rubric storage are never permitted destinations for a
Session under evaluation, whatever the effective set says.

Egress is evaluated live on each attempt, as the authorization row of
[ADR#0031](./0031-agent-implementation-and-session-plan.md) Decision 8 already
says. What was permitted when is the policy stream's history, not a field on
the Session.

### 4. Verdict authority is a policy binding, and absence means a human

`policy/verdict_authorities` mirrors the same layout. Per subject, per
`AgentChangeClass`, a rule names the authority required to record a proposal
verdict: `HUMAN_MEMBER` or `HUMAN_MEMBER_OR_REGISTERED_AGENT`. The nearest
binding on the chain that lists a class decides it, starting at the proposal's
Agent and walking up through its ancestor nodes. No binding means human member
for every class, so the default fails closed.

The charter class may never be relaxed below human member. The policy message
states this as a CEL constraint. The issuer of a verdict is read from the event
envelope ([ADR#0069](./0069-event-envelope-records-issuer-and-times.md)); no
verdict field names its own author.

### 5. Scoring lives in evaluation streams, and rubrics are versioned

- `evaluation/rubrics`: `RegisterRubric` fixes an identity and display name;
  `AddRubricVersion` appends an immutable version whose criteria are a
  `ClaimCheck criteria_ref`, plus an optional `ClaimCheck
  withheld_vocabulary_ref` naming terms that would reveal how a run is scored,
  and rejects a repeated exact version string;
  `RetireRubric` stops new versions and bindings. A pinned reference is
  `RubricVersionRef { rubric_id, version }`.
- `evaluation/bindings`: an `EvaluationBinding` pins a `RubricVersionRef` and is
  immutable once bound. Rebinding is an unbind followed by a new binding under
  a new `EvaluationBindingId`.
- `RecordEvaluationResult` records an `EvaluationResultId`, the binding, an
  `evaluated_ref`, an optional `candidate_ref`, the `Score` inline (the score is
  the fact), an optional `ClaimCheck rationale_ref`, and a content digest. The
  evaluated run is a temporal pointer, `SessionPositionRef { session_id,
  through_ordinal }`: that session's log as of that ordinal.
- `evaluation/comparisons`: `OpenComparison` fixes a base revision and a task by
  `ClaimCheck task_ref`; `EnrollComparisonRun` assigns a `CandidateLabel` to a
  session and optionally a `CandidateRef`; `CloseComparison` ends enrollment. A
  label is assigned once, is immutable, and is unique within its comparison.
  The label-to-session mapping lives only in this stream and is never exposed
  to the improver. `screening_rubric_refs` pins the rubric versions whose
  withheld vocabulary screens the task and every rendered run item: admission
  refuses a matching task, and a matching item renders as
  `ELISION_REASON_WITHHELD_VOCABULARY`, which never says which term or rubric
  matched.
- The session run-history query takes a `ComparisonId` and a `CandidateLabel`,
  defined locally in the queries package so the session package imports no
  evaluation type. It carries no score, rubric, verdict, candidate identity, or
  session id, and the server resolves labels through the comparison stream.

Scores, rubrics, bindings, and comparisons never enter agent, proposal, or
session streams and are never readable through their queries
([ADR#0024](./0024-agent-platform-stream-topology.md),
[ADR#0025](./0025-agent-definition-data-ownership.md)). A verdict cites
`EvaluationResultRef { evaluation_result_id, content_digest }`; the score
itself never leaves the evaluation stream.

### 6. Pointers are typed, and references are value objects

Under [ADR#0068](./0068-reference-fields-say-they-are-references.md), every
identifier in the new packages is a single-field id message, never a bare
string: `CandidateId`, `RubricId`, `EvaluationBindingId`, `EvaluationResultId`,
`ComparisonId`, `CandidateLabel`, `SkillId`, `MemoryId`, `ProposalId`, and
`SessionId`. Out-of-line bytes are a `ClaimCheck` under a `*_ref` name. Existing
`session_id` and `proposal_id` strings on existing messages are not migrated;
only new fields use the new types.

Two existing fields change type in place, keeping their field numbers. These
contracts are excluded from codegen and have no deployed readers, so the break
is taken directly rather than reserving the old numbers:

- `EvaluationResultRef.evaluation_result_id` becomes an `EvaluationResultId`.
- `SkillVersionRef.skill_id` becomes a `SkillId`.

`SkillId` lives in a leaf package that imports nothing from agents or from the
skills module, because the agents `SkillVersionRef` uses it while the skills
module imports agents, and a package used from both directions is an import
cycle.

### 7. Temporality is the event history

Nothing records "current" state on a fact. A candidate, a rubric version, a
binding, a result, and an enrollment are each immutable once recorded; a change
of mind is a new fact. The envelope owns issuer and time
([ADR#0069](./0069-event-envelope-records-issuer-and-times.md)), so none of
these messages carries an actor or a timestamp. A reader reconstructs what was
true at a moment by folding the relevant stream, or a session's log through a
`SessionPositionRef`, up to that point.

### 8. The library records provenance, not copies

Library skills and memories hold content by `ClaimCheck content_ref`. A skill
version event carries the agents `SkillVersionRef` as `version_ref`. Skill
provenance is either a promotion, naming the `ProposalId` that promoted it and,
when the content was measured as a candidate, the `CandidateRef`, or an
authoring Agent. A memory's provenance names the author, the
`AgentRevisionRef produced_by_revision_ref`, and the `ProposalId`.
`SupersedeMemory` carries `memory_id` and `superseded_by_id`.

### 9. Improver and selector instructions are ordinary agent configuration

The agents that propose and select candidates are ordinary Agents. Their
instructions are `PlatformHarnessConfiguration.instructions`, so changing them
is an Agent revision like any other, and they reach the platform only through
`RecordCandidate`, `OpenProposal`, and `RecordProposalVerdict` under policy.
Instruction text adapted from third-party work carries `InstructionSource {
source_uri, spdx_license_id, attribution_notice }` in the same configuration, so
the license obligation is pinned by the same digest as the text it covers.

## Alternatives Rejected

### A Session-scoped override of admitted values

Rejected. An override that exists only inside a Session's plan is not a durable
fact, so a score cannot point at it and a promotion cannot point back to it. It
also contradicts the rule that a Session runs one immutable plan.

### Minting an Agent revision per candidate

Rejected. A revision is the unit that an operator activates and reverts, and
minting one per trial would fill the revision history with configurations
nobody chose and make activation ambiguous. A candidate is a trial; a revision
is a decision.

### Scores on the proposal or session streams

Rejected. A score readable from a stream the improver can query is a yardstick
the improver can optimize. Keeping scores, rubrics, bindings, and the label
mapping in evaluation streams lets the run-history query stay score-free by
construction.

### Network egress open by default

Rejected. A declaration grants nothing, and an open default would let a
candidate under evaluation reach the stores that hold its own rubric and scores.
Absence is deny, as for delegation.

### Letting an agent judge every class of change

Rejected. Absence of a verdict authority binding means a human member, and the
charter class cannot be relaxed, so an Agent cannot approve a change to its own
charter.

## Consequences

- Candidates, rubrics, comparisons, egress permissions, and verdict authorities
  are new streams. Until enforcement lands, each is a reviewable record without
  effect.
- An Agent with no egress permission on its chain has no network egress, and an
  Agent with no verdict authority binding on its chain has its proposals judged
  by a human member.
- Dispatch of `RecordCandidate` must resolve the base revision's editable
  surface and verify every edit against it and its caps.
- Dispatch of `BindRubric` verifies the rubric version exists on a rubric that
  is not retired and that the judge differs from the subject.
- Rendering a run history must resolve the comparison's screening rubric
  versions and elide matching items before they reach the reader, and
  `OpenComparison` dispatch must screen the task.
- Library skill and memory content move out of line. Resolution fetches the
  bytes through the claim check and verifies the digest before use.
- `buf breaking` against `main` reports the type changes on
  `EvaluationResultRef` and `SkillVersionRef`. This is expected while
  `proto/trogonai/agents`, `proto/trogonai/evaluation`, and
  `proto/trogonai/library` remain excluded from codegen in `buf.gen.yaml`; no
  compiled caller observes the break.
- The Session execution plan is defined separately and must pin `candidate_ref`
  alongside `agent_revision_ref`.
- `SessionId` and `ProposalId` coexist with older messages that carry the same
  identifier as a bare string. A later migration can retire the strings.

## References

- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ADR#0063: An Agent Declares the Connections It Needs](./0063-agent-connection-declarations.md)
- [ADR#0068: A Reference Field Says It Is a Reference in Both Its Name and Its Type](./0068-reference-fields-say-they-are-references.md)
- [ADR#0069: Every Event Envelope Records Who Caused It and When Each Step Happened](./0069-event-envelope-records-issuer-and-times.md)
- [ADR#0071: What an Agent May Reach, How Far It May Go, and Whether It May Run at All Are Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
