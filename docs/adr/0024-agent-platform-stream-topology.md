---
number: "0024"
slug: agent-platform-stream-topology
status: accepted
date: 2026-07-13
---

# ADR 0024: Agent Platform Stream Topology

## Context

The agent platform manages self-evolving agents: an agent is a named,
versioned behavior declaration, with each version captured as one complete
immutable revision. A change is classified as charter-class or learned-layer.
A runtime instantiates the agent into sessions pinned to a specific revision.
The evolution loop is the product: sessions produce outcomes, curators distill
outcomes into behavior-change proposals, fresh-context verifiers judge
proposals (the proposer never approves its own change), approved proposals
activate into revisions, and rollback is one pointer move. The design comes
from a study of how the industry's agent products define, version, and evolve
agents. The full corpus ships alongside this ADR: one dossier per product,
the cross-product synthesis, and the running decision record, under
`docs/research/agent-platform/`. Three merged straw-hat-team ADRs settled
adjacent questions:
[hierarchy and placement (ADR 4761776210)](https://straw-hat-team.github.io/adr/adrs/4761776210/README),
[the bare `parent` field (ADR 6310044131)](https://straw-hat-team.github.io/adr/adrs/6310044131/README), and
[annotations (ADR 5177934677)](https://straw-hat-team.github.io/adr/adrs/5177934677/README).

The findings from that study that this decision rests on:

- **Revisions are immutable, numbered artifacts, and activation is a
  separate step.** Mature platforms keep activated definitions as numbered
  artifacts, separate candidate review from activation, and reject
  concurrent writes optimistically. Nobody edits a live definition in place.
- **The proposer never approves its own change.** Production experience
  across the field: reviewers with fresh context outperform reviewers who
  share the author's context, so verification is a separate actor judging
  a candidate change.
- **Proposal volume is high by design.** Self-improving agents run
  background curation that continuously proposes instruction, skill, and
  dependency changes; a platform multiplies that across every agent, every
  night, and most candidates are supposed to be rejected.
- **Sessions pin the revision they started on.** Only new sessions pick up
  an activation, and rollback is a pointer move.
- **The record and the change-in-flight are different resources
  everywhere we looked.** The pull request is the canonical shape: a
  repository's history holds merges, while proposals and reviews live on
  the PR, a separate resource with its own lifecycle. A single-stream
  design collapses that distinction.

The decision path, in order: the study produced a working definition (an
agent is a named, versioned declaration a runtime instantiates into pinned
sessions) and an evolution loop built on proposals, verification, and
activation.
Designing that loop surfaced two layering rules, recorded in the decision
record: deciders know principal identity but never principal kind
(authentication owns that ontology;
[ADR 0017](./0017-aauth-agent-authentication.md)), and wire schemas avoid
booleans because their false default is fail-open. Reviewing that draft
raised the question this ADR answers: why do a proposal and its verdict
live in the registry record's stream?

The first candidate topology puts six
commands and six events in one Agent stream: AgentProvisioned,
RevisionStaged, RevisionVerdictRecorded, RevisionActivated,
RevisionRolledBack, AgentArchived. Reviewing it found a coupling:
RevisionStaged plus RevisionVerdictRecorded form a proposal workflow (the
shape of a pull request) fused into the registry record (the shape of a
repository). Three consequences follow:

1. **The stream grows with failure volume, not meaningful history.**
   Proposal churn is the point of the platform: curators stage revisions
   nightly and most proposals are supposed to die at verification. In the
   single-stream shape every dead experiment is a permanent event in the
   stream that every session pin, activation, and replay depends on.
2. **Staging claims the next revision number**, forcing concurrent curators
   to serialize on optimistic concurrency before anyone knows whether their
   proposal survives judgment.
3. **The transactional argument for same-stream verdicts is weak.** A
   recorded verdict is immutable: it never un-happens. Immutable facts are
   race-free to pass as command input, which the platform's purity rule
   (Q26) already prescribes for every fact proven outside the stream:
   deciders consume stream facts and opaque inputs; ontologies from other
   planes arrive as data. Revocable facts (grants) are the dangerous kind,
   and even those arrive as data with live evaluation.

Related but distinct: the decider runtime serializes one fold-state per
command, because deciders compile to WASM components and their state
crosses the host boundary. Those per-command states are projections of a
stream, not streams, and this ADR does not change that pattern.

## Decision

Split the agent record's mutations into two kinds of stream and adopt one
placement rule. This ADR fixes the topology, the invariants each stream
enforces, and the naming intent. Concrete message, package, and field names
below are illustrative; the wire contracts own the final spelling, and
renames do not reopen this decision.

Here, stream means one logical ordered event history. This ADR does not
choose physical JetStream resources, subject partitioning, retention, or
projection checkpointing. Those deployment choices require a separate
decision before infrastructure provisioning.

**The placement rule:** a fact belongs in a stream when its order relative
to that stream's other events is load-bearing for an invariant the stream
must enforce. A fact whose latest value is all that matters lives aside and
arrives as proven command input.

**A registry stream per agent**, holding the record's lifecycle: the agent
comes into existence, an approved proposal becomes the next active
revision, the active pointer rolls back, the agent is archived (for
example: AgentProvisioned, RevisionActivated, RevisionRolledBack,
AgentArchived). Invariants this stream enforces:

- Provisioning mints revision 1 as the initial active revision. Later revision
  numbers are minted linearly at activation and nowhere else.
- Nothing activates on an archived agent, and nothing activates twice.
- Rollback targets only revisions that were previously active, including
  revision 1 made active by provisioning.
- The activation command carries the exact approved proposal, pinned base
  revision and digest, and candidate reference and digest. The registry
  rejects a base that is no longer current. The activation event preserves
  the proposal id, candidate reference, and candidate digest. The dispatcher
  proves approval upstream; a recorded verdict is immutable, so passing it as
  data carries no race.

**A proposal stream per proposal**, holding one change-in-flight from birth
to terminal state: opened, then judged or withdrawn (for example:
ProposalOpened, ProposalVerdictRecorded, ProposalWithdrawn). Naming intent:
name the entity for what it is, a proposal, never for what it may become, a
revision. Most proposals are expected to die unjudged or rejected, and a
proposal receives a revision number only when the registry stream mints one
at activation. Invariants this stream enforces:

- The proposer is not the approver; both facts are local to the stream.
- At most one verdict; a withdrawn proposal takes no verdict, and a judged
  proposal cannot be withdrawn.
- Only the author withdraws, enforced by the same opaque identity equality
  that enforces proposer versus approver.
- Withdrawal is the terminal for "never judged"; without it, stale
  proposals from continuous curation stay open forever and waste verifier
  work. Superseding a proposal is metadata plus withdrawal: the new
  proposal references what it replaces, and the author withdraws the old
  one. One stream never closes another.
- The opening event carries whatever the judgment and activation gates need
  as data: the pinned base revision and digest, immutable candidate reference
  and digest, typed difference, derived change class (learned-layer versus
  charter), evidence, and author. Proposals are identified by their own id,
  never by a claimed revision number.

**A fact is recorded once, in the stream whose invariants need its order.**
Activation lands only in the registry stream; the proposal stream gets no
closing marker, because no proposal invariant depends on activation
ordering, and a second write of the same fact across two streams diverges
the day one write fails. Projections join the streams to answer questions
that span them, such as what became of a proposal.

### Worked example (illustrative, like every name in this ADR)

One agent and three proposals from one night of curation: one withdrawn,
one rejected, one approved and activated.

Registry stream for `agent-pr-reviewer`:

```text
1  AgentProvisioned    { agent: "agent-pr-reviewer", revision: 1,
                         artifact: "artifact-revision-1",
                         digest: "sha256:01...", owner: "principal-owner" }
2  RevisionActivated   { revision: 2, previous: 1,
                         proposal: "prop-7f3a",
                         candidate: "artifact-prop-7f3a",
                         digest: "sha256:11..." }
```

Proposal stream for `prop-b804`, withdrawn by its author:

```text
1  ProposalOpened      { agent: "agent-pr-reviewer",
                         base: "rev-1@sha256:01...",
                         candidate: "artifact-prop-b804", changeClass:
                         "learned-layer", digest: "sha256:0a...",
                         difference: [ ... ],
                         author: "principal-curator", evidence: [ ... ] }
2  ProposalWithdrawn   { by: "principal-curator" }
```

Proposal stream for `prop-9c21`, judged and rejected:

```text
1  ProposalOpened      { agent: "agent-pr-reviewer",
                         base: "rev-1@sha256:01...",
                         candidate: "artifact-prop-9c21", changeClass:
                         "learned-layer", digest: "sha256:f4...",
                         difference: [ ... ],
                         author: "principal-curator", evidence: [ ... ] }
2  ProposalVerdictRecorded { verdict: "rejected",
                             verifier: "principal-verifier",
                             reasons: [ ... ] }
```

Proposal stream for `prop-7f3a`, which supersedes the withdrawn one and
becomes revision 2:

```text
1  ProposalOpened      { agent: "agent-pr-reviewer",
                         base: "rev-1@sha256:01...",
                         candidate: "artifact-prop-7f3a", changeClass:
                         "learned-layer", digest: "sha256:11...",
                         difference: [ ... ],
                         author: "principal-curator",
                         supersedes: "prop-b804", evidence: [ ... ] }
2  ProposalVerdictRecorded { verdict: "approved",
                             verifier: "principal-verifier" }
```

What the example shows:

- Revision 2 exists nowhere until the registry stream mints it; every
  proposal is identified by its own id before that moment.
- Every proposal pins revision 1 as its base. After one proposal activates,
  another proposal on that base must be rebased and verified again.
- Two of the three proposals never touch the registry stream. Its replay is
  two events, however many candidates died along the way.
- `supersedes` is metadata on the new proposal; the closing of `prop-b804`
  is its own author's withdrawal in its own stream.
- The verdict on `prop-7f3a` is final before activation is dispatched; the
  dispatcher hands the registry stream an immutable, already-proven fact.
- Proposer and verifier differ in every judged stream, and the withdrawal
  matches the opening author: all three checks are opaque identity
  equality, local to each stream.

**What stays aside, unchanged:** hierarchy (placement and moves), grants and
policy, rubrics and evaluation bindings, outcomes, and sessions each keep
their own streams. The judged never owns the yardstick: rubrics and bindings
never enter the agent or proposal streams; only the verdict result lands on
the proposal it judges.

[ADR 0025](./0025-agent-definition-data-ownership.md) defines the complete
AgentRevision, Session, Memory, contract, and external-plane ownership
boundaries that this topology preserves.

**App-level gates, unchanged:** principal kind (human versus machine) is
authentication context (aauth, [ADR 0017](./0017-aauth-agent-authentication.md))
enforced at command dispatch.
Charter-class activation requiring a human and the human-only policy
mutations remain dispatch preconditions; deciders treat principals as opaque
identities. Rollback authorization is also a dispatch concern outside this
topology decision.

## Consequences

- The agent domain implementation ships as two decider families: agent
  stream (provision, activate, rollback, archive) and proposal stream (open,
  record verdict, withdraw). The proto packages split the same way, and each
  decider carries its own serialized fold state.
- Agent stream replay stays proportional to meaningful history (activations
  and lifecycle), not to experimentation volume. Rejected proposals age out
  in their own streams and remain queryable through projections.
- Relative to the single-stream alternative, "no staging on an archived
  agent" is a dispatch-level check rather than stream-enforced. An orphan
  proposal against an archived agent is harmless: activation, which is
  stream-enforced, can never land it.
- Concurrent curators do not contend: proposals open freely and only
  activation serializes, which matches the intent that most proposals die
  without ever touching the agent record. Once one proposal activates, other
  proposals pinned to its former base must be rebased and verified again.
- A proposal does not receive a revision number when opened. Anything
  referencing the unactivated change uses the proposal id, and the revision
  number appears only when the agent stream mints it at activation.
