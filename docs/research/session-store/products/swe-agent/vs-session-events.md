# SWE-agent compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [SWE-agent](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 3/12**, evolution scars 0/3 (the dossier finds exactly one
named schema break in the whole system: `query` replaced an older `message`
field at product version 1.1.0, documented in prose only, with no
`schema_version` field anywhere in `TrajectoryStep`, `HistoryItem`, or the
top-level trajectory dict, `docs/usage/trajectories.md:27-30`; one prose-only
rename is not evolution scarring), operational age 1/3 (the dossier's own open
questions admit no test fixture exercises the torn-write scenario and no issue
history was surveyed for the trajectory format specifically; the format has
shipped long enough to acquire one documented breaking rename, which is weak
but non-zero signal of contact with real use), exposure 1/3 (SWE-agent is an
academic/benchmark harness, not a vendor-shipped product a user resumes work
in; its own docs call the `.traj` file "the main output," never a session a
person returns to, `docs/usage/inspector.md:4`, so the store has essentially
no exposure to the failure modes that matter for resumption: crash-then-
continue, multi-host, upgrade-across-versions, because nothing in the
product's design asks it to survive them), design independence 3/3 (no fork
parent; the dossier finds no evidence the trajectory format was inherited from
another product). This is thin evidence per the Method's rule: **do not read
any SWE-agent recommendation below as an industry norm.** Its value here is
not as a peer store to weigh against ours, but as a clean negative control
for a different question: what does a benchmark harness look like when it
never needs to resume, and which of the properties we build for resumption
turn out to be properties resumption specifically requires, versus properties
any complete transcript would benefit from regardless.

## The one structural difference everything else follows from

**A `.traj` file is an output artifact, not a session store.** The dossier's
own framing section states the operational test plainly: is the file ever
read back by the *producing* program to continue, versus read only by a
*different* consumer (an evaluator, a viewer, a demo tool)? For SWE-agent the
answer is no on every path traced. `run_replay.py` starts a brand-new
`SWEEnv` and a brand-new `DefaultAgent` and re-executes stored actions through
a `ReplayModel` that plays back the `history` list instead of querying a real
LLM (`sweagent/agent/agents.py:1284-1286`, `sweagent/run/run_replay.py:186-192`,
`sweagent/agent/models.py:464-481`, per the dossier). `RunBatch.should_skip`
either skips a completed instance outright or deletes an incomplete one and
reruns from scratch (`sweagent/run/run_batch.py:376-409`); nothing in that
path loads `history` back into a live agent. The inspector is read-only
display (`sweagent/inspector/server.py`, `sweagent/inspector/static.py`).

Every other divergence in this document is downstream of that one fact. We
are comparing a durable, resumable, addressable record (ours) against a
complete, replayable, but never-resumed document (theirs). Where the dossier
and this comparison therefore diverge from the shape of every other
comparison in this corpus: there is no meaningful "fact-by-fact mapping" to
draw for most of our catalog, because most of what we record exists to make
resumption possible, and SWE-agent has no resumption to support. The mapping
below is consequently short and, per the Method, does not manufacture
equivalents where none exist.

## Mapping

