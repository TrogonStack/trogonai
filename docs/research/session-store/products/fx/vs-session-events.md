# fx compared to our session event catalog

Part of Session Store Research. This maps the reconstructed fx storage
format ([dossier](./index.md)) onto `trogonai.session.sessions.v1alpha1`
and separates three things: where fx carries structure we do not, where our
model is stronger, and where the difference is a trade-off rather than a gap.

Sources are the fx dossier (evidence tags `[observed]` / `[literal]` carry
over unchanged), the documented CLI read contract in the
[fx session detail JSON reference](./session-detail-json-reference.md),
and the 57 `.proto` files under
`proto/trogonai/session/sessions/v1alpha1/`. Where a conclusion here differs
from an accepted record in the [ADR index](../../../../adr/index.md), the ADR is
authoritative. This document proposes; it does not decide.

## The one structural difference everything else follows from

fx commits at **turn granularity**. A single `history_turn_committed` event
carries the user message, the final assistant message, every tool step, every
tool call and result, every diff, and the per-turn file ledger, written once
after the turn finishes. Nothing about a turn is durable until all of it is.

We commit at **fact granularity**. `UserMessageRecorded`,
`AssistantMessageStarted`, `ToolCallRequested`, `ToolCallStarted`,
`ToolCallCompleted`, `AssistantMessageCompleted` are separate events on the
session stream, each durable when it happens.

Most of fx's compound fields exist to compensate for turn-granular commit,
and we get their effect for free:

| fx field | Why fx needs it | Our equivalent |
| --- | --- | --- |
| `interrupted.completed_tool_names[]` | The turn event never got written, so the cancelled turn has to restate which tools finished | The `ToolCallCompleted` events are already on the stream |
| `interrupted.tool_call` | Names the call that was in flight at the cut | A `ToolCallStarted` with no terminal event |
| `interrupted.cancelled_command` | Same, for the command case | `ToolCallFailed` with `TOOL_CALL_FAILURE_REASON_CANCELLED` |
| `tool_steps[].assistant` | Interstitial narration has no place to live between commits | Ordinary `AssistantMessageCompleted` events, interleaved |
| `execution.files[]` | A per-turn rollup, because there is no per-event stream to scan | Fold over `FileChanged` |

The corollary is the cost: fx pays one large write per turn and loses an
in-flight turn on crash, while we pay N small writes and can always resume
mid-turn. That is settled by
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) and is not
reopened here.

The rest of this document is about the fields where fx is carrying real
information that our catalog does not model.

## Mapping

