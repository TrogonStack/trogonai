---
number: "0024"
slug: agent-platform-stream-topology
status: accepted
date: 2026-07-13
---

# ADR 0024: Agent Platform Stream Topology

## Context

The agent platform manages self-evolving agents: an agent is a named,
versioned declaration (a charter plus a learned layer) that a runtime
instantiates into sessions pinned to a specific revision. The evolution loop
is the product: sessions produce outcomes, curators distill outcomes into
proposed revisions, fresh-context verifiers judge proposals (the proposer
never approves its own change), passing proposals activate, and rollback is
one pointer move. The design comes from a study of how the industry's agent products define,
version, and evolve agents. The full corpus ships alongside this ADR: one
dossier per product, the cross-product synthesis, and the running decision
record, under `docs/research/agent-platform/`. Three merged
straw-hat-team ADRs settled adjacent questions: hierarchy and placement
(4761776210), the bare `parent` field (6310044131), and annotations
(5177934677).

The findings from that study that this decision rests on:

- **Revisions are immutable, numbered artifacts, and activation is a
  separate step.** Mature platforms mint a numbered revision on every
  change, stage it inactive, and reject concurrent writes optimistically.
  Nobody edits a live definition in place.
- **The proposer never approves its own change.** Production experience
  across the field: reviewers with fresh context outperform reviewers who
  share the author's context, so verification is a separate actor judging
  a staged change.
- **Proposal volume is high by design.** Self-improving agents run
  background curation that continuously proposes memory and skill changes;
  a platform multiplies that across every agent, every night, and most
  candidates are supposed to be rejected.
- **Sessions pin the revision they started on.** Only new sessions pick up
  an activation, and rollback is a pointer move.
- **The record and the change-in-flight are different resources
  everywhere we looked.** The pull request is the canonical shape: a
  repository's history holds merges, while proposals and reviews live on
  the PR, a separate resource with its own lifecycle. Our first cut
  collapsed that distinction.

The decision path, in order: the study produced a working definition (an
agent is a named, versioned declaration a runtime instantiates into pinned
sessions) and an evolution loop built on staged, verified revisions.
Prototyping that loop surfaced two layering rules, recorded in the decision
record: deciders know principal identity but never principal kind
(authentication owns that ontology; ADR 0017), and wire schemas avoid
booleans because their false default is fail-open. Reviewing the prototype
raised the question this ADR answers: why do a proposal and its verdict
live in the registry record's stream?

The obvious first topology, and the one the prototype used, puts six
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
- Rollback targets only previously activated revisions.
- The activation event references the proposal it lands (id and content
  digest). The dispatcher proves approval upstream; a recorded verdict is
  immutable, so passing it as data carries no race.

**A workflow stream per proposal**, holding one change-in-flight from birth
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
  as data: the change class (learned-layer versus charter), the content
  digest, evidence, the author. Proposals are identified by their own id,
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
                         charter: { ... }, owner: "owner@tenant-acme" }
2  RevisionActivated   { revision: 2, proposal: "prop-7f3a",
                         digest: "sha256:11..." }
```

Proposal stream for `prop-b804`, withdrawn by its author:

```text
1  ProposalOpened      { agent: "agent-pr-reviewer", changeClass:
                         "learned-layer", digest: "sha256:0a...",
                         author: "curator@tenant-acme", evidence: [ ... ] }
2  ProposalWithdrawn   { by: "curator@tenant-acme" }
```

Proposal stream for `prop-9c21`, judged and rejected:

```text
1  ProposalOpened      { agent: "agent-pr-reviewer", changeClass:
                         "learned-layer", digest: "sha256:f4...",
                         author: "curator@tenant-acme", evidence: [ ... ] }
2  ProposalVerdictRecorded { verdict: "rejected",
                             verifier: "verifier@tenant-acme",
                             reasons: [ ... ] }
```

Proposal stream for `prop-7f3a`, which supersedes the withdrawn one and
becomes revision 2:

```text
1  ProposalOpened      { agent: "agent-pr-reviewer", changeClass:
                         "learned-layer", digest: "sha256:11...",
                         author: "curator@tenant-acme",
                         supersedes: "prop-b804", evidence: [ ... ] }
2  ProposalVerdictRecorded { verdict: "approved",
                             verifier: "verifier@tenant-acme" }
```

What the example shows:

- Revision 2 exists nowhere until the registry stream mints it; every
  proposal is identified by its own id before that moment.
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

**App-level gates, unchanged:** principal kind (human versus machine) is
authentication context (aauth, ADR 0017) enforced at command dispatch.
Charter-class activation requiring a human and the human-only policy
mutations remain dispatch preconditions; deciders treat principals as opaque
identities.

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
  without ever touching the agent record.
- A proposal does not receive a revision number when opened. Anything
  referencing the unactivated change uses the proposal id, and the revision
  number appears only when the agent stream mints it at activation.