| SWE-agent | Ours | Verdict |
| --- | --- | --- |
| `<instance_id>.traj` file, rewritten whole every step (`get_trajectory_data`/`save_trajectory`, `sweagent/agent/agents.py:762-787`) | One append-only stream per session, `SessionEvent` oneof (`proto/trogonai/session/sessions/v1alpha1/events.proto:58-114`) | Structural mismatch, not an equivalence; see above |
| `trajectory: list[TrajectoryStep]`, a post-hoc per-step summary (`sweagent/types.py:44-52`) | `ToolCallRequested`/`Started`/`Completed`/`Failed` as separate durable facts (`tool_call_requested.proto`, `tool_call_completed.proto`, `tool_call_failed.proto`) | Ours, decisively; SWE-agent's step summary exists only inside a document that is rebuilt every write; ours is durable the instant it is appended |
| `history: list[HistoryItem]`, the literal LM conversation, "all messages that were shown to the LM" (`docs/usage/trajectories.md:22`) | `CanonicalMessage` on `UserMessageRecorded`/`AssistantMessageCompleted` (`proto/trogonai/session/sessions/v1alpha1/message.proto:14-28`) | Equivalent in intent (both are the provider-visible transcript form); ours is append-only per message, theirs is one array inside a rewritten whole document |
| `info: AgentInfo` (exit status, submission, cost stats, `swe_agent_hash`/`swe_agent_version`; `sweagent/types.py:94-95`) | `SessionClosed`/`SessionFailed`/`SessionCancelled` + `TokenUsage`/`Cost` spread across message events (`session_closed.proto`, `token_usage.proto`) | Ours, decisively; no single denormalized rollup object that the whole write path recomputes and re-serializes on every step |
| Instance id: content-derived sha256[:6] of the problem text for most problem-statement types, `uuid.uuid4()` only for the no-problem-statement case (`sweagent/agent/problem_statement.py:84-86,117-119,144-147,58`) | Opaque `SessionId`, one logical stream per session ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) | Semantic mismatch; see below |
| `output_dir`/file path as the true identity for collision purposes; no id field inside the JSON itself (`sweagent/agent/agents.py:589`, per the dossier's Keying section) | `SessionId` addresses a JetStream subject `session.sessions.events.<session_id>`, independent of any storage path ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) | Ours, decisively; identity survives relocation; SWE-agent's does not, by the dossier's own account ("move the file and... that trajectory no longer exists at its old identity") |
| `RunBatch.should_skip`: skip a complete run, delete-and-rerun an incomplete one (`sweagent/run/run_batch.py:376-409`) | No equivalent concept; dedup-on-completion is not a primitive our catalog needs, because we never delete-and-redo a session | Deliberate divergence, see "Skip is not resume" below |
| `RetryAgent` attempts, each a full independent `DefaultAgent` with its own `.traj` at `output_dir / f"attempt_{i}"`, folded whole into the parent's `{"attempts": [...]}` (`sweagent/agent/agents.py:257-440, 358-388`) | `DelegationDispatched`/`ParentLinked` linking two independently durable streams, never a copy of one inside the other (`delegation_dispatched.proto`, `parent_linked.proto`) | Ours, decisively; see the subagent-cascade section below |
| No rewind/undo/branch operation exists at all (per dossier, "Rewind, checkpoints, and fork") | `SessionRewound.keep_through`, `SessionForked` (`session_rewound.proto`, `session_forked.proto`) | Ours; SWE-agent's `while not step_output.done` loop only ever moves forward |
| `state.diff` (`diff_state` tool bundle): a full, non-deduplicated `git diff --cached` recomputed every step, used only as a crash-recovery autosubmission fallback (`attempt_autosubmission_after_error`, `sweagent/agent/agents.py:823-851`) | `FileChanged.before_ref`/`after_ref` (`ArtifactRef`, content-addressed, deduplicated) plus `DiffSummary` (`file_changed.proto`, `diff_summary.proto`) | Ours, decisively; see below |
| No compaction/history-summarization concept found; `history_processors` only ever produce a model-visible view (`sweagent/agent/agents.py:539-551`) | `Compacted{covers_from, covers_through, summary_content}` (`compacted.proto`) | Ours; no equivalent exists on their side to compare against; not a gap, since a benchmark run's transcript never needs to be shortened for context-window reasons across resumption |
| No retention/TTL policy; the only cleanup is `remove_unfinished`, a manual, opt-in, `dry_run=True`-by-default offline CLI tool (`sweagent/run/remove_unfinished.py:14-41`) | `SessionHidden`, `RedactionApplied`, `ArtifactErased` (`session_hidden.proto`, `redaction_applied.proto`, `artifact_erased.proto`); keep-forever with a typed masking contract ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours, decisively; see the retention section below |
| No index; every consumer globs `*.traj`/`**/*.traj` (`sweagent/run/remove_unfinished.py:20`, `sweagent/inspector/server.py:274`) | `SessionProjection`, a rebuildable read model checkpointing `last_applied_stream_position` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively |
| No search subsystem of any kind over trajectory content (per dossier) | Out of scope for the core catalog too; "any full-text or vector search subsystem is a separate, independently bootstrapped projection off the same log" ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Trade-off/parity; neither side builds this into the store proper |

## What we should consider changing

None. Every recommendation in this section had to clear one bar: does
SWE-agent's evidence say something about our design that a *stronger* store
in this corpus (fx, Cline, or another product scoring above 6/12) has not
already said more strongly? For every candidate change that came up while
writing this comparison, the answer was no, and the reason is the structural
difference itself: SWE-agent's design choices are optimized for a batch
harness that recomputes everything from scratch and never resumes, and the
properties that follow from that (whole-file rewrite, no lock, no rewind, no
retention policy) are absent *because resumption isn't attempted*, not
because SWE-agent found a cheaper way to get resumption's benefits. A store
whose maturity score is 3/12, and whose exposure axis is capped precisely
because it never faces the failure modes ours must survive, cannot anchor a
schema or ADR-decision change on its own. Where SWE-agent's evidence does
sharpen something, it sharpens the *rationale* for a decision we already made,
which is what the sections below record instead of a numbered recommendation
list.

If a reader wants the one thing closest to a recommendation: confirm, in the
ADR or an implementation note, that `should_skip`-style "dedup on completion"
and "resume from position" remain two different vocabulary items in our own
design language, never conflated even informally. That costs nothing to
write down (**blast radius: additive**, a documentation clarification only)
and the evidence anchor is exactly the confusion SWE-agent's own code
invites, addressed in "What not to copy" below.

## What our design already does better

**Content addressing instead of full non-deduplicated snapshots.** SWE-agent's
`diff_state` bundle recomputes a full `git diff --cached` at *every step* and
stores it inline in `TrajectoryStep["state"]["diff"]`, with "no
content-addressing or diffing between steps; each step's `diff` is a
complete snapshot of the working tree's staged changes at that instant" (per
the dossier's Rewind/checkpoints section). Our `FileChanged.before_ref`/
`after_ref` are `ArtifactRef`s keyed by `Digest`, deduplicated globally, with
a `DiffSummary` carrying exact line counts and a claim-checked rendered form
(`file_changed.proto:33-46`, `diff_summary.proto`). SWE-agent's mechanism
exists purely as an autosubmission fallback for a dying process, not as a
first-class change record, and it pays for that with an unbounded,
non-deduplicated snapshot on every single step it is enabled for.

**Durable identity independent of storage location.** SWE-agent's identity is
the file path itself; per the dossier, "move the file and, from the program's
point of view, that trajectory no longer exists at its old identity," because
neither `info` nor `trajectory`/`history` carries an `instance_id` field of
its own. Our `SessionId` addresses a JetStream subject
(`session.sessions.events.<session_id>`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) independent of
any physical storage location, so relocation, cold-tiering, or backup/restore
never breaks identity.

**Typed, durable outcome record instead of a pair of parsed fields.**
SWE-agent's `AgentInfo.exit_status` is the sole signal `should_skip` inspects
to decide completion, and the dossier notes there is no compare-and-swap, no
ETag, no sequence check on any write to the file at all. Our terminal markers
(`SessionClosed`, `SessionCancelled`, `SessionFailed`, `SessionHidden`) are
each their own typed, `At`-guarded event with a typed reason enum
(`session_cancelled.proto:19-33`, `session_failed.proto:16-28`), so "why did
this session end" is a durable fact rather than a string parsed out of an
exit-status field designed for a different purpose (skip-or-rerun logic).

**Rewind and fork exist at all.** SWE-agent's `while not step_output.done`
loop "only moves forward" (per dossier); there is no rewind, undo, or branch
operation anywhere in the codebase, and the one appearance of "fork" in the
source is git-repository forking for opening a pull request, unrelated to
session forking. `SessionRewound.keep_through` and `SessionForked` with
`context_prefix_boundary` (`session_rewound.proto`, `session_forked.proto`)
give us both, atomically and without touching prior events.

## Trade-offs, not gaps

**Whole-document rewrite is a coherent choice for a system that never needs
partial recovery of its own record; and stops being coherent the moment
recomputing from scratch is not free.** SWE-agent's `save_trajectory`
overwrites the entire accumulated `history` + `trajectory` + `info` from
scratch on every step via a single `write_text` call, with "no temp file, no
fsync anywhere in this path" (`sweagent/agent/agents.py:786-787`, confirmed by
grep across `sweagent/agent/agents.py` and `sweagent/utils/`, per the
dossier). A crash mid-`write_text` can leave a torn JSON file. The batch
runner's answer to that torn file is not repair; it is deletion and rerun
(`RunBatch.should_skip`, `sweagent/run/run_batch.py:391-406`). This is a
genuinely defensible design *for SWE-agent specifically*: an instance run is
a pure function of a public benchmark problem statement plus a deterministic
harness plus (for the non-`ReplayModel` case) a model API call, so "delete
and recompute" costs one more model-call budget, not lost work. The whole
system is built around the assumption that recomputation is cheap and the
inputs are reproducible.