| fx | Ours | Verdict |
| --- | --- | --- |
| `session.json` manifest | Fold of the session stream | Ours (no denormalised counters to drift) |
| `authority_id` + `session.lock` + `log_generation` | JetStream `WRITE_PRECONDITION` | Ours, with one caveat (see epochs below) |
| `seq` in the envelope | `SessionOrdinal`, fold-derived | Trade-off |
| `checkpoint.json` + `checkpoint_sha256` | `Checkpoint` with `Digest`, `covers_through` | Ours (checkpoint is an event, not a file) |
| `state.context_history_start` | `Compacted.covers_from` / `covers_through` | Ours (range, not index) |
| `HistoryTurn{assistant}` | `UserMessageRecorded` + `AssistantMessageCompleted` | Ours |
| `HistoryTurn{interrupted}` | `AssistantMessageFailed` + per-call terminal events | Ours |
| `HistoryTurn{background_command}` + `background_record_id` | No equivalent | Gap, minor |
| `HistoryTurn{compacted_summary}` | `Compacted` | Ours |
| `UserMessage{text, images[]}` | `CanonicalMessage` with `ContentBlock` oneof | Ours (blocks, thinking, signatures) |
| `ToolCall{id, name, arguments_json}` | `ToolCallRequested{tool_call_id, tool_name, input_json}` | Equivalent |
| `ToolCall.provider_result`, `ToolResult.provider_native` | `ProviderBlock` arm on `ContentBlock` | Closed in `v1alpha1` (write-verbatim, read-never) |
| `ToolResult.output` | `TextToolResult.content` | Equivalent |
| `ToolResult.output_handle` + `preview` | `ArtifactRef{artifact_id, digest, preview}` | Ours (content-addressed) |
| `ToolResult.output_bytes` vs `stored_output_bytes` | `ArtifactRef.size_bytes` + `untruncated_size_bytes` | Closed in `v1alpha1` |
| `ToolResult.status` | `ToolCallResultStatus` + `ToolCallFailureReason` | Ours |
| `command_process_presentation{exit_code, signal}` | `ToolCallCompleted.termination` (`CommandTermination`) + `duration` | Closed in `v1alpha1` |
| `command_output_replay{handle, framed_bytes}` | No equivalent | Gap, optional |
| `CommittedFilePresentation.previous_content` (inline) | `FileChanged.before_ref` | Ours, decisively |
| `CommittedFilePresentation.lines[]`, `additions`, `deletions` | `FileChanged.diff` (`DiffSummary`: exact counts, rendered artifact) | Closed in `v1alpha1`, minus a structured per-line array |
| `CommittedFilePresentation.lifecycle_id{turn_id, call_id}` | `FileChanged.turn_id` + `tool_call_id` | Closed in `v1alpha1` |
| `execution.files[].action = read` (of 9 action values) | `ToolCallCompleted.observed` (`ResourceObservation`) | Closed in `v1alpha1` for content reads; no list/search/copy taxonomy |
| `execution.files[].stale`, `model_view_covers_full_file` | `ResourceObservation.complete` + `range` for coverage; freshness stays in the fold | Coverage closed in `v1alpha1`; `stale` deliberately not copied |
| `UsageSnapshot` counters | `TokenUsage` + `Cost{amount_micros, rate_ref}` | Ours |
| `usage.billing`, `*_complete` flags | `TokenUsage.completeness` | Closed in `v1alpha1` |
| `usage.pending[]`, `settled_through_sequence` | Not in the session stream | Ours, deliberately |
| `preferences{model, effort, fast_mode}` + `preferences_changed` | `AssistantMessageStarted.settings` (`ModelSettings`) | Closed in `v1alpha1` (per-completion; a change is two adjacent facts) |
| `workspace_root`, `origin_workspace_root`, `workspace_rebound` | `SessionStarted.workspace` (`WorkspaceRef`) | Closed in `v1alpha1`; rebinding is out of scope (new session or fork) |
| `conversation_language` | No equivalent | Gap, minor |
| `display.json{title, preview}` | `SessionRenamed` + read model | Equivalent |
| subagent relationship pages, channel families | `DelegationDispatched`, `ParentLinked`, `CascadePolicy` | Ours, decisively |
| `session migrate` with a journaled migration record | No equivalent | **Gap**, cheap to close |
| `fx session <id> --json` read contract (documented beta, unversioned top level) | Not yet designed | Warning (see below) |
| `fx session <id> --json` error envelope `{kind, error, code}` | Not yet designed | Warning (query error shapes undecided) |
| `ToolResult.permission_feedback[]` (flat strings) | `ToolCallApproved` / `ToolCallDenied` | Ours (typed approval lifecycle) |

## What we should consider changing

Ordered by how much is lost today, not by implementation cost.

Status note: items 1, 2, 3, 5, 6, 7, 8, and the `untruncated_size_bytes` part of
item 10 have since been implemented in `v1alpha1` and folded into
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 3, and
the mapping table above reflects that ("Closed in `v1alpha1`" rows). Item 4
was implemented as `ResourceObservation` on `ToolCallCompleted` rather than as a
`FileRead` event, for the reason given in that section. Items 9 and the rest of
10 remain open.

### 1. There is no turn identity in our catalog

fx correlates a committed file edit with `lifecycle_id = {turn_id, call_id}`.
The `call_id` half we have. The `turn_id` half we do not: nothing in our 41
event arms groups the facts of one user-request-to-final-answer cycle. A
reader wanting "show me this turn" has to reconstruct the boundary by
scanning for the previous `UserMessageRecorded`, which is a heuristic, not a
fact. It breaks on the cases that matter most: a turn that fans out into
delegated child sessions, a turn resumed on a second `ExecutionAttempt`, a
turn interrupted and retried.

