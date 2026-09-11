# Contract impact: OpenAI Agents API

Part of Agent Definition Research. Written 2026-09-11 against the
[Agents API dossier](./products/openai-agents-api.md).

This document reads the Agents API back against our shipped wire contracts
and names where they differ. It is **analysis, not a decision**. Every
delta below that touches `proto/` is gated on an ADR, because
`proto/trogonai/session/sessions/v1alpha1` is governed by its own package
preamble and by
[ADR#0031](../../adr/0031-agent-implementation-and-session-plan.md),
[ADR#0035](../../adr/0035-session-store-decider-aggregate.md),
and [ADR#0040](../../adr/0040-contract-field-vocabulary.md).
Where a conclusion here differs from an accepted record in the
[ADR index](../../adr/index.md), the ADR is authoritative.

Contracts examined: `proto/trogonai/agents/agents/v1/` (6 files) and
`proto/trogonai/session/sessions/v1alpha1/` (109 files).

## The deadline that governs all of this

`session/sessions/v1alpha1/events.proto` states the window for adding
required fields:

> Within v1alpha1 a field may still be added as LEGACY_REQUIRED. That window
> is open only while both conditions hold, no deployed producer has written
> these events, and this package has not promoted, and it closes at
> whichever comes first.

Several deltas below want new required fields. They are nearly free today
and cost a package version once a producer ships. **That timing, not the
Agents API itself, is the reason to look at them now.** The same preamble
warns that `buf breaking` under `WIRE_JSON` will not catch a missed one.

## Where we are already stronger (record, do not change)

Worth writing down first, because the pull when reading a major vendor
release is to converge on it, and on seven axes their public contract is
weaker than what we already ship. Their disclaimers are useful evidence that
the problems we modelled are real.

| Concern | Agents API | Ours |
| --- | --- | --- |
| Usage accounting | "best-effort... may change as accounting arrives... not a final bill", nullable | `TokenUsage.completeness` (FINAL/PARTIAL, "never summed with one") plus `Cost.amount_micros` and `rate_ref` |
| Output truncation | "Command-output truncation is not reported" | `TextToolResult.truncated`, `ArtifactRef.truncated`, `ArtifactRef.untruncated_size_bytes` |
| Compaction provenance | automatic and opaque | `Compacted.covered_input_digest`, `CompactionProducer`, `CompactionContextRoot`, `CompactionTrigger` |
| Child lifetime on parent death | undocumented | `CascadePolicy`, explicitly "closes the industry-wide silent-orphan gap" |
| Work outliving its turn | not modelled | `DetachedWork`, `TerminalOwnership`, `SupervisionPolicy` |
| Namespace exposure | not modelled | `ResourceAccessRecord` and `ResourceAction` |
| Replay | "Streams do not replay missed events", recovery is a five-step client procedure | durable event log, `SessionOrdinal` stable across restore and relocation |

None of these need changing. They are the reason to treat the Agents API as
a source of *noun decomposition* rather than of contract detail.

## D1. We have no environment noun, and they made it the interesting one

**Their model.** Three lifetimes, decoupled and documented: agent
(settings), session (durable), environment (disposable compute with its own
id, its own status, and its own event family). "An agent session can outlive
its environment." "Deleting a session neither stops its environment nor
emits a deletion webhook." "Every session receives a different environment
ID and needs a separate executor."

**Ours.** There is no environment. Two things stand partly in for it:

- `WorkspaceRef` (`workspace.proto`), which is *where files are*, immutable
  for the session's life, carrying `workspace_id`, `uri`, `revision`.
- `ExecutionAttemptStarted.isolation_placement` and `.host_artifact_ref`
  (opaque strings), which are *where this attempt ran*.

Neither is a compute identity that survives across attempts. In our model a
restart mints a new attempt and "reconnect to the same environment" cannot
be said. `grep -rin "sandbox\|environment" proto/trogonai/session/sessions/v1alpha1/*.proto`
returns nothing.

**The gap that matters.** Not the noun, the *blocked state*. Their session
can sit in `requires_action` with `required_action.type:
"environment_connection"` for up to five minutes waiting for the customer to
bring compute. We have `ExecutionAttemptStarted` and
`ExecutionAttemptReady`, both positive facts. A session blocked on absent
customer compute produces no event at all, so it is invisible to a
projection and to a reconciler.

**Recommendation.** Do not add an `Environment` message to v1alpha1 yet. The
prior question is an ADR question: is compute a facet of the session
aggregate or its own aggregate with its own stream?
[ADR#0031](../../adr/0031-agent-implementation-and-session-plan.md) §3,
"Keep every implementation attached to its platform Session", biases toward
facet, but a
reconnectable environment shared across attempts is exactly the shape that
argues for its own stream. What the ADR should settle:

1. environment identity stable across attempts, versus attempt-scoped
   placement (what we have);
2. whether "blocked awaiting external compute" is a recorded fact;
3. whether session-terminal cascades to compute, and in which direction
   (their answer is neither direction, stated as a surprise).

If and only if that ADR lands: `ExecutionAttemptStarted` gains an
`environment_id`, and it wants to be required, which puts it inside the
window above.

## D2. `turn_id` is stamped on conversation events and missing from every event a turn is billed and audited by

**Verified gap.** Of the 43 arms of the `SessionEvent` union, 12 carry
`turn_id` and 31 do not. Most of the 31 are correctly turnless: session
lifecycle, the `Parent*` lineage family, and `ExecutionAttempt*`, which spans
turns by construction. The ones that are *not* correctly turnless are:

`OperationReserved`, `OperationOutcomeRecorded`,
`OperationCancellationRequested`, `DelegationDispatched`,
`ExternalDelegationDispatched`, `ArtifactRecorded`, `TodoUpdated`,
`SystemNoticeRecorded`.

`Compacted` and `CheckpointProduced` are excluded from that set on purpose:
both already carry `SessionOrdinal` ranges (`covers_from`, `covers_through`),
which is the correct shape for a fact that spans turns, and an id would be
the wrong one. `ResourceObservation` is excluded because it is not an event
at all; it is a value message inside `ToolCallCompleted.observed`, and
`ToolCallCompleted` already carries `turn_id`.

**Why that is the wrong set to omit.** `user_message_recorded.proto`
justifies stamping precisely so that "what happened in this turn" is "a
filter over a decoded field rather than a reconstruction that walks message
and tool-call joins in fold order". That rationale applies with more force,
not less, to the omitted events: they are what a turn *cost* (operation
ledger), what it *delegated* (both delegation events), what it *produced*
(artifacts), and what it *exposed* (resource observations). A reader today
can attribute a file change to a turn but not the artifact recorded from it,
which is an inconsistency inside one aggregate rather than a difference with
OpenAI.

The Agents API makes exactly these the first-class turn queries:
`turns.list`, `turn.usage`, `turn.subagent_id`, and "filter session items by
`turn_id`". That is corroboration, not the reason.

**Recommendation.** Add `turn_id` to the eight events named above, required
on seven and optional on `SystemNoticeRecorded` (whose existing
`tool_call_id` is optional for the same reason: a notice can be emitted out
of band). Highest value and lowest risk delta in this document, and the one
most exposed to the window.

The repo's own convention settles the one question this raises. The
operation-ledger outcome events carry `operation_id` and could reach a turn
by a single join to `OperationReserved`, so stamping them is denormalization.
But `ToolCallCompleted` already carries `turn_id` despite being able to join
to `ToolCallRequested` on `tool_call_id`, so stamp-over-join is the
established choice and consistency favors stamping all three. One semantic
must be written into the doc comment: on `OperationOutcomeRecorded`,
`turn_id` is the turn that *reserved* the operation, not the turn the
outcome landed in, because `DetachedWork` exists precisely so work can
outlive its turn.

**Separately, verify one existing inconsistency.** `turn_id` is optional on
`ApproveToolCall`, `ToolCallApproved`, `DenyToolCall`, and `ToolCallDenied`
while required on the other 20. A human approving out of band is a good
reason for that, and if it is the reason it belongs in the doc comment,
since nothing currently states it.

## D3. Steering has no representation, and one doc comment forecloses it

**Their model.** One input channel, discriminated by session state: "A
message sent to an idle session starts a new turn. A message sent during an
active turn steers that turn." The announcement gives the motivating case, a
user saying "no, use TypeScript instead" without cancelling.

**Ours.** `UserMessageRecorded.turn_id` is documented as "The turn this
message opens", and a turn is "one user-prompt-to-final-assistant-message
cycle". A mid-turn steer fits neither reading: it does not open a turn, and
it breaks the one-prompt-per-turn definition.

**Recommendation.** This is a contract ambiguity we own regardless of
OpenAI, and it will surface the first time we ship an interactive surface.
Either record the disposition explicitly (an enum arm on
`UserMessageRecorded` distinguishing a turn-opening message from a steer of
the active turn) or amend the definition of a turn and the doc comment to
admit multiple user messages. Do not leave it to the implementation to pick.

## D4. Their subagent is an in-session actor; our only delegation primitive is a new session

**Their model.** A subagent has its own context and its own item history,
`subagent_id` on turns, and nothing else of its own: "The coordinator and
subagents share its filesystem. Creating a subagent does not create another
environment." Function tools are unavailable to subagents.

**Ours.** `DelegationDispatched` plus `ParentLinked` mint a child *session*,
with its own stream, its own `StoredSessionExecutionPlan`, and its own
`WorkspaceRef`.

**The gap.** Our `WorkspaceRef` can carry the same `workspace_id` and `uri`
on parent and child, but nothing in the contract distinguishes "same repo,
separate checkout" from "the same live filesystem, concurrently written by
both". Those have opposite safety properties, and OpenAI's own guidance
("Agents that edit the same files must coordinate their changes") is the
evidence that the shared case is the dangerous one.

**Recommendation.** Two questions for an ADR, in order:

1. Is shared-live-filesystem delegation a shape we support? If yes, it needs
   a discriminator, on `WorkspaceRef` or on `DelegationDispatched`, and
   `CreateChildSession` must reject the ambiguous case rather than let two
   sessions silently share a working tree.
2. Do we need an in-session actor dimension at all, or is child-session our
   answer to subagents? Their split exists because their session is the unit
   of managed state and they did not want N of them. Ours is event-sourced
   and cheap, so the same pressure may not apply. Answering "child session
   is our subagent" is a legitimate outcome, but it should be written down,
   because `subagent_id` on a turn is the attribution query their
   observability guide leads with and ours would answer by stream lineage
   instead.

Worth noting in the other direction: `CascadePolicy` already answers the
orphan question their docs do not address at all.

## D5. `OperationKind` has no arm for provisioning

`operation_reserved.proto` enumerates `TOOL`,
`CHILD_SESSION_DELEGATION`, `EXTERNAL_DELEGATION`. Provisioning compute is a
retriable, dedupe-needing side effect, which is the ledger's stated purpose,
and their lifecycle guide warns about exactly the failure the ledger
prevents: "Repeated or concurrent requests must not create duplicate
environments."

**Recommendation.** Gated on D1. If environments become real, add
`OPERATION_KIND_ENVIRONMENT_PROVISION`. Adding an enum value is wire-safe
and is not subject to the required-field window.

## D6. `AgentConfiguration` holds up, with one uncovered case

The Agents API maps onto `AgentConfiguration{runtime, Any settings}` without
strain: `runtime` names the Agents API, `settings` carries `model`,
`instructions`, `tools`, `multi_agent`, `reasoning`. This is an independent
confirmation of the central claim in
[ADR#0043](../../adr/0043-agent-instructions-ownership-and-shape.md),
"Even the content half of that contract is runtime vocabulary": OpenAI's
`instructions` is a bare string,
the Claude Agent SDK's is a four-way union, and both fit under `settings`
with no platform-level field left empty. A third sample point that would
have broken a mirrored platform field instead cost nothing.

We are also ahead on versioning. Their saved agent has update and delete and
no revision, no pinning, and no statement of what an update means to a
session already created from it beyond "Changes to a stored agent apply to
new sessions". `AgentProvisioned` mints numbered revisions with a
`content_digest`.

**The uncovered case.** Their harness versions *underneath* the customer:
"The Agents API provides versioned access to these capabilities with each
model launch. We maintain and continuously improve the harness alongside
your models." Under our model, a vendor-side harness upgrade changes an
agent's behavior while `runtime`, `settings`, `revision`, and
`content_digest` all stay byte-identical. The revision model in
[ADR#0025](../../adr/0025-agent-definition-data-ownership.md)
assumes configuration change is the only behavior change, and
[ADR#0024](../../adr/0024-agent-platform-stream-topology.md)
pins a revision per session on that assumption.

This lands directly on two stated goals of
[ADR#0025](../../adr/0025-agent-definition-data-ownership.md):
"Revisions change only when agent behavior changes" and "Every execution is
traceable to the
exact revision it ran, by reference and digest, so verification results
remain meaningful." A vendor-side harness upgrade changes behavior without
changing a revision, and a verification result recorded against revision N
stops meaning what it meant. The rule in
[ADR#0024](../../adr/0024-agent-platform-stream-topology.md),
"Sessions pin the revision they started on", pins a number that no longer
pins behavior.

This is not a proto field. It is a question
[ADR#0025](../../adr/0025-agent-definition-data-ownership.md)
does not currently answer: **what does a revision mean when the runtime's
own behavior is
versioned by a third party?** Options range from recording an observed
runtime build on the session's execution plan, to declaring runtime drift
out of scope and saying so explicitly. Worth raising as a decision-record
entry (Q27-shaped) rather than quietly assuming the digest still means what
it says.

## D7. Two smaller observations, no action proposed

- **MCP connection origin.** They treat *who dials the server* as a
  first-class axis (`connection_origin: service` versus `environment`,
  plus `stdio` inside the sandbox) because it determines both reachability
  and which secret store applies, with vaults working only for
  service-origin connections. Before the canonical MCP bodies of
  [ADR#0041](../../adr/0041-canonical-mcp-jsonrpc-bodies-over-nats.md)
  meet a BYO-compute story, we should confirm we can express that
  distinction; we
  are not obviously missing a field, but nothing today records it.
- **Residency.** "The Agents API currently supports data residency only in
  the United States and does not support Zero Data Retention (ZDR).
  Choosing a self-hosted sandbox does not make the Agents API ZDR-eligible."
  A customer-visible constraint of this runtime, inherited by anyone who
  selects it in `AgentConfiguration.runtime`, and worth surfacing at
  selection time rather than discovering at procurement. No contract
  change.

## Suggested sequencing

1. **D2 now**, as its own ADR or as an amendment, because the required-field
   window closes on first producer and the gap is internally inconsistent
   independent of OpenAI.
2. **D3 next**, small, and it is our ambiguity rather than an integration
   question.
3. **D1 and D5 together**, one ADR, only after deciding facet versus
   aggregate. This is the largest piece of work here.
4. **D4** as a decision-record entry feeding an ADR, since the honest answer
   may be "child session is our subagent, written down".
5. **D6** as a decision-record entry against
   [ADR#0025](../../adr/0025-agent-definition-data-ownership.md).
   No proto change.