That assumption is exactly what breaks for a session store whose sessions are
user-owned and not reproducible. A user's session cannot be regenerated from
a public problem statement; the "input" is an unrepeatable sequence of user
messages, tool executions against a live filesystem and network, and
model responses that are not deterministic even given the same prompt. Once a
session is a record of something that happened rather than a cached answer to
a reproducible question, "delete the torn file and start over" stops being a
recovery strategy and becomes a bill for a user's genuinely irrecoverable
work. This is the sharpest available argument, in this whole corpus, for why
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2's append-only mutation with a server-enforced
`WRITE_PRECONDITION` matters: our design never has a "whole document" to tear
in the first place, because "append one small fact, guarded where it needs
guarding" replaces "rewrite everything and hope the write finishes" as the
unit of durability. SWE-agent is not a counterexample to that design; it is
the clearest illustration of why a benchmark harness can get away with the
thing our design is built to rule out, and why the same shortcut would be a
user-hostile failure mode for us.

**"Skip if already done" is not resume, and SWE-agent shows how easily the two
get confused.** `should_skip`'s behavior (skip a complete run, delete-and-
redo an incomplete one) looks superficially like idempotent resume but is
neither incremental nor state-preserving: nothing in that path ever loads
`history` back into a live agent (per dossier, Framing section, point 2). Our
platform keeps these concepts sharply separate by construction: `NoStream` on
`CreateSession` makes creation idempotent-by-rejection (a second create simply
fails), while resumption is `StartExecutionAttempt` replaying the effective
tail after the newest admitted checkpoint ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8, facet 3). The
value of SWE-agent's evidence here is not that it proposes a change to our
schema, it proposes nothing because it has no resume path to compare, but
that it is the cleanest available demonstration of a category error a future
implementer could make by accident: treating "the file already exists and
looks done" as equivalent to "we know how to pick this session back up,"
when the two require entirely different guarantees (idempotent rejection
versus checkpoint-verified replay).