Two ways to close it, both compatible with per-fact commit:

- A `turn_id` (or `round_id`) correlator on the conversation and tool events,
  minted at `UserMessageRecorded` and echoed on everything caused by it. This
  is the fx shape and the cheap one.
- An explicit `TurnStarted` / `TurnEnded` pair. More events, but it makes the
  boundary a fact with its own outcome, which is what interruption and retry
  actually need.

We already accepted this shape once, for tool calls: `ToolCallRequested`
carries both `tool_call_id` and `tool_execution_id` precisely so a retried
execution of the same logical call stays correlated. A turn deserves the
same treatment.

### 2. Command results are primitive text

fx stores `{"kind":"exit_code","value":1}` (and a `signal` variant
[literal]). We flatten every command outcome into `TextToolResult.content`
plus a two-value `ToolCallResultStatus`. For a coding agent, the exit status
is *the* structured outcome of the most consequential tool we run, and today
a consumer has to parse prose to recover it. This is exactly the primitive
obsession the codebase standards call out.

Proposal: a typed result variant in the `ToolCallResult` oneof, for example
`CommandToolResult { oneof termination { int32 exit_code, int32 signal },
Duration duration, ArtifactRef stdout, ArtifactRef stderr }`. The oneof
already exists, so this is additive and does not disturb the text case.

### 3. No provider-native passthrough

fx keeps `provider_result` on the call and `provider_native` on the result.
Both were inert in the local corpus, but the fields are the escape hatch: a
place to retain a provider's own representation of a step that our canonical
form does not cover. Our catalog is canonical-only, with exactly one
exception, `ThinkingBlock.signature`, which is a provider-native blob we were
forced to keep because dropping it breaks re-prompting.

That exception is the argument. Server-side tool use, provider-side search
results, cache breakpoints, and future block types will each present the same
choice: extend the canonical union, or lose the ability to replay the
conversation to the same provider byte-for-byte. A generic escape hatch (an
opaque `ArtifactRef` or `bytes` on `CanonicalMessage` and `ToolCallResult`,
tagged with the provider) buys forward compatibility at low cost, with the
usual discipline: it is for replay, never for reading.

### 4. We record file changes but not file reads

`FileChanged` covers `CREATED`, `MODIFIED`, `DELETED`, `RENAMED`. fx's
`execution.files[]` also records `read`, `list`, and `search` actions, 340
entries across two sessions, of which the large majority are reads.

What we lose by not recording reads:

- **Audit.** "What did this agent look at" is unanswerable from our stream.
  For any session touching sensitive files, that is the question a reviewer
  asks first.
- **Context reconstruction.** Rebuilding what the model had in view at a
  given ordinal requires knowing which files were read and how much of each.
- **Cache and staleness reasoning.** Whether a file was re-read after being
  changed by an external process is not derivable.

Note that a read is arguably not a session *fact* but a tool detail, and
`ToolCallCompleted` for a `read_file` call already implies it. The honest
statement is that the information is recoverable only by parsing tool inputs,
which is the same anti-pattern as item 2. If reads matter, they should be
typed.

Resolved as `repeated ResourceObservation observed` on `ToolCallCompleted`, not
as a `FileRead` event. The observed read:edit ratio in the sampled sessions was
194:9, so a per-read event would have dominated the log while carrying almost no
decision value, and reads have no lifecycle worth its own arm. Hanging the read
set on the call that already completed answers all three losses above and, by
recording the digest and byte range actually seen, turns a replayed write into
something checkable rather than merely repeatable.

### 5. No rendered diff summary on `FileChanged`

fx stores `additions`, `deletions`, `truncated`, and a rendered `lines[]`
array with `{kind, old_line, new_line, text}` per line. Rendering a change
list therefore costs one read. Ours costs two artifact fetches plus a diff
computation per changed file, on every render, for a result that never
changes.

