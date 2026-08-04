# Claude Agent SDK 0.3.220 session type snapshot and platform comparison

Part of [Session Store Research](../index.md). This is an immutable inspection
snapshot of the published `@anthropic-ai/claude-agent-sdk` npm tarball, not a
rolling description of the package. A later SDK release should get a new
snapshot or an explicitly dated addendum.

## Snapshot identity

The npm registry and tarball were inspected on 2026-08-03. Package facts in
this document come from the published declarations and bundled runtime. They
are separate from adapter recommendations, which are labeled as inference.

This comparison does not define the platform Session Store. The platform owns
its Session schema and harness loop independently. Any later Claude integration
would translate at the edge and cannot introduce Claude identities, transcript
layouts, bridge positions, or resume semantics into the core contract.

| Fact | Snapshot value |
| --- | --- |
| Package version | `0.3.220` |
| Bundled Claude Code version | `2.1.220` |
| Dist-tags | `latest` and `next` both `0.3.220`; no `beta` tag present |
| Registry metadata retrieval | `2026-08-03T21:30:09Z` |
| Compressed size | `1,144,946` bytes |
| Unpacked size | `4,258,606` bytes, 15 files |
| SHA-1 | `c59cf4fff0166d2a04b01470eabdd4a792add48d` |
| SHA-512 | `82573b49dc0f90e90bc3ca31c0ba3d3ca4dd2c91aa5bf3c8478bab59716846d5fd625970a33b0455ce53735f84bcb4a47ebb313c9a905aa3a0a6350e4177f5a0` |
| npm integrity | `sha512-glc7SdwPkOkLw8oxwLo9PKTdLJGqW/PIR4urWXFoRtX9YllwozsEVc5Tc1+EvLSkfrsxPJqQWqOgpjUOQXf1oA==` |

