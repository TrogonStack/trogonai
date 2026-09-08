# Session Detached Work

An agent starts a development server. The tool returns a URL, the turn ends, and
the process keeps running and keeps producing output for hours. This page
documents how the session records that the turn was released while the work
continued, and why almost none of that needed new machinery. It documents the
protobuf contract that exists today. There is no Rust implementation yet.

See [Session Aggregate](./session-aggregate.md) for the operation ledger and
[Session Doctor](./session-doctor.md) for what happens to work nobody is
supervising any more.

## What was already there

Detached work looks like it needs a whole subsystem, and it mostly does not,
because the operation ledger was built for exactly this shape. Work whose outcome
arrives later than the call that started it already has:

- a durable identity, from `OperationReserved`
- a cancellation channel, from `OperationCancellationRequested`
- an outcome that arrives whenever it arrives, from `OperationOutcomeRecorded`
- a way to say the outcome is not knowable, from `OperationUnknown`
- somewhere to put the output, in `CommandOutputReplayRef`
- a reader-visible flag that the session is not finished settling, in
  `SessionView.has_unreconciled_work`

Adding a compound background-turn event on top of that would have restated all of
it in a second vocabulary, and the two would drift.

## What was actually missing

Three things, and they are small.

The first is that the tool call lied. `ToolCallCompleted` means the call
finished, and a reader folds it as done. When the call returned a handle rather
than an outcome, that fold is wrong in a way nothing on the event corrects.
`DetachedWork` on `ToolCallCompleted` says the completion is a handoff: the
result is a handle, and the outcome is in the ledger under `operation_id`.

The second and third are policies nobody can infer afterwards: what happens to
this work when the session ends, and how long it may go unwatched before someone
should go looking for it.

## Cancel is the safe default

`TerminalOwnership` decides what becomes of the work when the session reaches a
terminal state, and its zero value cancels.

The two failure modes are not symmetric. Cancelling work that should have
survived is loud: something the user wanted running stopped, and they find out
within minutes. Leaving work running that should have stopped is silent, and it
is precisely the orphan problem from
[Session Doctor](./session-doctor.md#orphans-and-why-finding-one-is-not-enough-to-delete-it)
with a process attached instead of a byte range. A default that leans toward the
silent failure is a default that accumulates.

`TERMINAL_OWNERSHIP_SURVIVE_TERMINAL` is the deliberate opt-in. A session that
ends with surviving detached work is terminal and not complete, which is the
distinction `has_unreconciled_work` exists to carry.

## The lease policy is an event; the heartbeat is not

This is the one call worth arguing for, because the obvious design is wrong.

Supervision looks like something to record. A runner holds the work, renews a
lease every thirty seconds, and if it stops renewing the work is abandoned. It is
tempting to append each renewal, so the history shows exactly when supervision
was lost.

That is an append per interval into a log that is never truncated
([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 7), and
long-lived detached work generates the most of them. A server running for eight
hours would contribute nearly a thousand events that no projection folds and no
reader wants, permanently. This is the same fault the analysis flags for a
downstream consumer writing its checkpoint into session streams: liveness is
live state, and live state does not belong in history that outlives it.

So `SupervisionPolicy` records `max_unsupervised` and the `runner_id` that held
the work, and nothing else. The live lease sits in the runner registry a
projection reads. What the log keeps is the number a reconciler needs *after* the
fact: given how long the work was allowed to go unwatched and when it was last
seen, whether it is lost is a question anyone can answer later without having
been watching.

That is what makes an expired lease a doctor finding rather than a lost fact.
`ORPHAN_CLASS_EXPIRED_RECONCILIATION_LEASE` is reachable because the policy is on
the log even though the heartbeat never was.

## The endpoint has the same rule as every other external URL

`DetachedWork.endpoint` is where the caller reaches what was started: a URL, a
socket path, a container id. It must be credential-free, which is the prohibition
`ExternalArtifact.source_url` already carries. A detached development server's URL
is, empirically, the field most likely to arrive with a session token stapled to
the query string, into a log that is never truncated.

## Layout

`proto/trogonai/session/sessions/v1alpha1/detached_work.proto`

| Type | What it settles |
| --- | --- |
| `DetachedWork` | the completion is a handle, and where the real outcome lives |
| `TerminalOwnership` | what happens to the work when the session ends |
| `SupervisionPolicy` | how a lost runner becomes detectable, without a heartbeat in the log |

It attaches at `ToolCallCompleted.detached` and `CompleteToolCall.detached`.

## Status

Shipped: `detached_work.proto`, lint-clean, formatted, building, and generating
Rust bindings reachable at `trogonai_proto::session::sessions::v1alpha1`.

Not shipped: the runner registry that holds live leases, the reconciler that
turns an expired one into a finding, and any enforcement of
`TERMINAL_OWNERSHIP_CANCEL_ON_TERMINAL` at session close. The contract names what
must be true; nothing yet makes it true.