fx pays for this the wrong way, by inlining full pre-images (see below), but
the summary itself is sound. Proposal: add an optional
`DiffSummary { uint64 additions, uint64 deletions, bool truncated,
ArtifactRef rendered }` to `FileChanged`. The counts are tiny and answer most
UI questions on their own; the rendered form stays a claim check, so the
event does not grow.

Bonus: `lines_added` / `lines_removed`, which fx tracks as first-class
product metrics, become a fold over these counts instead of a separate
counter.

### 6. Usage has no completeness flag

fx marks every snapshot with `billing: complete | incomplete` plus
`api_duration_complete`, `wall_duration_complete`, and `code_complete`. Those
flags exist because interrupted and streaming turns produce genuinely partial
numbers, and a consumer that cannot tell partial from final will either
double-count or under-bill.

Our `TokenUsage` has no such marker. An interrupted assistant message and a
completed one carry structurally identical usage. Proposal: a completeness
enum on `TokenUsage` (or, more in keeping with our style, allow usage to be
absent and require it only on terminal events, making absence mean
"unknown"). The choice matters less than making the distinction expressible.

Duration is a related but weaker case: fx stores `api_duration_ms` and
`wall_duration_ms`, and ours are derivable from event timestamps. Derivable
is good enough.

### 7. Model settings beyond the model name are not recorded

fx persists `preferences {model, effort, fast_mode}` in the manifest, in the
`session_started` payload, and again in a `preferences_changed` event
[literal], so a mid-session change of reasoning effort is an auditable fact.

We record `model` on `CanonicalMessage` and `AssistantMessageStarted`, and
nothing else. Reasoning effort, thinking budget, temperature, and the
provider-side flags that changed the output are all absent, so a session
cannot be reproduced from its own record. Proposal: a `ModelSettings` value
object alongside `model`, and, if settings can change mid-session, an event
recording the change rather than only its effects.

### 8. Session-to-workspace binding is invisible

fx binds a session to a workspace root, keeps `origin_workspace_root`
separately from the current `workspace_root`, emits `workspace_rebound` when
the two diverge [literal], and maintains a `latest/<sha256(workspace_root)>`
index so "resume the last session for this directory" is a single lookup.

For us the workspace lives inside `StoredSessionExecutionPlan.plan_bytes`,
which the session store treats as opaque. Three consequences: the binding is
not queryable, a rebind is not expressible except by replacing the plan, and
"latest session for this workspace" cannot be served without opening the
plan. For a coding-agent product this is the most user-visible gap in the
list. Proposal: lift a typed workspace or repository reference out of the
plan bytes into the session events, and decide explicitly whether rebinding
is a supported operation.

### 9. Migrations are not journaled

fx treats schema migration as a first-class, auditable operation: one-way,
size-guarded, non-destructive on failure, and journaled with the prior
storage format, prior digest, and proposed digest. We will eventually move
`v1alpha1` to `v1`. Recording that transition as an event on the session
itself, with digests either side, costs one message type and turns a
migration from an operational memory into a fact.

### 10. Minor

- `background_command` turns: fx models "the work was handed to a background
  process and the turn ends here" as a distinct turn kind with a
  `background_record_id` (the documented read contract also carries
  `log_path`, `expect_url`, and `url`). We have no notion of a tool call
  whose result arrives outside the session's own timeline. The operation
  ledger (`OperationReserved` / `OperationOutcomeRecorded`) is the natural
  home if we want one.
- `conversation_language` at session scope. Small, but it is a real product
  behaviour and it is not derivable after redaction.
- `command_output_replay`: fx keeps framed terminal tapes so a UI can replay
  output with original timing. Worth wanting; not worth copying its
  implementation, since the tapes live outside the session directory and
  become dangling references.
- `output_bytes` vs `stored_output_bytes`: our `ArtifactRef.size_bytes` is a
  single number, so when a result is truncated before storage we lose the
  original size. One extra field makes "this was 40 MB, we kept 64 KB"
  expressible.

## What our design already does better