Pinned sources: [npm registry metadata](https://registry.npmjs.org/@anthropic-ai%2fclaude-agent-sdk/0.3.220),
[`sdk.d.ts`](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.220/sdk.d.ts),
[`browser-sdk.d.ts`](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.220/browser-sdk.d.ts),
[`bridge.d.ts`](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.220/bridge.d.ts), and
[`sdk-tools.d.ts`](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.220/sdk-tools.d.ts).

This snapshot complements the broader
[Claude Agent SDK and Claude Code dossier](./claude-agent-sdk.md), which covers
documented and observed storage behavior. This page is narrower: it freezes
the complete session-related TypeScript surface in this one release and tests
it against the platform Session contract in this checkout.

## Authority boundary

- [ADR#0024](../../../adr/0024-agent-platform-stream-topology.md) is accepted.
  It authoritatively requires Sessions to pin the Agent revision they start on
  and gives the placement rule for ordered facts.
- [ADR#0025](../../../adr/0025-agent-definition-data-ownership.md),
  [ADR#0031](../../../adr/0031-agent-implementation-and-session-plan.md), and
  [ADR#0035](../../../adr/0035-session-store-decider-aggregate.md) are drafts.
  So are [ADR#0032](../../../adr/0032-model-route-and-credential-binding.md)
  and [ADR#0043](../../../adr/0043-agent-instructions-ownership-and-shape.md).
  They express current design directions, not accepted architecture.
- Draft [ADR#0031](../../../adr/0031-agent-implementation-and-session-plan.md)'s
  platform-readable model fields are contested. Draft
  [ADR#0032](../../../adr/0032-model-route-and-credential-binding.md) says that
  ownership precondition is currently unmet. Draft
  [ADR#0043](../../../adr/0043-agent-instructions-ownership-and-shape.md) keeps
  instruction content and injection shape in runtime-owned settings.
- The
  [`v1alpha1` Session event package](../../../../proto/trogonai/session/sessions/v1alpha1/events.proto)
  is real code in this checkout, but its own header says it depends on draft
  ADRs and cannot promote to `v1` until those decisions and prerequisites are
  settled.
- Every statement beginning with **Inference** below is a proposed integration
  rule. It is not a fact about the SDK and not an accepted platform decision.

## Export topology

**Package fact.** The package has no unified `Session` class or interface.
Session behavior is split across execution, live messages, transcript
utilities, a storage adapter, browser transport, and bridge transport.

| Package subpath | Declaration | Session relevance |
| --- | --- | --- |
| `.` | `sdk.d.ts` | `Options`, `Query`, `SDKMessage`, history operations, hooks, `SessionStore` |
| `./browser` | `browser-sdk.d.ts` | Restricted remote `query()` over WebSocket or SSE |
| `./bridge` | `bridge.d.ts` | Remote worker attachment, epoch, sequence, state, and delivery reporting |
| `./sdk-tools` | `sdk-tools.d.ts` | Tool schemas with subagent, workflow, cron, and remote-session fields |
| `./sdk-tools.js` | `sdk-tools.d.ts` | Alias of the same type-only surface |
| `./extract` | `extractFromBunfs.d.ts` | Package extraction helper, not a Session contract |

`agentSdkTypes.d.ts` is shipped but is not an exported package subpath. It
re-exports the root declarations for the browser and bridge build. Root
`export declare` names are importable. Unexported `declare type` names remain
private even when an exported wrapper refers to them.

## Conceptual model

**Package fact.** A root session is a native transcript identity plus a live
`Query` process and control channel. History utilities operate on that
transcript; `SessionStore` mirrors it; browser assumes an external Session
service; bridge attaches a remote worker; tool schemas add adjacent tasks and
workflows. These surfaces share IDs in places, but no one exported object owns
their identity, durable state, execution, and lifecycle together.

## Root execution surface

### `Options`

**Package fact.** All 63 `Options` fields are grouped below. The
grouping is ours; the names and types are from the tarball.

| Concern | Fields |
| --- | --- |
| Process and cancellation | `abortController`, `cwd`, `additionalDirectories`, `env`, `executable`, `executableArgs`, `extraArgs`, `pathToClaudeCodeExecutable`, `spawnClaudeCodeProcess`, `stderr` |
| Native agent and tools | `agent`, `agents`, `allowedTools`, `disallowedTools`, `toolAliases`, `tools`, `toolConfig`, `canUseTool`, `skills`, `plugins`, `hooks`, `includeHookEvents` |
| Models and limits | `model`, `fallbackModel`, `thinking`, `effort`, `maxThinkingTokens`, `maxTurns`, `maxBudgetUsd`, `taskBudget`, `betas`, `outputFormat` |
| Session selection | `continue`, `resume`, `resumeSessionAt`, `forkSession`, `sessionId`, `title` |
| Persistence | `persistSession`, `sessionStore`, `sessionStoreFlush`, `loadTimeoutMs`, `enableFileCheckpointing` |
| Permissions and isolation | `permissionMode`, `planModeInstructions`, `allowDangerouslySkipPermissions`, `permissionPromptToolName`, `sandbox` |
| MCP and interaction | `mcpServers`, `strictMcpConfig`, `onElicitation`, `onUserDialog`, `supportedDialogKinds`, `promptSuggestions`, `agentProgressSummaries` |
| Streaming and configuration | `includePartialMessages`, `forwardSubagentText`, `systemPrompt`, `settings`, `managedSettings`, `settingSources` |
| Diagnostics | `debug`, `debugFile` |

Session-specific semantics are important. `continue` chooses the newest
conversation for the working directory. `resume` selects an ID.
`resumeSessionAt` limits resume through an assistant-message ID. `forkSession`
changes a resume into a new native session. `sessionId` supplies a custom UUID
for a new or forked session. `persistSession: false` prevents later resume.
`title` applies only at creation.
The store key calls `projectKey` caller-defined, but root `Options` cannot set
it directly. Query-backed store use derives it from cwd.

### `Query`

**Package fact.** `query({prompt, options})` returns a `Query`, which is an
`AsyncGenerator<SDKMessage, void>` plus all 27 declared methods:

| Concern | Methods |
| --- | --- |
| Execution | `interrupt`, `streamInput`, `close` |
| Live mutation | `setPermissionMode`, `setMcpPermissionModeOverride`, `setModel`, `setMaxThinkingTokens`, `applyFlagSettings` |
| Initialization and discovery | `initializationResult`, `reinitialize`, `supportedCommands`, `supportedModels`, `supportedAgents`, `accountInfo` |
| Context and usage | `getContextUsage`, `usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET` |
| Files and plugins | `readFile`, `rewindFiles`, `seedReadState`, `reloadPlugins`, `reloadSkills` |
| MCP | `mcpServerStatus`, `reconnectMcpServer`, `toggleMcpServer`, `setMcpServers` |
| Tasks | `stopTask`, `backgroundTasks` |

`reinitialize()` is transport-gap recovery, not transcript replay. It sends a
fresh initialize request and can redeliver pending permission and dialog
requests by `request_id`. `startup()` returns a one-use `WarmQuery`, whose
`query()` hands back the normal `Query` after prewarming a subprocess.
`close()` terminates query process resources; it is not a domain
`SessionClosed` operation.

**Inference.** Most live mutation methods cannot be exposed directly in a
verified platform Session. Model, permission, MCP, plugin, skill, tool, and
settings changes must be rejected or converted into explicit platform
commands whose policy preserves the pinned plan.

## Root session management

| Export | Package behavior |
| --- | --- |
| `listSessions`, `SDKSessionInfo` | Lists ID, summary, modified time, and optional size, title, first prompt, branch, cwd, tag, and creation time |
| `getSessionInfo` | Reads one session summary locally or through a store |
| `getSessionMessages`, `SessionMessage` | Returns the linked message chain with optional system messages, pagination after chain materialization |
| `listSubagents`, `getSubagentMessages` | Enumerates native subagent IDs and reads one native child transcript |
| `forkSession` | Copies through an optional message ID, remaps message UUIDs, preserves links, and omits undo history |
| `renameSession`, `tagSession` | Appends native metadata entries |
| `deleteSession` | Deletes local transcript state or calls optional `SessionStore.delete`; absent store deletion is a no-op |
| `importSessionToStore` | Replays local main and optional subagent transcripts to `append` in batches |
| `foldSessionSummary` | Folds opaque entries into SDK-owned summary data; set-once and last-write-wins fields differ |
| `InMemorySessionStore` | Full non-production implementation with test helpers |
| `SessionMutationOptions` | Selects local `dir` or `sessionStore` for rename, tag, delete, and fork |
| `SessionStartHookInput`, `SessionStartHookSpecificOutput` | Reports start source; can add context or input, title, watch paths, and skill reload |

Named companions are `ListSessionsOptions`, `GetSessionInfoOptions`,
`GetSessionMessagesOptions`, `ListSubagentsOptions`,
`GetSubagentMessagesOptions`, `ImportSessionToStoreOptions`,
`ForkSessionOptions`, and `ForkSessionResult`. They cover selection, paging,
filters, import batching, and fork output. `SessionEndHookInput` carries
`ExitReason`; `SessionCronSummary` carries ID, schedule, recurrence, and prompt.
`SessionMessage` exposes only `type`, `uuid`, `session_id`, opaque `message`,
`parent_tool_use_id`, and `parent_agent_id`; history content remains unknown.

There is no public replay command, replay cursor, expected-position token,
lease, lock, transaction, or Session lifecycle state machine in this surface.
`SDKUserMessageReplay` is an observed output shape, not a replay operation.

## Complete `SDKMessage` inventory

The public union contains exactly 39 named members. Some expand into several
wire states; `SDKResultMessage`, for example, has success and multiple error
subtypes. The treatment column is our adapter classification:

- **D** means translate a stable domain fact into one or more typed platform
  events after validation and correlation.
- **L** means publish as live telemetry or a projection signal, but do not
  append it as a Session domain fact by itself.
- **P** means prohibit the autonomous native behavior in a verified Session.
  A platform command may provide an allowed replacement.
- **A** means adapter-owned only. Raw `SessionStoreEntry` or JSONL, SDK summary
  sidecars, native session, message, subagent IDs and subpaths, and bridge
  sequence, epoch, and cursor support recovery or correlation, but are not
  platform domain facts.

| # | Variant and wire discriminator | Key payload | Treatment |
| ---: | --- | --- | --- |
| 1 | `SDKAssistantMessage`, `assistant` | provider message, native IDs, error, request, replacement and abort metadata | D, assemble before `AssistantMessageCompleted` or `AssistantMessageFailed` |
| 2 | `SDKUserMessage`, `user` | user or tool-result content, origin, priority, synthetic and query flags | D for new input or tool outcome, after origin and content classification |
| 3 | `SDKUserMessageReplay`, `user` plus `isReplay: true` | required native message and session IDs, optional attachments | A; consume only for native context reconstruction and never re-append |
| 4 | `SDKResultMessage`, `result` | success or typed query error, usage, cost, denials, terminal reason | D as turn or attempt outcome, never automatically `SessionClosed` |
| 5 | `SDKSystemMessage`, `system/init` | effective model, tools, cwd, permissions, MCP, skills, plugins, capabilities | D only after Ready verification against the pinned plan |
| 6 | `SDKPartialAssistantMessage`, `stream_event` | raw provider stream event and time to first token | L |
| 7 | `SDKCompactBoundaryMessage`, `system/compact_boundary` | trigger, token counts, preserved-message linkage | D when source boundaries and summary are made self-sufficient |
| 8 | `SDKStatusMessage`, `system/status` | compacting or requesting state and compact result | L |
| 9 | `SDKAPIRetryMessage`, `system/api_retry` | attempt, delay, status, error category | L |
| 10 | `SDKControlRequestProgressMessage`, `system/control_request_progress` | request ID and started or retry status | L |
| 11 | `SDKModelRefusalFallbackMessage`, `system/model_refusal_fallback` | original and fallback models, replacement lineage | P because autonomous model substitution violates the draft plan rule |
| 12 | `SDKModelRefusalNoFallbackMessage`, `system/model_refusal_no_fallback` | model, request, refusal detail | D as typed refusal outcome |
| 13 | `SDKLocalCommandOutputMessage`, `system/local_command_output` | display text | D only when it is model-visible or audit-relevant, otherwise L |
| 14 | `SDKHookStartedMessage`, `system/hook_started` | hook identity and event | L |
| 15 | `SDKHookProgressMessage`, `system/hook_progress` | hook output streams | L |
| 16 | `SDKHookResponseMessage`, `system/hook_response` | output, exit, success, error, or cancellation | D for decision-bearing outcome, otherwise L |
| 17 | `SDKPluginInstallMessage`, `system/plugin_install` | install state and optional failure | P for mid-Session dependency mutation |
| 18 | `SDKToolProgressMessage`, `tool_progress` | tool IDs, elapsed time, heartbeat and retry | L |
| 19 | `SDKAuthStatusMessage`, `auth_status` | in-progress output and error | L |
| 20 | `SDKTaskNotificationMessage`, `system/task_notification` | native task terminal status, output file, summary, usage | P until native tasks map one-for-one to platform operations or child Sessions |
| 21 | `SDKTaskStartedMessage`, `system/task_started` | task ID, type, description, prompt, subagent or workflow metadata | P until child or external delegation admission exists |
| 22 | `SDKTaskUpdatedMessage`, `system/task_updated` | mutable task status patch | P for hidden native task state |
| 23 | `SDKTaskProgressMessage`, `system/task_progress` | description, usage, last tool, summary | L only for an already admitted platform operation |
| 24 | `SDKBackgroundTasksChangedMessage`, `system/background_tasks_changed` | replace-set of process-local tasks | L only for admitted operations; otherwise P |
| 25 | `SDKThinkingTokensMessage`, `system/thinking_tokens` | approximate running estimate | L |
| 26 | `SDKSessionStateChangedMessage`, `system/session_state_changed` | `idle`, `running`, or `requires_action` | L as liveness, not durable lifecycle |
| 27 | `SDKWorkerShuttingDownMessage`, `system/worker_shutting_down` | host reason | L as live-tail signal, never Session terminal state |
| 28 | `SDKCommandsChangedMessage`, `system/commands_changed` | replacement command list | P for unplanned command-surface mutation |
| 29 | `SDKNotificationMessage`, `system/notification` | keyed text, priority, color, timeout | L |
| 30 | `SDKFilesPersistedEvent`, `system/files_persisted` | persisted and failed files | D after claim-check and workspace attribution |
| 31 | `SDKToolUseSummaryMessage`, `tool_use_summary` | summary and preceding tool IDs | L as a derived view |
| 32 | `SDKMemoryRecallMessage`, `system/memory_recall` | mode and recalled sources or content | D if it entered model context, with source normalization and claim-checks |
| 33 | `SDKRateLimitEvent`, `rate_limit_event` | current allowance and reset data | L |
| 34 | `SDKElicitationCompleteMessage`, `system/elicitation_complete` | MCP server and elicitation IDs | D when joined to an admitted tool operation |
| 35 | `SDKPermissionDeniedMessage`, `system/permission_denied` | tool, input linkage, typed and human reasons | D to `ToolCallDenied` without parsing reason text |
| 36 | `SDKPromptSuggestionMessage`, `prompt_suggestion` | predicted next prompt | L |
| 37 | `SDKMirrorErrorMessage`, `system/mirror_error` | failed store key and error | D as integrity failure if mirroring is enabled; production authority must not depend on it |
| 38 | `SDKInformationalMessage`, `system/informational` | text, level, optional stop flag | D when it changes execution or model-visible history, otherwise L |
| 39 | `SDKConversationResetMessage`, `conversation_reset` | new native conversation ID | P; map clear or reset intent to explicit platform rewind, fork, or new Session semantics |

`SDKActiveGoalMessage` is exported and appears in the private `StdoutMessage`
union, but is absent from public `SDKMessage`. Runtime 0.3.220 nevertheless
forwards `active_goal` into `Query`. It is a version-specific 40th observable
Query payload and a declaration defect, not a stable `SDKMessage` member. Its
single goal condition, iteration state, timestamps, token baseline, and
optional reason have no exact platform event match. Keep it adapter or live
state, or add a distinct typed domain event only if durability is required.

## `SessionStore`

**Package fact.** `SessionStore` is an alpha mirror adapter, not a replacement
for Claude Code's local JSONL authority.

| Type or method | Contract in 0.3.220 |
| --- | --- |
| `SessionKey` | `projectKey`, `sessionId`, and optional opaque `subpath` |
| `append` | Required, receives opaque JSON-safe batches after local write succeeds |
| `load` | Required, returns the complete transcript or `null` before resume |
| `listSessions` | Optional ID and adapter-clock modification time listing; required by store-backed `continue` |
| `listSessionSummaries` | Optional bulk SDK-owned summary sidecar listing |
| `delete` | Optional, main-key deletion must cascade to subkeys and summary |
| `listSubkeys` | Optional, required to restore subagent transcripts |
| `SessionStoreEntry` | Only `type`, optional `uuid`, optional `timestamp`, and opaque JSON fields are public |
| `SessionStoreFlush` | `batched` or `eager` |
| `SessionSummaryEntry` | Session ID, adapter-clock `mtime`, and opaque SDK-owned `data` |

Runtime inspection confirms these constraints: `sessionStore` cannot combine
with `persistSession: false`; store-backed `continue` needs listing; file
checkpointing cannot combine with `sessionStore`; default load timeout is 60
seconds; and `continue` chooses the newest `mtime`.

The mirror retries rejected appends up to three total attempts. Timed-out
calls are not retried because they might still land. Final failure drops the
batch and emits `mirror_error` while the query continues. Adapters are expected
to deduplicate UUID-bearing entries. Entries without UUIDs are append-only.
Resume loads the entire transcript and materializes temporary local JSONL.
There is no range read, cursor, version, compare-and-swap token, or atomic
append precondition.
`foldSessionSummary` is pure, so the adapter must serialize or transaction/CAS
its own summary-sidecar read-fold-write. That responsibility does not add a
compare-and-swap token to the transcript contract.

**Inference.** If a Claude integration is added later, it must treat
`SessionStoreEntry` as opaque product recovery material in an edge-owned store.
It cannot be the authoritative platform Session event stream, because
successful platform commands cannot depend on a best-effort secondary write
that may be dropped.

## Shipped private control declarations

**Package fact.** Public `SDKControlRequest` and `SDKControlResponse` wrappers
refer to non-exported request and response unions. Consumers can narrow the
objects at runtime, but cannot import the member type names.

The private request union covers `interrupt`, `can_use_tool`, `initialize`,
`set_permission_mode`, `set_model`, `set_max_thinking_tokens`,
`rename_session`, `set_color`, `mcp_status`, `get_context_usage`,
`get_session_cost`, `list_models`, `get_usage`, `get_binary_version`,
`mcp_call`, `file_suggestions`, `hook_callback`, `mcp_message`,
`rewind_files`, `cancel_async_message`, `read_file`, `get_workspace_diff`,
`get_plan`, `seed_read_state`, `mcp_set_servers`, `register_repo_root`,
`reload_plugins`, `reload_skills`, `mcp_reconnect`, `mcp_toggle`, `stop_task`,
`background_tasks`, `apply_flag_settings`, `get_settings`, `elicitation`, and
`request_user_dialog`. Private success and error envelopes can also carry
pending permission and dialog requests during initialize recovery.

The private `StdoutMessage` union adds `SDKActiveGoalMessage`, control request,
control response, control cancellation, and keepalive to public `SDKMessage`.
This proves that the typed public iterator is narrower than the shipped wire.

**Inference.** A future Claude edge adapter needs an exhaustive control-channel
allowlist.
Read-only discovery may pass through. Permission asks must bind to the
platform tool operation. Model, settings, MCP topology, rewind, plugin, skill,
task, and rename mutations require platform command authority or rejection.

## Browser, bridge, and tool-schema surfaces

### `./browser`

The browser export accepts exactly one transport: WebSocket, or preferred SSE.
Its query options contain prompt stream, abort, tool permission callback,
hooks, MCP servers, output schema, elicitation, dialogs, and prompt
suggestions. It does not expose root options for resume, continue, fork,
custom session ID, title, persistence, `SessionStore`, cwd, or model. SSE takes
an externally created session ID. **Inference:** browser mode presupposes an
external Session service and is a client transport, not our aggregate model.
The declaration example omits required `SSEOptions.sessionId`, another seam
consumers must not copy literally.

### `./bridge`

`BridgeSessionHandle` has a remote session ID, live SSE high-water sequence,
worker epoch, connection status, message and result writes, control forwarding,
transport reconnect, state and metadata reporting, delivery reporting, flush,
and close. Attach options add inbound messages, permission responses,
interrupt, live model, thinking, permission, and title controls.

Bridge sequence resumes transport frames, not transcript history. Epoch fences
the active worker, but neither value is a platform Session ordinal or event
version. The declaration also says bridge alpha stability is a separate
versioning universe from root `query()`.

### `./sdk-tools`

These are tool schemas, not the root lifecycle API. Session-adjacent fields
include a remote agent `sessionUrl`; subagent fork model inheritance and
session permission inheritance; same-session workflow `resumeFromRunId`;
durable cron jobs that survive Sessions; workflow run IDs and remote session
URLs and `transcriptDir`; replayed notification timestamps; and `Monitor`
persistent mode, which runs until `TaskStop` or Session end. **Inference:** a
URL or transcript directory is not platform identity, and native fork, cron,
or monitor ownership cannot silently become platform Session semantics.

## Platform Session in this checkout

**Repository fact.** The current code has 57 Session proto files, 72 messages,
19 enums, and 41 concrete arms in
[`SessionEvent`](../../../../proto/trogonai/session/sessions/v1alpha1/events.proto).
It has generated Rust exports, codecs, and local per-event semantic validation.
`validate_session_event` checks only facts one event can prove about itself;
cross-event joins and stream invariants are explicitly out of scope. Current
non-test code only re-exports it, while its call sites are the validation tests.
The catalog covers lifecycle, fork, rewind, compaction, conversation, tools,
artifacts, files, execution attempts, delegation, operation ledger, privacy,
system notices, todo state, and organization metadata.

[`SessionStarted`](../../../../proto/trogonai/session/sessions/v1alpha1/session_started.proto)
stores one
[`StoredSessionExecutionPlan`](../../../../proto/trogonai/session/sessions/v1alpha1/execution_plan.proto),
currently opaque canonical `plan_bytes` plus a digest, and a workspace
reference. The concrete typed plan described by draft
[ADR#0031](../../../adr/0031-agent-implementation-and-session-plan.md) is not
yet a proto contract in this package.

**Repository fact.** No Session-specific commands, decider `initial_state`/
`evolve`/`decide`, composed event store and subject resolver, projection,
snapshot policy, reconciler, Claude adapter, or adapter conformance suite
exists yet. Generic infrastructure does exist: the decider runtime supplies
write preconditions and snapshots, while its NATS crate supplies a JetStream
stream store, optimistic concurrency, snapshot storage, and projector
primitives. Draft
[ADR#0035](../../../adr/0035-session-store-decider-aggregate.md) leaves their
Session-specific composition and its listed substrate corrections as follow-up
work.

## Comparison matrix

| Axis | SDK 0.3.220 | Platform Session direction | Deferred edge rule |
| --- | --- | --- | --- |
| Aggregate | No unified aggregate | One logical event-sourced aggregate per Session in draft [ADR#0035](../../../adr/0035-session-store-decider-aggregate.md) | Adapter must not equate `Query` or JSONL with aggregate state |
| Identity | cwd-derived project key, native UUID, optional subpath | Opaque Session ID and subject resolution | Keep any product identity binding at the edge, never reuse one ID domain as the other |
| Revision and plan | Model, tools, settings, plugins, and agents can be selected or mutated | Accepted revision pin; draft immutable execution plan | Freeze admitted native projection and reject unplanned mutation |
| Authority | Local JSONL first, optional best-effort mirror | Typed authoritative event log | Store native recovery bytes separately |
| Ordering | JSONL order, UUID links, bridge sequence | Logical `SessionOrdinal`; physical sequence is not domain identity | Keep bridge and transcript positions outside core Session ordering |
| Concurrency | Same native session can receive interleaved writers | Draft `NoStream`, `At`, and `Any` preconditions by command | Platform owns command concurrency and invariants |
| Idempotency | UUID dedupe recommendation; some entries lack UUID | Draft deterministic event IDs and operation ledger | Bind native IDs to stable platform operation IDs and request digests |
| Resume | Full transcript load and temporary JSONL | Harness recovery is platform-owned | Product resume stays an edge concern and cannot define core recovery fields |
| Replay | Output flag only | Read and fold of durable events | Never append replayed SDK messages again |
| Fork | Physical copy with remapped UUIDs and no undo history | Draft new Session with source-prefix reference | Implement platform fork first, then materialize a native transcript view |
| Rewind | Native file rewind and resume-at-message | Draft append-only `SessionRewound` plus read-time history invalidation | Convert explicit command, do not truncate platform history |
| Compaction | Native boundary plus preserved-message links | Draft self-sufficient in-stream `Compacted` marker | Capture summary, exact source range, prompt rules, and digest |
| Assistant stream | Partial and full provider messages plus result boundary | Coarse started, completed, or failed facts; no token-delta events | Stream deltas live, assemble one canonical durable outcome |
| Tool authorization | Callback and control request, plus auto-denial event | Typed request, approval or denial, execution, and operation ledger | Platform authorizes and dispatches before the native loop observes a result |
| Subagents | Native task IDs and subpath transcripts | Draft one child Session per admitted delegation | Intercept native spawn or disable it |
| External delegation | Remote task and session URL shapes | Draft typed external delegation operation | Record authenticated destination, authorization, request digest, and outcome |
| Liveness | `idle`, `running`, `requires_action`; task and worker signals | ExecutionAttempt and Session lifecycle facts | Keep transient state out of durable terminal fold |
| Listing | Local scan or optional summary sidecar | Rebuildable projection | Build platform picker from events, not SDK summaries |
| Deletion | Physical local delete or optional store delete | Draft keep-forever log, hide, redaction, artifact erasure | Do not route SDK delete to event-log deletion |
| Checkpoints | File-history blobs cannot use `SessionStore` | Platform harness checkpoint is independent | Keep product recovery artifacts outside the core checkpoint schema |
| Browser and bridge | External remote service, transport sequence, worker epoch | Aggregate identity, event ordinal, execution attempt | Treat as transport and hosting facts only |
| Privacy and tenancy | cwd-oriented scope, opaque transcript payload | Resolver and authorization decisions remain separate | Never encode tenant or authority from a filesystem key |

## Deferred integration questions

These questions matter only if Claude becomes an integration target. They are
not gaps in the platform-owned Session Store or harness design.

1. Define an edge-owned binding among the platform Session, native UUID,
   execution attempt, TurnId, project key, subpath, bridge identity, epoch, and
   cursor. Hook `prompt_id` is not uniformly present on `SDKMessage`, so turn
   correlation needs an explicit integration rule.
2. Specify an exhaustive native message and control translation contract,
   including assembly, replay dedupe, refusal, clear, compact, and result
   semantics. `SDKResultMessage` ends a turn or attempt, not a Session;
   approvals arrive through callbacks or control, not the output iterator.
3. Build immutable native configuration projection and verification. Disable
   fallback model selection, dynamic tools, hidden subagents, and mid-Session
   dependency mutation unless a platform command explicitly authorizes them.
4. Persist opaque native transcript and recovery material outside typed
   platform events, harness checkpoints, and projections.
5. Put tool, model, delegation, and external side effects behind stable
   operation IDs, request digests, authorization, and reconciliation.
6. Define product restart behavior without turning native resume coordinates
   into core Session fields.
7. Add conformance tests for multi-host resume, retry, duplicate delivery,
   control re-delivery, crash windows, native fork materialization, rewind,
   compaction, and child-session recovery.

## Recommendation

**Inference.** Build the platform Session Store and harness loop from the
platform's own domain first. This SDK snapshot is comparison material, not a
reason to shape the core schema around Claude sessions.

If Claude support is chosen later, implement it as an edge translation with
its own identity, transcript, control, and recovery state. Its conformance tests
must prove that native behavior maps into existing platform commands and events
without adding product-specific fields to the Session contract.
