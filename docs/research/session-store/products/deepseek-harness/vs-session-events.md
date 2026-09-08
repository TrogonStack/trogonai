# DeepSeek Harness compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [DeepSeek Harness](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on
2026-08-20. Upstream is pinned to commit
[`141eb6fef83422698aef7a981029e843e8161534`](https://github.com/deepseek-ai/deepseek-harness/tree/141eb6fef83422698aef7a981029e843e8161534),
tagged `dsh-v0.1.0-rc.8`.

**Store maturity: 5/12** - evolution scars 1/3 (the logical format remains
version `0`, limited old records are normalized in memory, SQLite is already at
physical schema `17`, and incompatible schemas are rejected rather than
migrated: [`types.ts:33-56`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L33-L56),
[`README.md:38-40`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L38-L40),
[`schema.ts:17-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/schema.ts#L17-L20),
[`schema.ts:107-149`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/schema.ts#L107-L149));
operational age 0/3 (the persistence seam first landed on 2026-06-15 in
[`df4b7d3`](https://github.com/deepseek-ai/deepseek-harness/commit/df4b7d3d9adf43bdf913e17b502f042f80379394),
only 66 days before this retrieval, entirely within the prerelease window);
exposure 1/3 (it is officially distributed through `npx`, but remains a
compatibility-breaking developer preview and its first-party stores are local,
single-owner designs:
[`README.md:9-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/README.md#L9-L23),
[`session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77));
design independence 3/3 (inference from repository history: the persistence
code was introduced as first-party DeepSeek Harness code in `df4b7d3`, with
JSONL and later SQLite implementations, not inherited from a store fork).

Because this score is under 6, a recommendation supported only by DeepSeek
Harness is labelled **thin evidence**. It is not treated as an industry norm.

## The one structural difference everything else follows from

DeepSeek Harness has two commit points. `Session.append()` synchronously commits
a fact to the process-local live session, then observers enqueue it for
write-behind persistence. A persistence `append()` acknowledges only after the
batch is durable, but failure there does not undo the already-acknowledged live
fact.
[`packages/core/session/src/index.ts:569-655`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L569-L655)
[`packages/session/session-persistence/src/write-behind.ts:18-56`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/write-behind.ts#L18-L56)

Our Session has one authoritative commit point: the durable JetStream append to
one session subject. [ADR#0035 decision 2](../../../../adr/0035-session-store-decider-aggregate.md#2-append-only-mutation-opaque-identity-ordinal-anchors-and-per-command-optimistic-concurrency)
classifies each command as `NoStream`, `At(current_position)`, or `Any`; success
means the selected durable write completed. This difference explains the rest:

- DeepSeek sequence allocation and live exclusion are process-local first, with
  backend tail checks as a second defense. Our optimistic concurrency is at the
  authoritative append boundary.
- DeepSeek repairs a cold durable log to catch it up to semantic state after a
  process crash. Our log already contains every acknowledged fact, so recovery
  is a reconciler over durable history rather than a second commit domain.
- DeepSeek can use a mutable in-memory surface and copy fork prefixes because a
  `Session` object owns the immediate view. Our projections must be deterministic
  over immutable cross-stream facts.

DeepSeek narrows the risk of its two commit points with a separate checkpoint
policy. First-party persisted runtimes flush before model dispatch, before a
top-level tool body may cause an external side effect, and before the next step.
The plugin fails those boundaries closed, but a deployment may omit it.
[`packages/session/session-checkpoint-policy/README.md:5-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-checkpoint-policy/README.md#L5-L23)
Our durable operation ordering is structural instead: `OperationReserved`
records `operation_id`, `request_digest`, and `operation_kind` before a guarded
side effect, at the same authoritative append boundary used by every command.

## Mapping

The [stage-one field table](./index.md#complete-payload-catalog-at-the-snapshot)
is the field-level source of record: it transcribes each event's complete `data`
payload and expands aliased nested values immediately below the table. The rows
here map every one of those event types and call out the payload fields that do
not map together. Physical SQLite packing adds no logical facts.
[`docs/persistence-catalog.md:1-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L1-L20)

### Header, envelope, and store mechanics

| DeepSeek Harness persisted fact | Our equivalent | Verdict |
| --- | --- | --- |
| Header `version` | Concrete protobuf type name plus `v1alpha1` package policy in `proto/trogonai/session/sessions/v1alpha1/events.proto` | Semantic mismatch. We have no per-session format integer; promotion requires a written compatibility policy. |
| Header `id` | `session_id` and the session subject | Equivalent opaque identity. |
| Header `createdAt` | No payload equivalent in `proto/trogonai/session/sessions/v1alpha1/session_started.proto` | Gap. Stream append time is not the same occurrence fact. |
| Header `cwd` | `SessionStarted.workspace` as `WorkspaceRef` | Ours is typed and portable rather than a raw local path. |
| Header `parentSession` | `ParentLinked.parent_session_id` for delegation, or `SessionForked.source_session_id` for branching | Semantic mismatch. DeepSeek overloads one header field; we distinguish delegation from fork ancestry. |
| Header `seedLength` | `SessionForked.context_prefix_boundary` | Semantic mismatch. DeepSeek counts copied child events; ours addresses an immutable source prefix by reference. |
| Header `origin` | Presence of `ParentLinked` | Ours derives the classification from a durable relationship. |
| Header `delegationDepth` | Projection over `ParentLinked` ancestry | DeepSeek stores a denormalized depth; ours can derive it and avoids drift. |
| Header `agentPreset` | `SessionStarted.execution_plan.plan_bytes` and `plan_digest` | Ours stores the complete immutable execution plan, not a preset label. |
| Event `type`, `data` | One concrete protobuf event type and its typed fields | Ours is closed and schema-validated; DeepSeek is declaration-merge extensible. |
| Event `seq` | `SessionOrdinal.value` in `proto/trogonai/session/sessions/v1alpha1/session_ordinal.proto`, fold-derived and 1-indexed | Equivalent ordering, different base and ownership. DeepSeek has no independent event id. |
| Event `time` | Append metadata, plus typed occurrence timestamps only where event time matters | Ours deliberately does not put a generic wall clock in every payload. |
| Event `ignorable` | No direct equivalent | DeepSeek lets unknown informational events be skipped. Our concrete event type and compatibility policy fail closed at the storage boundary. |
| Event `sourceEventSeqs` | Domain-specific joins and digests, such as `Compacted.covered_input_digest` | Ours rejects a generic provenance list in favor of typed evidence. |
| Event `surfaceOp` | No generic equivalent. `Compacted`, `SessionRewound`, and `RedactionApplied` alter effective model-visible history through separate typed semantics. | Semantic mismatch. DeepSeek has a general append-or-replace surface algebra; ours keeps compaction, rewind, and privacy masking distinct. |
| SQLite `incarnation`, `revision`, and JSONL source-qualified revision | JetStream stream identity and current position | Both are backend freshness values, not session facts. DeepSeek revisions protect prepare/inspect freshness, not command-level CAS. |
| Checkpoint-policy flush before model dispatch, top-level tool side effects, and the next step | Durable fact append, especially `OperationReserved` before a side effect | Same ordering goal, different enforcement. DeepSeek policy is an optional plugin over write-behind; ours makes the reservation an aggregate fact. [`checkpoint-policy/README.md:5-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-checkpoint-policy/README.md#L5-L23) |
| Projection cache domain version, unit `stateVersion`, watermark, and payload | Rebuildable projection state outside the Session stream | Equivalent. Both treat projection state as disposable. [`session-projection-cache/src/spec.ts:16-69`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/spec.ts#L16-L69) |
| FTS query rows | External read model | Equivalent. Neither makes search index state authoritative. |

### Event payload catalog

| DeepSeek Harness event type | Our equivalent | Verdict |
| --- | --- | --- |
| `agent/inbox/spliced {target, start, removedCount?, inserted, outcome?}` | No equivalent | Every payload field belongs to a mutable pending-inbox splice; our stream records admitted messages instead. [`catalog:101-122`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L101-L122) |
| `agent-preset/selected {agentPreset}` | Immutable `StoredSessionExecutionPlan` in `SessionStarted` | Semantic mismatch: changing execution identity means a new session or fork for us. [`catalog:124-140`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L124-L140) |
| `approval/asked {id, toolName, callId?, reason?}` and `approval/decided {id, outcome}` | No direct equivalent. `ToolCallApproved` and `ToolCallDenied` overlap only approved and denied tool outcomes. | DeepSeek owns a separate `ApprovalRequestId` audit lifecycle whose outcomes also include cancelled and unavailable. `ToolCallRequested` is model intent, not an approval request. [`catalog:142-183`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L142-L183) |
| `approval/policy {policy, source?}` | No typed Session event; at most an opaque policy/configuration binding inside the immutable plan | Semantic mismatch. DeepSeek can change an explicit approval-policy snapshot in one session and records delegation as the source. [`catalog:185-207`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L185-L207) |
| `assistant/chunk {turn, step, chunk}` and `assistant/message {turn, step, message, usage?, interrupted?}` | `turn_id`, `AssistantMessageStarted`, `AssistantMessageCompleted`, `AssistantMessageFailed`, and `TokenUsage`; no step number or token delta | Completed message and usage map. DeepSeek can retain a provider-visible partial message and mark it interrupted in one event; our mutually exclusive completed or failed terminal facts cannot express both, so partial interrupted content is a gap. Ours deliberately omits raw chunks. [`catalog:209-244`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L209-L244) |
| `command/run {commandId, name, args?, source}` and `command/done {commandId, kind, text?, sourceEventSeq?}` | No direct equivalent | These are slash-command handler audit facts, not tool execution. Mapping them to `ToolCallRequested` would invent model intent. [`catalog:246-287`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L246-L287) |
| `compaction/start {compactionId, sourceCommandId?, turn}`, `summary {summary, shadowedRange, shadowedSeqs, shadowedTokenCount, provider, model, maxTokens?, usage?, rawOutput?, llmStreamCall?}`, `prune {shadowedRange, shadowedSeqs, shadowedTokenCount}`, and `end {compactionId, sourceCommandId?, turn, error?}` | `Compacted` maps summary, inclusive range, model, and usage; `summary_id`, `trigger`, `context_root`, `producer`, and `covered_input_digest` are ours only | DeepSeek records the procedure and mutable replacement provenance. We record one validated result and do not retain command id, raw output, or a start/end bracket. [`catalog:289-398`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L289-L398) |
| `feedback/record {text}` | No typed equivalent | Gap. `SystemNoticeRecorded` would preserve text but lose feedback semantics. [`catalog:400-414`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L400-L414) |
| `goal/change` with version, operation, goal snapshot and timestamps, or clear tombstone | No Session event equivalent | Deliberate boundary until goal ownership is assigned; none of its durable goal identity, revision, round, or tombstone fields map. [`goal/domain.ts:13-44`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/goal/goal/src/domain.ts#L13-L44) |
| `hook/invoked {turn, point, dialect, matcher?, handlerId}` and `hook/result {turn, point, handlerId, decision, exitCode?, stderrSummary?, durationMs}` | Only `turn_id` correlates; no typed hook lifecycle | Gap if hook identity, policy decision, exit, stderr, and duration must be session audit facts. [`catalog:431-479`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L431-L479) |
| `llm/retry {retryId, turn, step, provider, mode, policyKey, retry, maxRetries?, delayMs, failure}` and `llm/retry-started {retryId, turn, step, retry}` | `turn_id` and assistant failure overlap; no model-request retry event | Execution attempts are harness attempts, not provider retries. Retry identity, policy, attempt count, delay, and transition are gaps, but this store is not evidence for copying their exact schema. [`llm-retry/types.ts:15-48`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm-retry/src/types.ts#L15-L48) |
| `permission/preset {preset}`, `plan/mode {active}`, `sandbox/mode {mode, source?}` | No typed Session events; the immutable execution plan and configuration may bind related policy | Deliberate semantic mismatch: DeepSeek mutates explicit runtime modes inside one session; our catalog fixes plan identity without naming equivalent mutable fields. [`catalog:505-538`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L505-L538) [`catalog:570-591`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L570-L591) |
| `request/context {provider, model, contextWindow?}` and `request/header {header, reason}`, where header has config, adapterDefaults?, system?, tools? | The immutable [`SessionExecutionPlan`](../../../../adr/0031-agent-implementation-and-session-plan.md) maps the resolved provider route, protocol, driver, connection, and non-secret binding; `AssistantMessageStarted` maps model and settings | Gaps are advertised context window, exact rendered prompt, effective adapter defaults, tool schemas, and initial/resume/change reason. [`types.ts:196-228`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L196-L228) |
| `schedule/change` version-1 create, delete, or dispatch over `{id, kind, prompt, scheduledAt}` plus after/every interval and dispatch time | No Session event equivalent | Deliberate boundary: none of the schedule rule, identity, or dispatch fields are part of this aggregate today. [`schedule/types.ts:9-105`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/schedule/schedule/src/types.ts#L9-L105) |
| `session/end-seed {}` | No general equivalent. `SessionForked` records the fork case only. | DeepSeek marks the end of a constructor seed for resume, fork, or replay. Our resume and replay are reads and append no lifecycle fact; fork is explicit atomic creation. [`types.ts:315-336`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L315-L336) |
| `session/title {title, messageSeqs, source}` and `session/title-llm-request {titleProvider, messageSeqs, route, system, messages, maxTokens}` | `SessionRenamed` maps the final title only | DeepSeek also retains source-message provenance and the complete derived title request; ours has no title-generation request audit. [`catalog:643-672`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L643-L672) |
| `step/start {turn, step}`, `step/end {turn, step}`, `turn/start {turn}`, `turn/end {turn, reason}` | `turn_id` on conversation and tool events; terminal message and tool facts | Turn identity maps, but our correlation does not store numeric step boundaries or one aggregate turn reason. [`catalog:674-696`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L674-L696) [`catalog:943-979`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L943-L979) |
| `subagent/descriptor {version, mode, provider, label?, agentProvider?, agentModel?, persona?, toolFilter?}` | Child `SessionStarted.execution_plan` maps composition and `ParentLinked` maps lineage; mode and label have no exact field | Ours separates executable identity from lineage. DeepSeek preserves one-shot or continuable mode and an enumeration label but has no parent-side dispatch fact. [`descriptor.ts:41-88`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/descriptor.ts#L41-L88) |
| `team/member {version, teamId, member}`, `team/task {version, teamId, task}`, `team/message/queued {version, teamId, message}`, `team/message/delivered {version, teamId, messageId, targetId}` | No Session event equivalent | The nested snapshots retain member identity, provider, context and phase; task revision, ownership, dependencies and write scopes; and mailbox sender, target, delivery and content. These are a deliberate team aggregate boundary for us. [`agent-team/types.ts:43-116`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/experimental/agent-team/src/types.ts#L43-L116) [`agent-team/types.ts:203-218`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/experimental/agent-team/src/types.ts#L203-L218) |
| `todo/write {todos[{content, status}]}` | `TodoUpdated.items[{id, content, status}]` plus `revision` | Equivalent whole-list snapshot; ours adds stable item ids and a monotonic revision. [`catalog:776-789`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L776-L789) |
| `tool/call {turn, step, callId, name, arguments}` and `tool/result {turn, step, message, error?, meta?}` | `ToolCallRequested`, `ToolCallStarted`, `ToolCallCompleted`, `ToolCallFailed` | Call identity, name, raw arguments, content, and error map. Ours adds execution id, approval, application-error status, duration, termination, artifacts, observed resources, and operation id; DeepSeek adds opaque tool presentation metadata and step number. [`catalog:791-806`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L791-L806) [`catalog:856-883`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L856-L883) |
| `tool/code-dispatch-start {rootCallId, parentCallId, subCallId, name, arguments}` and `tool/code-dispatch` with the same ids plus `isError, content` | `ToolCallRequested.parent_tool_use_id` plus nested tool lifecycle | Parent-child call correlation maps; DeepSeek also stores root identity and a specialized combined result. [`catalog:808-854`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L808-L854) |
| `tool-workflow/run-start {runId, name}`, `agent-start {runId, seq, label, phase?, childId}`, `agent-end {runId, seq, outcome}`, `run-end {runId, stopReason}` | Operation ledger and delegation facts map child dispatch and outcomes, but no workflow-run aggregate | Run identity, step sequence, label, phase, and aggregate stop reason have no direct equivalent. [`catalog:885-941`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L885-L941) |
| `user/message` with `id, role, content[], source` | `UserMessageRecorded.message` maps identity, role, and typed content; `turn_id` is ours only | DeepSeek's source attribution has no field in `CanonicalMessage`; ours adds message `created_at` and explicit turn correlation. [`catalog:981-998`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L981-L998) |
| `web/deepseek-search-llm-request {endpoint, apiVersion, body{model, max_tokens, messages, tools}}` | No provider-specific request event | Every provider request field is absent today; a generic request-envelope artifact would cover it without adding provider-specific schema. [`catalog:1000-1007`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L1000-L1007) |

### Facts only our catalog records

| Our facts | DeepSeek Harness position | Assessment |
| --- | --- | --- |
| `SessionClosed`, `SessionCancelled`, `SessionFailed`, `SessionHidden`, archive and unarchive | No durable session lifecycle or hide/archive API | Our lifecycle is materially stronger. |
| `SessionForked` and `SessionRewound` | Fork is copied header state; no rewind event or active-history pointer | Our history control is explicit and append-only. |
| `ExecutionAttemptStarted`, `ExecutionAttemptReady`, `ExecutionAttemptEnded`, `CheckpointProduced` | Process resume and projection checkpoints exist, but no durable execution-attempt aggregate or attested checkpoint | Our execution evidence is materially stronger. |
| `OperationReserved`, `OperationCancellationRequested`, `OperationOutcomeRecorded` | No general side-effect ledger | Our crash reconciliation does not infer operation identity from tool prose. |
| `DelegationDispatched`, `ParentLinked`, termination, history invalidation, and two-sided detach facts | One child header points at one parent; orderly process-local teardown drains descendants outside the durable store | Our durable lineage and cascade semantics are materially stronger. |
| `RedactionApplied`, `ArtifactErased` | No retention, redaction, or deletion API | Our decision 7 defines read-time privacy masks while retaining audit history. |
| `ArtifactRecorded`, `FileChanged`, `ResourceObservation` | Tool-private payloads can preserve some evidence, but no shared typed artifact and file ledger | Our event sizes, integrity checks, and erasure joins are stronger. |
| `ExternalDelegationDispatched` | A non-session-backed provider may leave no local child record | Our external delegation still records authorization and request evidence. |

### Semantic mismatches that must not be mapped by name alone

**Append.** DeepSeek's live append means accepted in memory; its persistence
append means durable. Our append acknowledgement is the authoritative durable
commit. A bridge must select one DeepSeek commit point explicitly.

**Fork.** DeepSeek's `seedLength` addresses physically copied child events. Our
`SessionForked.context_prefix_boundary` addresses source-stream history that the
child inherits by immutable reference. Reusing the integer without the source id
changes its meaning.

**Compaction.** DeepSeek's summary becomes active through a later generic
`surfaceOp.replace`. Our `Compacted` is itself a self-sufficient fact with exact
`covers_from`, `covers_through`, `context_root`, `producer`, and
`covered_input_digest`. A converter cannot map only the summary text.

**Revision and OCC.** DeepSeek revisions invalidate stale prepared objects, and
SQLite checks the durable tail inside a transaction. Neither is a caller-visible
expected-version command precondition. Our `At(current_position)` is.
[`packages/session/session-persistence/README.md:46-59`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L46-L59)

**Projection checkpoint.** DeepSeek projection checkpoints cache fold state.
Our `Checkpoint` is an attested, digest-bound execution checkpoint that may be
restored by a later execution attempt. The names are not interchangeable.

## What we should consider changing

### 1. Persist evidence of the exact rendered model request

- **Change:** Add an optional `ArtifactRef request_envelope_ref` to
  `proto/trogonai/session/sessions/v1alpha1/assistant_message_started.proto`,
  with canonical bytes for the rendered system prompt, tool schemas, effective
  adapter defaults, and other request-scoped envelope values not already bound
  by the immutable plan or the event. Keep `model` and `settings` queryable in
  the event and resolve route identity from the session's stored plan.
- **Evidence:** DeepSeek Harness, 5/12, persists those exact values in
  `request/header`.
  [`packages/core/session/src/types.ts:196-210`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L196-L210)
- **Blast radius:** Additive.
- **Judgment:** Consider it. This is **thin evidence** from DeepSeek alone, but
  it identifies a real reproducibility gap: plan bytes plus model settings do
  not prove which prompt and tool schemas were actually rendered for a call.
- **Cost:** One artifact write and digest verification per distinct request
  envelope, privacy filtering for prompt content, artifact retention policy,
  and a new failure mode when envelope storage succeeds or fails separately
  from model dispatch.

### 2. Add an assistant process-loss reason and safe crash-closure contract

- **Change:** Add `ASSISTANT_MESSAGE_FAILURE_REASON_PROCESS_LOST` to
  `AssistantMessageFailureReason` in `assistant_message_failed.proto`, then add
  a recovery row to the ADR command matrix and backend conformance tests. A
  reconciler should preserve every valid fact and may close an orphaned
  assistant generation with that reason. It must not map an unmatched recorded
  tool call to `ToolCallFailed`: when a reserved tool operation has no known
  outcome, append `OperationOutcomeRecorded.unknown` and reconcile it before
  claiming success or failure. Never truncate a complete fact. The current
  assistant `INTERRUPTED` value means user steering, so `ERROR` is the only
  honest existing assistant value until the new reason exists.
- **Evidence:** DeepSeek Harness, 5/12, drops only a torn final fragment and
  appends semantic closure for a cold interrupted log. Its repair distinguishes
  an assistant-declared call with no durable `tool/call` as `TOOL_NOT_STARTED`
  from a durable `tool/call` with no result whose side effect may have occurred,
  rendering the latter as `TOOL_OUTCOME_UNKNOWN` rather than declaring failure.
  [`packages/session/session-persistence/README.md:25-30`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L25-L30)
  [`packages/core/session/src/repair.ts:89-123`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/repair.ts#L89-L123)
- **Blast radius:** Additive, with no migration: one protobuf enum value plus
  new recovery-command, fold, projection, and operation-reconciliation
  behavior.
- **Judgment:** Do it. The product evidence is **thin**, but the rule follows
  our existing first-terminal-outcome fold and removes ambiguity from recovery.
- **Cost:** One enum value, a recovery detector, idempotency tests across
  repeated repair, and an operation reconciliation path that may leave the
  model-facing tool lifecycle open until the side effect is classified.

### 3. Require one OCC conformance suite for every store implementation

- **Change:** Make [ADR#0035 decision 2](../../../../adr/0035-session-store-decider-aggregate.md#2-append-only-mutation-opaque-identity-ordinal-anchors-and-per-command-optimistic-concurrency)
  acceptance criteria require each backend to pass the same `NoStream`, `At`,
  and `Any` race tests, including first creation, stale invariant transition,
  commuting terminal facts, and cross-process writers.
- **Evidence:** DeepSeek Harness, 5/12, shares a coordinator but still documents
  different concurrency limits: JSONL permits one live writer, while SQLite
  uses `BEGIN IMMEDIATE` and tail checks without an application lease.
  [`session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77)
  [`session-persistence-sqlite/src/store.ts:173-239`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/store.ts#L173-L239)
- **Blast radius:** Additive tests and release criteria.
- **Judgment:** Do it. DeepSeek alone is **thin evidence**, but it reinforces the
  stronger cross-product conclusion already recorded in the
  [stage-two synthesis](../../synthesis.md#stage-two-results-not-yet-absorbed-above).
- **Cost:** A reusable adversarial harness, at least two backend instances in
  each test, deterministic race orchestration, and blocking any backend whose
  advertised interface is stronger than its actual atomicity.

### 4. Do not replace reference forks with copied prefixes

- **Change:** Reject changing [ADR#0035 decision 5](../../../../adr/0035-session-store-decider-aggregate.md#5-fork-is-an-atomic-self-contained-creation-inheritance-is-by-explicit-reference)
  or `proto/trogonai/session/sessions/v1alpha1/session_forked.proto`
  to store the source prefix again on the child stream.
- **Evidence:** DeepSeek Harness, 5/12, copies an inclusive source prefix into
  the child's seed and records `parentSession` plus `seedLength`.
  [`packages/core/session/src/index.ts:1067-1138`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L1067-L1138)
- **Blast radius:** Breaking the decision, not the schema: decision 5, if
  adopted. No change is needed to retain the current design.
- **Judgment:** Do not do it. The copied prefix makes each child self-contained,
  but duplicates O(history) facts and makes privacy and artifact reachability
  harder to reason about. Low maturity does not justify reversing our decision.
- **Cost:** Keeping the current design requires a cross-stream context
  projection and source-prefix availability for the lifetime of the fork.

### 5. Do not generalize `Compacted` into a mutable surface operation

- **Change:** Reject a generic `surfaceOp { append | replace(start, end) }` on
  conversation events. Keep `Compacted` in
  `proto/trogonai/session/sessions/v1alpha1/compacted.proto`
  as the only compaction operation and retain its digest, producer, context
  root, and inclusive ordinal range.
- **Evidence:** DeepSeek Harness, 5/12, makes the model-visible summary active by
  appending a `user/message` with a range replacement after the summary event.
  [`packages/compaction/compaction-basic/src/region.ts:152-254`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/compaction/compaction-basic/src/region.ts#L152-L254)
  [`packages/compaction/compaction-basic/src/region.ts:426-477`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/compaction/compaction-basic/src/region.ts#L426-L477)
- **Blast radius:** Breaking the decision, not the schema: decision 4, if
  adopted. No change is needed to retain the current design.
- **Judgment:** Do not do it. DeepSeek's surface algebra is compact, but a
  generic replacement can become valid without proving which masked input was
  summarized or which plan and attempt produced it.
- **Cost:** Our typed marker is larger and makes every compaction recalculate
  `covered_input_digest`; its projection is less generic than DeepSeek's surface
  fold.

## What our design already does better

**One durable authority.** A successful command cannot exist only in process
memory. `NoStream`, `At`, and `Any` express command intent at the same boundary
that assigns order.

**Reference fork with explicit meaning.** `SessionForked` carries
`source_session_id`, inclusive `context_prefix_boundary`, and `reason`; the child
folds only its own stream. DeepSeek stores a complete copied log and a coarse
header boundary.

**Self-sufficient compaction.** `Compacted` stores the summary, exact coverage,
trigger, context root, producing attempt and plan digest, and covered-input
digest. DeepSeek's replacement records range and source sequence provenance but
does not bind the active surface replacement to equivalent privacy-masked input
or execution-plan evidence.

**Two-sided lineage and cascade.** Parent `DelegationDispatched` and child
`ParentLinked` join by `operation_id`, then typed parent termination, history
invalidation, and detach facts preserve why a child stopped or became
independent. DeepSeek has one child-to-parent header pointer.

**Privacy lifecycle on a keep-forever stream.** `SessionHidden`,
`RedactionApplied`, and `ArtifactErased` define read-time masking and content
erasure without rewriting event history. DeepSeek has no delete or retention
operation at all.

**Typed recovery and operation evidence.** Execution attempts, checkpoints, and
the operation ledger let recovery distinguish a retry from a new side effect.
DeepSeek repairs conversation shape well, but does not provide a general
side-effect reservation and reconciliation protocol.

## Trade-offs, not gaps

**Live responsiveness versus one commit boundary.** DeepSeek can update its UI
and model-visible surface without waiting for storage. It pays with an
acknowledged-live versus durable distinction. We pay durable append latency for
one unambiguous authority.

**Copied fork versus reference fork.** A DeepSeek child can be read after its
parent artifact disappears. Our child stays small and preserves one copy of the
facts, but the context projection must resolve the source prefix.

**Raw chunks versus coarse facts.** DeepSeek can replay token-level generation.
We avoid making high-volume deltas durable and accept that exact streaming
animation is not reconstructable from the Session stream.

**Mutable runtime modes versus immutable execution identity.** DeepSeek can
change presets, permissions, plan mode, and sandbox mode inside one session. We
make an execution plan immutable, which improves audit and authorization at the
cost of requiring a new session or fork for a materially different runtime.

## What not to copy

- Do not present process-local acceptance as durable success.
- Do not use copied history as the persisted meaning of fork.
- Do not let a generic surface replacement bypass compaction provenance and
  digest checks.
- Do not treat a schema-version integer as a migration system. DeepSeek's
  version `0` and SQLite schema `17` currently reject incompatibility.
- Do not mistake best-effort process-local descendant drain for durable
  cascade, or leave retention to out-of-band file or row deletion.
- Do not expose one pluggable store interface while allowing its backends to
  disagree silently on concurrency guarantees.

## The two gaps the industry has not closed

### Subagent cascade

DeepSeek Harness does not challenge [ADR#0035 decision 6](../../../../adr/0035-session-store-decider-aggregate.md#6-child-sessions-parent-first-dispatch-rewind-invalidation-distinct-from-termination-and-a-two-fact-detach-saga).
A session-backed child is an independent session with `parentSession` in its
header, and child discovery follows that pointer. During orderly process-local
shutdown, `SubagentRuntime` closes new admission, cancels continuable
descendants top-down, releases them child-first, and attempts a final flush.
That is real runtime cascade, but it appends no durable parent disposition and
does not delete child artifacts. A hard crash skips it. The persistence API has
no delete cascade, parent-termination, rewind-invalidation, or detach operation;
out-of-band parent removal can therefore still leave an orphan.
[`packages/subagent/subagent/src/child-agent.ts:85-120`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/child-agent.ts#L85-L120)
[`packages/subagent/subagent/src/index.ts:294-325`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/index.ts#L294-L325)
[`packages/subagent/subagent/src/continuation.ts:746-841`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L746-L841)
[`packages/subagent/subagent/src/continuation.ts:1332-1394`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L1332-L1394)
[`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240)

Result: DeepSeek validates top-down cancel plus child-first release as an
orderly runtime policy, but decision 6 remains necessary for durable recovery.
Its answer is worse after hard crash, delete, or rewind, and has no analog to
history invalidation.

### Retention on an unbounded log

DeepSeek Harness separates transcript compaction from storage compaction, which
supports one premise of [ADR#0035 decision 7](../../../../adr/0035-session-store-decider-aggregate.md#7-the-log-is-never-truncated-keep-forever-with-a-read-time-redaction-and-erasure-contract):
model-visible replacement should not silently delete audit history. It offers no
answer to growth, however. JSONL files and SQLite rows accumulate; there is no
delete, retention, or background historical compaction API.
[`session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77)
[`session-persistence-sqlite/README.md:55-63`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/README.md#L55-L63)

Result: the product validates the distinction between context compaction and
physical retention, but neither validates nor challenges our read-time
redaction and artifact-erasure contract. Its unbounded-growth answer is worse.

## Open questions for the ADR

- Must `AssistantMessageStarted` commit to the exact rendered model request, or
  is immutable plan identity plus `model` and `settings` intentionally enough?
- Which durable fact proves that a recovery reconciler, rather than a live
  writer, closed an interrupted message or tool call?
- What exact compatibility artifact is required when `v1alpha1` becomes `v1`:
  a per-stream format marker, a type-registry version, explicit migration
  events, or only stable concrete type names and readers?
- Must every future store backend support cross-process writers, or may a local
  backend advertise a weaker capability while still implementing the same
  interface?