**Content addressing instead of inlined pre-images.** fx stores the entire
prior file content inline in `previous_content`, per edit, with no dedup, and
does it twice: in the committed turn event and in the folded checkpoint. One
observed session produced a 1.2 MiB checkpoint from a single turn. Our
`before_ref` / `after_ref` `ArtifactRef` pair with `Digest` deduplicates
identical content globally, keeps the event small, and makes erasure possible
without touching the event. This is the clearest win in the comparison.

**Usage does not live in the session log.** 1434 of 1436 observed fx events
are `usage_checkpointed`; two are actual conversation. fx's session log is a
billing ledger with a transcript attached, and it carries a settlement
watermark, a pending queue, and a publication backlog inside the session
directory. Ours attaches usage to the facts that caused it and leaves
settlement to a downstream consumer of the stream. The ratio is the
cautionary tale: a session stream that also serves as a billing ledger is
dominated by billing.

**Range-addressed compaction with provenance.** fx's `compacted_summary`
carries `removed_turn_count` and `compaction_count`, both counts, plus a
`context_history_start` index into a live array. Any earlier rewrite
invalidates them, and the dossier could not settle whether history is
rewritten by `state_replacement_*` or not. `Compacted` names
`covers_from` and `covers_through` as `SessionOrdinal`s, records the
`CompactionTrigger`, the `guidance`, the model, the tokens either side, and
the usage the summarisation itself cost. The summary is addressed by range,
so it stays correct under any later operation, and the marker is in-stream,
so nothing is rewritten.

**Cancellation and failure are typed, not reconstructed.** `ToolCallFailed`
distinguishes `ERROR`, `CANCELLED`, `TIMEOUT`, `INTERRUPTED`;
`AssistantMessageFailed` has its own reason enum; the operation ledger
records `OperationCancelled { cancelled_by, reason }`. fx has one
`interrupted` turn shape that has to restate what completed.

**Idempotency is a modelled concern.** `OperationReserved` with a
`request_digest`, and `OperationOutcomeRecorded` with an explicit
`OperationUnknown` arm, mean a retried delegation or tool invocation has a
defined outcome including "we do not know". fx has no equivalent: a crashed
turn simply leaves nothing behind, and any external effect it had is
unrecorded.

**Delegation is sessions all the way down.** fx keeps subagent relationships
in an undocumented binary index (`relationship-index.bin`,
`relationship-page-*`) with a dozen live channel families and a repair path
for a corrupted projection. Ours makes a child a real session with
`DelegationDispatched`, `ParentLinked`, and a `CascadePolicy`, so the child
gets the same durability, resume, audit, and redaction machinery as any other
session, and the parent-child edge is an event rather than a sidecar to
repair.

**Erasure and redaction exist.** `RedactionApplied` names redacted event ids
in-stream; `ArtifactErased` removes payloads while the reference remains. The
fx corpus contains no retention policy, no deletion path, and no redaction
concept at all: a session grows without bound and every pre-image stays
forever.

**Server-side fencing.** `WRITE_PRECONDITION` (`Any` / `At` / `NoStream`) is
enforced by the broker. fx's equivalent is a lock file plus an
`authority_id` plus a `log_generation` echoed into every event plus a stat
fingerprint, all client-enforced, and the dossier notes a whole family of
recovery outcomes (`recovered`, `recovered_with_unverified_artifacts`,
`indeterminate`) that exist because that scheme can lose.

## Trade-offs, not gaps

**Fold-derived ordinals versus stamped sequence numbers.** fx writes `seq`
into every event, so a position is readable without folding. Our
`SessionOrdinal` is derived, so it is stable across restore and cannot be
forged, but every consumer that wants a position has to fold. Stamping the
ordinal at write time would reintroduce exactly the coordination problem the
broker's preconditions solve. Keep the fold; the cost is a read model, not a
correctness problem.

**Epoch versus stream sequence.** One thing fx's `authority_id` buys that our
preconditions do not: it survives a rewrite. If a session's stream is ever
recreated (restored from backup, migrated between clusters), sequence numbers
may not be identical, and an in-flight writer holding `At(seq)` could match
against a stream that is not the one it read. An epoch or authority token in
the aggregate would fence that case. This is a question for the ADR, not a
defect; it only bites under stream recreation, which may simply be
prohibited.