## What not to copy

- **Whole-file rewrite with no temp-file-and-rename and no fsync.**
  `save_trajectory`'s single `write_text` call, invoked after every agent step
  (`sweagent/agent/agents.py:779-787,1284-1286`), is the exact failure mode our
  append-only log with server-enforced write preconditions ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision
  2) exists to rule out. Even setting aside resumption, this is a pattern to
  reject on durability grounds alone: a crash mid-write can silently corrupt
  the entire record, not just the in-flight step.
- **Healing a torn record by deletion rather than repair.**
  `should_skip`'s response to an unparsable or incomplete `.traj` file is
  `log_path.unlink()` followed by a full rerun (`sweagent/run/run_batch.py:
  391,400,405`). Coherent when the record is a cached, reproducible answer;
  actively harmful as a pattern for a store whose records are the only copy
  of something that happened and cannot be regenerated.
- **A processor that mutates the durable record while claiming to be a
  view-only mechanism.** SWE-agent's `history_processors` are documented and,
  for four of five processors, actually implemented as read-time,
  copy-first transformations that never touch `self.history`
  (`LastNObservations`, `ClosedWindowHistoryProcessor`, `RemoveRegex`,
  `ImageParsingHistoryProcessor`, all copying or deep-copying `entry` before
  mutating it, `sweagent/agent/history_processors.py:157-176,230-258,
  320-336,352-360`). `CacheControlHistoryProcessor` is the one exception: its
  `__call__` mutates `entry` in place with no copy anywhere in the function
  (`sweagent/agent/history_processors.py:287-303`), so a prompt-caching
  metadata hint leaks into the exact dict objects that `self.history` holds
  and gets serialized into the next `.traj` write. The dossier is careful to
  call this "metadata, not conversational content," and that is true; but it
  is still a concrete instance of a model-view-only mechanism eroding the
  boundary between "what the model sees" and "what the durable record
  contains," inside a codebase whose other four processors got that boundary
  right. This is exactly the boundary [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 8 keeps explicit by
  construction: the model-visible context is *compiled* deterministically
  from the event log, never mutated back into it, and `ProviderBlock`/
  `ThinkingBlock.signature` are the two places we deliberately let
  provider-specific data ride along, both write-verbatim-read-never, never a
  silent in-place mutation of an already-durable payload. The lesson is not
  "audit history_processors code we don't have"; it's "a documented
  view/record split is only as good as its least-audited implementation, and
  a single mutating branch is enough to erode it invisibly." Our own design
  has no equivalent mutation path today (compaction and redaction are both
  new appended events, never edits, per [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2), and this
  comparison is the reason to keep it that way rather than to introduce a
  "cheap" in-place metadata patch later.
- **An identity that lives only in a file path, with no id inside the
  payload.** Neither `info` nor any `TrajectoryStep`/`HistoryItem` carries an
  `instance_id` field; the id lives only in the filename and the in-memory
  problem-statement object (per dossier, Keying section). This makes the
  record itself non-self-describing: read the JSON with no path context and
  you cannot say whose trajectory it is. Every one of our events carries
  `session_id` as a `LEGACY_REQUIRED` field for exactly this reason.

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a position: a child session is its own
logical stream, linked by facts recorded on each side
(`DelegationDispatched`/`ParentLinked`), acyclic by construction (a fresh
`child_session_id` every dispatch, `ParentLinked` valid only inside a
`NoStream` creation batch), with terminal cascade driven by a reconciler
reacting to Session-level terminal markers and rewind-invalidation kept a
distinct saga from terminal cascade. The question here is whether
`RetryAgent`'s evidence validates, challenges, or refines that position.

**What `RetryAgent` does.** Each attempt is a full, independent `DefaultAgent`
instance with its own output directory (`output_dir / f"attempt_{i}"`,
`sweagent/agent/agents.py:303-319`) and therefore its own, separately durable
`<instance_id>.traj` at that path; the closest thing in this codebase to a
subagent: an independently durable child run. It is *also* folded whole into
the parent's own `.traj` under `"attempts": [...]` (`sweagent/agent/agents.py:
358-364,385-388`), so the child transcript exists twice: once as its own
file, and once duplicated inside the parent's file. `_next_attempt` calls
`self._env.hard_reset()` and sets up a brand-new `DefaultAgent`
(`sweagent/agent/agents.py:321-326`); there is no shared-prefix history
between attempts, and lineage is recorded only by the attempt's list index in
the parent's `.traj`, not by any parent/child pointer inside the child's own
file. Nesting is bounded only by `RetryLoopConfig.max_attempts` and a cost
limit (`sweagent/agent/reviewer.py:184-216`); per the dossier, "there is no
crash/rewind cascade to reconcile because each attempt is independent from
setup."

**Does this validate, challenge, or refine decision 6?** It is the honest
"this product has no position" case the Method anticipates, and it is worth
stating precisely why, rather than treating the absence as silence. A
`RetryAgent` never faces the cascade problem decision 6 solves, for a reason
that is structural, not incidental: there is no live parent process for a
crashed or rewound attempt to cascade *from*. `_next_attempt` runs only after
the *current* process has already decided the prior attempt is done (an
in-process, synchronous decision, not a fact discovered later by a reconciler
watching for a terminal marker); there is no notion of an attempt continuing
to run unsupervised while another part of the system decides its parent's
fate. Decision 6's entire cascade machinery; the reconciler reacting to
`ParentTerminated`/`ParentHistoryInvalidated`, the crash-repair path for a
dispatch that acked but whose child creation never happened, the eventually-
consistent O(depth) cascade; exists to solve a problem that only exists once
parent and child are running as independent, potentially crashable,
potentially concurrently-progressing processes linked by durable facts rather
than by being steps in the same call stack. `RetryAgent`'s attempts are
steps in the same call stack. This does not challenge decision 6; it
sharpens exactly what decision 6 is *for*, by showing the one case where
the machinery would be pure overhead: a supervisor and worker with no
independent liveness, no possibility of the worker outliving a decision about
it, and therefore nothing to reconcile. That case does not describe our
child sessions (which are dispatched to run independently and *can* outlive
or crash independently of the parent), so it is not evidence for simplifying
decision 6; it is evidence that decision 6 correctly targets a harder
problem than SWE-agent ever has to solve.

One thing worth naming as a genuine, if minor, point of comparison: `RetryAgent`
duplicating the full child transcript inside the parent's own file
(`"attempts": [...]`) is the pattern [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s record-once rule
([ADR#0024](../../../../adr/0024-agent-platform-stream-topology.md), cited throughout [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 6) explicitly forecloses for us:
our parent never carries a copy of a child's events; it carries only the
linking facts (`DelegationDispatched`, and later the delegation's own
`OperationOutcomeRecorded`). SWE-agent's duplication is affordable because a
`.traj` file is rewritten from a small, bounded, in-memory Python list every
time anyway; it would not be affordable, and would violate our own stream-
placement rule, if attempted at the scale and independence our child sessions
operate at.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a position: keep-forever, with
`SessionHidden` as a visibility tombstone, `RedactionApplied` for read-time
masking, `ArtifactErased` for out-of-band artifact-byte destruction, and
aggregate snapshots bounding replay cost rather than storage. The question
here is whether SWE-agent's evidence validates that design or exposes a cost
the ADR does not bound.

**What SWE-agent does.** There is no TTL, no scheduled cleanup, and no
automatic retention policy anywhere in the codebase or docs; trajectory
directories accumulate under `trajectories/<user>/<experiment>/` indefinitely
unless a human intervenes (per dossier, Retention section). The only cleanup
tool is `remove_unfinished`
(`sweagent/run/remove_unfinished.py:14-41`): a *separate, manual CLI
invocation*, `dry_run=True` by default, that deletes a whole instance
directory only when it finds exactly one `.traj` with no `info.submission`.
It is never called from `run`, `run-batch`, or any hook; a human has to
remember it exists and choose to run it. The dossier found no issue reports
naming this a user-visible problem, which it explicitly attributes to scale:
"the whole design assumes a batch of hundreds to a few thousand instances...
processed once, not a live, growing store enumerated repeatedly."

**Does this validate, challenge, or refine decision 7?** It validates the
core shape (keep-forever, no automatic purge) while being far too thin an
evidence base to validate the *rest* of decision 7's contract, and the gap
between the two is itself informative. SWE-agent's "retention policy" is not
really a retention policy in the sense decision 7 means the term: it is an
offline maintenance script a human runs by hand, with no automatic trigger,
no typed reason, no visibility tombstone, no read-time masking, and no
distinction between "hidden from listing" and "bytes destroyed"; `rmtree`
deletes the whole directory outright, an irreversible physical purge, not the
graduated masking-then-erasure story decision 7 builds (`SessionHidden` never
deletes bytes; `RedactionApplied` masks at read time over a keep-forever log;
`ArtifactErased` is the one place bytes actually go, and only for claim-
checked artifacts, never for the event log itself). SWE-agent's approach is
coherent for its own scale and purpose (a bounded batch of instances, run
once, where an incomplete/unsubmitted directory really is disposable junk,
not a record anyone needs to audit later) but it offers no evidence at all
about the harder question decision 7 actually answers: what happens when a
log *cannot* be treated as disposable and must instead support redaction,
audit, and eventual legal erasure while never losing the ability to replay.
The dossier is explicit that no growth-related issue was found for this
product, which is the expected result of "batch, not live" scale, not
evidence the pattern would hold at session-store scale; this is exactly the
maturity-weighting the Method calls for: a 3/12 store's silence on a failure
mode is not evidence the failure mode doesn't matter, it's evidence the store
was never big enough or long-lived enough to hit it. Decision 7's own harder
cases (fx's usage-ledger-dominated log, Cline's confirmed `#9011` growth
failure, both discussed in those products' comparisons) remain the load-
bearing evidence for this gap; SWE-agent adds nothing to that case beyond
confirming, once more, that "no automatic policy, manual cleanup only" is
the default an unconstrained system drifts toward absent a deliberate
decision like ours.

## Open questions for the ADR

None. Every question this comparison could plausibly raise is already
answered more sharply by a stronger store elsewhere in the corpus (fx or
Cline), and manufacturing an ADR question from a 3/12-maturity, non-resuming
benchmark harness would misrepresent how much weight its evidence can bear.
The one item worth flagging is not a question for the ADR owner so much as a
note for whoever writes the ADR's prose on idempotency: SWE-agent's
`should_skip` is a good citable example, in ADR text or an implementation
note, of the specific "skip-vs-resume" conflation [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s `NoStream`
(idempotent-by-rejection creation) versus `StartExecutionAttempt`
(checkpoint-verified replay) split already prevents by construction; worth
keeping in mind as a concrete illustration if that distinction ever needs
re-explaining to a new implementer, not as an open design question.