**Rollups versus folds.** fx's per-turn `files[]` ledger is a rollup stored
alongside the facts. It is redundant and can disagree with the tool results
it summarises, which is why we fold instead. But it is also the reason fx can
answer "what did this turn touch" with one read. If our read models end up
recomputing the same fold constantly, a materialised per-turn summary is a
legitimate read-side answer. The distinction to hold: a rollup in the read
model is fine, a rollup in the event is a second source of truth.

## What not to copy

- **Inlined pre-images.** Covered above. The single largest cost in fx's
  format.
- **Derived state baked into immutable records.** `execution.files[].stale`
  is a fact about the world that can change after the event is written. Once
  it is in the event, it is either wrong or it forces a rewrite. Freshness
  belongs in the fold.
- **An underversioned public projection.** `fx session <id> --json` emits
  `ExecutionRecord` `schema_version: 2`. An earlier revision of this document
  claimed it silently drops handles, diffs, and the file ledger; a field-level
  diff of the CLI output against the durable `schema_version: 3` checkpoint for
  the same session (v0.3.64) refutes that. The real drop is only
  `command_output_replay` and `command_process_presentation`, so the public
  JSON hides exit codes and replay tapes but keeps handles, diffs, and the
  file ledger, and the projection is now a documented beta contract, the
  [session detail JSON reference](./session-detail-json-reference.md). What
  survives of the criticism: the top-level response carries no
  `schema_version` of its own (only the nested execution object does), it
  omits the session-level metadata the store holds (workspace, model, token
  totals, title, preview), and the emitted JSON can contain unescaped control
  characters that strict parsers reject. When we expose a read API over the
  Session Store, its projection has to be a documented contract versioned at
  the top level and emitting strictly valid JSON, never an older internal
  shape reused as the public one.
- **Denormalised counters in the manifest.** `history_len`,
  `total_input_tokens`, `total_output_tokens`, `event_log_bytes`, and
  `last_event_seq` are all recomputable and all capable of drifting from the
  log they summarise.

## Questions this study opened, and how they were answered

All five are closed. They are kept here rather than deleted so the comparison
records what it actually changed.

- **Turn identity: a correlator field, or explicit `TurnStarted` / `TurnEnded`
  events?** The correlator. A required `turn_id` correlates conversation, tool,
  and file facts without adding events whose only content is a boundary.
- **Are file reads session facts or tool details?** Session facts.
  `ToolCallCompleted.observed` records URI, digest or confirmed absence, range,
  and completeness, so audit is a fold rather than a log-parsing exercise. A
  later pass separated *seeing a name* from *reading contents*, because they are
  different compliance questions: see
  [Session Tool Effects](../../../../architecture/session-tool-effects.md).
- **Is workspace binding part of the aggregate or part of the opaque execution
  plan, and is rebinding supported?** Part of the aggregate, and rebinding is
  not supported. The binding is immutable for the life of the session; a change
  is a new session or a fork. Recorded in
  [Session Schema Boundaries](../../../../architecture/session-schema-boundaries.md).
- **Does a provider-native escape hatch belong in a canonical catalog?** Yes,
  under a strict rule. `ProviderBlock` retains unmodelled provider blocks as
  write-verbatim, read-never data, inline or via `ArtifactRef`. Read-never is
  what keeps it an escape hatch rather than a second schema: a projection may
  not mine it. That constraint is precisely why a malformed tool intent needed
  its own typed event, in
  [Session Provider Faults](../../../../architecture/session-provider-faults.md).
- **Do we need an epoch or authority token distinct from stream sequence, or do
  we prohibit stream recreation outright?** Neither, as posed. The incarnation
  is a token in the subject, so the fence is the address rather than a checked
  value, and a retired incarnation is sealed. See
  [ADR#0057](../../../../adr/0057-session-stream-incarnation-fencing.md).
