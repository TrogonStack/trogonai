# Genkit Agents Research: Gap Analysis for TrogonAI

Research into Google's Genkit Agents API announcement and what it means for this platform.

Sources:

- Blog post: [Build agentic full-stack apps with Genkit](https://developers.googleblog.com/build-agentic-full-stack-apps-with-genkit/)
- Docs: [Full-stack agents overview](https://genkit.dev/docs/agents/overview/), [State](https://genkit.dev/docs/agents/state/), [Interrupts](https://genkit.dev/docs/agents/interrupts/), [Background work](https://genkit.dev/docs/agents/background/), [Multi-agent](https://genkit.dev/docs/agents/multi-agent/), [Session stores](https://genkit.dev/docs/agents/session-stores/)
- Genkit canonical wire schema: [`js/ai/src/agent-types.ts`](https://github.com/firebase/genkit/blob/main/js/ai/src/agent-types.ts) (main branch, July 2026); this file is the single home for the agent/session wire types, mirrored in `genkit-tools/common/src/types/agent.ts`
- Internal baseline: ADRs 0001-0015 (`docs/adr/`), `rsworkspace/` crate survey (95 crates, branch `platform`), plus field-level extraction of the session, decider, artifact, and ACP data models cited inline below

Contents:

1. [What Google announced](#1-what-google-announced)
2. [Positioning: framework vs platform](#2-positioning-framework-vs-platform)
3. [Capability-by-capability comparison](#3-capability-by-capability-comparison)
4. [Gaps in detail](#4-gaps-in-detail)
5. [Partial situations in detail](#5-partial-situations-in-detail)
6. [Data model comparison](#6-data-model-comparison)
7. [Where we are ahead](#7-where-we-are-ahead-keep-investing-do-not-chase-genkit)
8. [Recommendation summary](#8-recommendation-summary)

## 1. What Google announced

Genkit (open-source, TypeScript/Go in preview, Dart/Python following) added an **Agents API** that packages the model loop, message history, tool calls, streaming, and persistence behind one interface:

- **`defineAgent()`**: an agent is a name + system prompt, extended with tools, typed custom state, and a session store. One interface covers one-shot replies, streamed turns, paused tool calls, and multi-turn conversation.
- **Snapshots and sessions**: the server persists messages, custom state, and artifacts as snapshots. Clients resume by `sessionId` (latest state) or branch from any historical point by `snapshotId`. Snapshot statuses: `completed`, `pending`, `failed`, `aborted`, `expired`; only `completed` is a valid resume point. A client-managed mode exists where the server returns full `SessionState` and the client sends it back each turn.
- **Custom state + artifacts**: typed application data (workflow status, task lists) updated via `updateCustom(fn)` and streamed to clients as JSON Patch chunks; named artifacts (reports, patches, files) with stable identity, replaced on same-name writes, stored separately from chat history.
- **Interrupts (human-in-the-loop)**: `defineInterrupt()` or conditional `ctx.interrupt(metadata)` inside a tool pauses the turn with finish reason `interrupted`. Two resume patterns: `respond()` (client supplies the tool output, tool does not re-run; approval flows) and `restart()` (tool re-runs with updated context, original inputs preserved). The runtime validates resume payloads against session history to prevent forging.
- **Background turns**: `detach()` returns a pending snapshot ID immediately; the agent keeps running server-side and writes progress into the pending snapshot. Clients `poll()`, `wait()`, or `abort()` without holding a connection, and reconnect later via `getSnapshot()`. Requires a session store that supports status subscriptions.
- **Multi-agent orchestration**: an `agents()` middleware injects `delegate_to_<agentName>` tools into an orchestrator. Delegation shows up as ordinary tool activity in the stream. Options: `historyLength`, `maxDelegations`, `artifactStrategy` (`session` merges sub-agent artifacts into the parent session, namespaced by invocation). Sub-agent failures and interrupts return as tool output, not top-level errors.
- **HTTP serving + client SDK**: route helpers mount an agent as three endpoints (turn handler, `getSnapshotDataAction`, `abortAgentAction`). A JS `remoteAgent({url})` gives browsers/mobile the exact same `chat()` / `sendStream()` interface as a local agent, with per-request auth headers, streamed state patches, and session continuation. A `@genkit-ai/vercel-ai` adapter plugs agents into Vercel's `useChat` and AI Elements.
- **Session stores**: in-memory, file, Firestore, or custom via a small store interface (snapshot read/write, optional `SnapshotSubscriber` for background/abort support).
- **Dev UI Agent Runner**: browser tool to run conversations, watch streamed state, drive interrupts, and inspect snapshots before any frontend exists.
- Google's own scoping note: when multi-agent orchestration is the whole system, or a managed runtime is needed, they point to ADK / Gemini Enterprise Agent Platform instead of Genkit.

## 2. Positioning: framework vs platform

Genkit is a **library embedded in application code**: you write a function, Genkit gives you the agent loop, persistence, and a client. TrogonAI is an **infrastructure platform**: NATS backbone (ADR 0003), protocol bindings for ACP/MCP/A2A over a shared JSON-RPC codec (ADR 0011, `crates/jsonrpc-nats`), an event-sourced session kernel (`trogonai-session-kernel`), WASM-sandboxed deciders (`trogon-decider-*`), agent registry/routing (`trogon-registry`, `trogon-router`), secret detokenization (`trogon-secret-proxy`), webhook ingestion (`trogon-gateway`), and rubric-based runtime evaluation (`trogon-outcomes`).

We sit closer to the ADK/Agent Engine end of Google's own split. That is a strength, not a problem. But Genkit's announcement is significant for a different reason: **it is defining the contract application developers will expect from any agent backend.** Turn lifecycle, snapshot resume/branch, interrupt/respond/restart, detach/poll/wait/abort, delegation-as-tools, and a client SDK that mirrors the server interface. We should be at parity on that contract because our substrate can implement it better than Genkit does, and we currently expose none of it to an application developer.

## 3. Capability-by-capability comparison

| Capability | Genkit | TrogonAI today | Verdict |
|---|---|---|---|
| Agent definition primitive | `defineAgent()` (name, prompt, tools, state, store) | Per-provider runner binaries behind ACP (`trogon-acp-runner`, `trogon-xai-runner`, `trogon-openrouter-runner`, `trogon-codex-runner`); no single declarative agent primitive | **Gap** (4.1) |
| Session persistence | Snapshot store (memory/file/Firestore/custom) | Event-sourced session kernel: append-only log, leases, snapshots, recovery, migration (`trogonai-session-kernel`) | **We are stronger**, but semantics not exposed as an API |
| Resume / branch | `sessionId` / `snapshotId` branching | CLI `--continue`, `/checkpoint`, `/rewind`; `SessionBranchedPayload` and `SnapshotCreatedPayload` events exist | **Partial** (5.1) |
| Snapshot status model | `completed/pending/failed/aborted/expired` | Status enums exist at operation/lease level, none attached to a client-addressable checkpoint | **Gap** (4.2) |
| Custom typed state, JSON Patch streaming | `updateCustom(fn)` + patch chunks | Context Twin / Prompt Compiler projections (`trogonai-session-projection`), internal only | **Gap** (4.6) |
| Artifacts | Named outputs, stable identity, session-scoped | `trogonai-artifacts`: durable claim-check store, far richer metadata, no client-facing name/replace/stream semantics | **Partial** (5.2) |
| Interrupts (HITL) | `defineInterrupt`, `respond()`/`restart()`, history-validated resume | Tool approval events, `PermissionChecker`, vault approvals; no turn-level pause, ACP `StopReason` cannot express `interrupted` | **Partial** (5.3) |
| Background turns | `detach()`/`poll()`/`wait()`/`abort()`, pending snapshots | JetStream makes this natural; `docs/research/durable-agent-runs.md` explores it; no API | **Gap** (4.4) |
| Multi-agent delegation | `agents()` middleware, `delegate_to_*` tools, artifact merge | `trogon-orchestrator` (LLM planning/synthesis), `trogon-registry` + `trogon-router` (live capability routing), A2A federation | **Different shape** (4.5) |
| Client SDK | `remoteAgent()` JS SDK, same interface as local; Vercel AI adapter | None. ACP over WebSocket (`acp-nats-ws`), A2A REST+SSE (`a2a-nats-http`), but no packaged SDK | **Biggest gap** (4.1) |
| HTTP serving story | Route helpers, 3 standard endpoints | Multiple ad hoc HTTP surfaces (gateway, console, a2a-nats-http, mcp-nats-server) | **Partial** (5.6) |
| Dev UI | Agent Runner: run turns, drive interrupts, inspect snapshots | `trogon-cli` REPL, `trogon doctor`, `trogon-console` admin API, `trogon-decider-sim` | **Gap** (4.7) |
| Model provider abstraction | Single plugin registry | Duplicated per-consumer provider traits (`AnthropicMemoryProvider`, `AnthropicEvaluationProvider`, per-runner loops) | **Partial** (5.5) |
| Prompt management | dotprompt templates (broader Genkit) | Deterministic Prompt Compiler (token-budgeted projection), no reusable template/versioning system | **Partial** (5.4) |
| RAG / embeddings / vector search | First-class in broader Genkit | None (ADR 0012 mentions future vector indexing for the catalog) | **Gap** (broader framework, not this announcement) |
| Evaluation | Batch eval datasets + Dev UI (broader Genkit) | `trogon-outcomes`: rubric LLM-as-judge, Ralph self-correction loop, continuity evals | **We are stronger** at runtime eval; weaker at batch/dataset eval |
| Event sourcing, WASM sandboxing, protocol federation, secret proxy, webhook ingress, cross-runner continuity | Absent | Core of the platform | **We are ahead**, no Genkit counterpart |

## 4. Gaps in detail

Each gap follows the same structure: what Genkit ships, what we have today, what is missing, and the adoption path.

### 4.1 A first-class client SDK and a stable turn contract (highest priority)

**What Genkit ships.** `remoteAgent({url})`: the browser gets the *same* `chat()` / `sendStream()` interface as server-local code, with streaming, JSON Patch state updates, session continuation (`sessionId` / `snapshotId` / full state), per-request auth headers, and a Vercel AI adapter. Server side, route helpers mount exactly three endpoints per agent: the turn handler, `getSnapshotDataAction`, and `abortAgentAction`.

**What we have.** No client SDK at all. Consumers today are the terminal CLI (`trogon-cli`), stdio bridges (`acp-nats-stdio`, `mcp-nats-stdio`), or raw protocol traffic over `acp-nats-ws` / `a2a-nats-http`.

**What is missing.** A packaged, versioned client library and the documented turn contract underneath it: send message, receive typed stream chunks (text, tool activity, state patches, artifacts), finish reasons including `interrupted`, and resume tokens.

**Adoption path.** Define the turn contract at the protocol level over our existing ACP/JSON-RPC-over-NATS binding, exposed at the edge via `acp-nats-ws` and/or `a2a-nats-http` SSE. Ship a TypeScript client SDK (ADR 0005's future `tsworkspace/`). Standardize the three edge endpoints. A thin Vercel AI / `useChat` adapter is cheap once the SDK exists. Without this, full-stack teams evaluating agent backends will default to Genkit/ADK ergonomics even if our runtime is stronger.

### 4.2 Snapshot/turn status taxonomy and branch-by-snapshot semantics

**What Genkit ships.** `SessionSnapshot` with `status` (`pending/completed/aborted/failed/expired`), `finishReason`, `parentId` lineage, and heartbeat-based expiry detection; "only completed snapshots are resume points"; branch by `snapshotId` as a client operation.

**What we have.** Our event-sourced session kernel is strictly more capable underneath (append-only history, leases, migration, provenance headers per ADR 0013), and the status vocabulary largely exists internally: `SessionOperationState` (`Pending/InProgress/Completed/Failed/Cancelled/RequiresReconciliation`, `trogonai-session-kernel/src/state.rs:3-10`) and `LeaseState` (`Idle/Acquired/Renewing/Expired/Contended`, `state.rs:24-30`). The CLI proves branching works (`/checkpoint`, `/rewind`), and `SessionBranchedPayload` / `SnapshotCreatedPayload` events exist in the contract.

**What is missing.** None of it is attached to a client-addressable checkpoint object. Our `SessionSnapshot` proto has no snapshot ID, no status, no finish reason, no lineage (see section 6.3). There is no protocol-level branch/resume operation.

**Adoption path.** Formalize the statuses in `trogonai-session-contracts` and expose branch/resume in the turn contract. Our event-sourced log lets us beat Genkit here: we can branch from *any* event `seq` with full provenance, not just from stored snapshots.

### 4.3 Generalized interrupts with respond/restart resume

**What Genkit ships.** `defineInterrupt()` / `ctx.interrupt(metadata)`, turn finish reason `interrupted`, resume via `respond` (client supplies tool output, tool never runs) or `restart` (tool re-runs, original inputs preserved), and runtime validation of resume payloads against session history.

**What we have.** The approval *problem* is handled in specific places: `PermissionChecker` and `TROGON_MODE` in `trogon-runner-tools`, ACP permission requests, vault approvals (`trogon-vault-approvals`), and first-class approval events in the session contract (`ToolCallRequestedPayload`, `ToolCallApprovedPayload`).

**What is missing.** A generalized "tool pauses the turn durably, any client resolves it later, runtime validates the resume against history" contract, and the respond-vs-restart distinction. Today approval is an in-band blocking exchange while the runner holds the turn open. See 5.3 for the full analysis, including the ACP `StopReason` limitation.

**Adoption path.** Add interrupt request/resolution events to the session contract; make `PermissionChecker` and vault approvals *implementations* of the interrupt contract rather than parallel mechanisms. Our append-only event log makes resume validation stronger than Genkit's: the interrupt and its resolution are both durable events with `causation_id` links, so forgery is structurally detectable.

### 4.4 Detached/background turns as a protocol feature

**What Genkit ships.** `detach: true` on `AgentInput` returns a pending snapshot ID immediately; the worker heartbeats (`heartbeatAt`) into the pending snapshot; clients `poll()`/`wait()`/`abort()`; a stale heartbeat surfaces status `expired` on read. Requires a store with `onSnapshotStateChange` support plus two companion endpoints.

**What we have.** No API, but the strongest possible substrate: a detached turn is a JetStream message, progress is events on the session stream, poll/wait is a consumer, abort is a control message (and `OperationCancelRequested/Cancelled/CancelFailed` events already exist in the contract). `docs/research/durable-agent-runs.md` explores this space. Our lease TTL + `LeaseState::Expired` already plays the dead-worker-detection role Genkit built heartbeats for.

**What is missing.** The client-facing verbs (`detach`, `poll`, `wait`, `abort`) and the pending-checkpoint progress model as a documented contract.

**Adoption path.** Implement natively on JetStream; this is where our architecture gives the biggest unfair advantage. "Close the tab, reconnect tomorrow, the agent kept working" should be a headline TrogonAI capability.

### 4.5 Delegation-as-tools pattern for multi-agent

**What Genkit ships.** `agents()` middleware generating `delegate_to_<name>` tools, with `historyLength` (context forwarded), `maxDelegations` (bound per turn), `artifactStrategy: 'session'` (sub-agent artifacts merged into the parent session, namespaced by invocation), and sub-agent failures surfacing as tool output rather than top-level errors.

**What we have.** Heavier machinery: live registry with TTL/heartbeat (`trogon-registry`), LLM-driven routing (`trogon-router`), task decomposition and synthesis (`trogon-orchestrator`), A2A federation across trust boundaries. The transcript model already records delegation (`TranscriptEntry::RoutingDecision`, `TranscriptEntry::SubAgentSpawn`, `trogon-transcript/src/entry.rs:22-49`).

**What is missing.** The ergonomic pattern: delegation as plain tools visible in the stream, bounded, with artifact merge into the parent session.

**Adoption path.** A delegation tool generator in `trogon-agent-core`/`trogon-orchestrator` that turns registry entries into `delegate_to_*` tool definitions; namespace merged artifacts by `operation_id` (we already carry it on every event).

### 4.6 Custom state and artifact streaming to clients

**What Genkit ships.** A typed `custom` slot on `SessionState`, mutated via `updateCustom(fn)`, auto-emitted as RFC 6902 JSON Patch chunks (`customPatch` on `AgentStreamChunk`) so clients stay synchronized mid-stream; artifacts also stream as chunks. Agent metadata even publishes the JSON schema of the custom state (`AgentMetadata.stateSchema`) so tooling can render it.

**What we have.** Rich *platform-owned* state (Context Twin, todos, plans) that streams to ACP clients as `Plan` / `SessionInfoUpdate` notifications, but no *application-defined* typed state slot and no generic patch mechanism.

**What is missing.** An app-typed `custom` equivalent in `SessionSnapshotState`, patch-streamed over the turn stream; artifact create/replace notifications in the stream.

**Adoption path.** Add a custom-state payload to the session event contract (a new `SessionEventPayload` variant), fold it in `materialize_from_events`, and emit patches as a new ACP session update (or `meta` extension). See 6.5.

### 4.7 Browser-based Agent Runner

**What Genkit ships.** A Dev UI Agent Runner: run conversations, watch streamed output and state, drive tool interrupts, inspect snapshots, before any frontend exists. `AgentMetadata` (`stateManagement`, `abortable`, `stateSchema`) lets the UI adapt to each agent's capabilities.

**What we have.** Terminal-only equivalents: `trogon-cli` REPL, `trogon doctor`, `trogon-decider-sim`; `trogon-console` is an admin API, not an interactive runner.

**What is missing.** Any browser surface for driving a turn.

**Adoption path.** Once the turn contract and TS SDK exist (4.1), a minimal web Agent Runner on top of `trogon-console` is mostly free and doubles as the SDK's first consumer and living documentation.

### 4.8 Internal hygiene: consolidate the provider abstraction

Not a Genkit feature per se, but the comparison exposes it: we re-implement LLM provider access per consumer (`AnthropicMemoryProvider`, `AnthropicEvaluationProvider`, four separate runner loops). Genkit's single model registry is one of its genuinely good library ideas. A shared provider crate (completion + streaming + tool-use surface) would cut duplication across `trogon-memory`, `trogon-outcomes`, `trogon-orchestrator`, and the runners, without flattening the deliberate runner-per-provider process isolation. See 5.5.

## 5. Partial situations in detail

These are the areas where we have real capability but it is incomplete, internal-only, or shaped differently from what application developers will expect.

### 5.1 Resume and branching

**What exists.**
- `SessionBranchedPayload` (variant 24) and `SnapshotCreatedPayload` (variant 37) in the session event contract (`rsworkspace/proto/trogonai/session/v1/session_event.proto:280-325`).
- Every event carries a per-session `seq: uint64`, so any historical point is addressable.
- CLI `--continue`, `--session-id`, `/checkpoint`, `/rewind` prove the capability end to end.
- `materialize_from_events` (`trogonai-session-kernel/src/materialize.rs:13`) can rebuild state at any point deterministically (it preserves event timestamps, dedups by `event_id`/`idempotency_key`).

**What is incomplete.**
- No client-facing resume token. Genkit clients hold a `snapshotId`; our clients hold nothing (the CLI resolves checkpoints internally).
- No ownership guard semantics. Genkit's `AgentInit` treats `sessionId` as an ownership check when both IDs are supplied; we have no equivalent contract.
- Branch lineage is recorded as an event but not queryable as a history tree by clients.

**Risk of leaving as-is.** Branching stays a CLI party trick instead of a platform feature; any future frontend must invent its own checkpoint addressing.

**Action.** Give checkpoints stable IDs in `trogonai-session-contracts`, expose `resume(session_id | checkpoint_id)` and `branch(checkpoint_id | seq)` in the turn contract. Branch-by-`seq` is a capability Genkit cannot offer (their snapshots are the only addressable points; our whole log is).

### 5.2 Artifacts

**What exists.** A far richer durability pipeline than Genkit's:
- `ArtifactMetadata` with 21 fields including `sha256`, `size_bytes`, `mime`, `preview`, `storage_ref`, `retention_policy`, `permission_scope`, `encryption_status`, `truncated`, `availability`, `source_url`, and decode provenance (`rsworkspace/proto/trogonai/session/v1/artifact_metadata.proto:9-41`).
- Claim-check vs inline storage (`ArtifactStorageMode`, `trogonai-artifacts/src/store.rs:21-24`), checksum-verified retrieval (`ArtifactUnavailableReason::ChecksumMismatch`), GC lifecycle events (`ArtifactGcMarkedPayload`, `ArtifactGcDeletedPayload`), redaction (`RedactionAppliedPayload`).
- Full event context on every write (`ArtifactEventContext`: `operation_id`, `correlation_id`, `idempotency_key`, `causation_id`, `actor`, `trogonai-artifacts/src/kernel.rs:19-25`).

**What is incomplete.**
- No human-meaningful stable name. Genkit's `Artifact.name` ("generated_code.go") is the identity clients use, with replace-by-name semantics for iterative regeneration. We have only opaque `artifact_id` plus a `preview` string.
- No client streaming: artifacts never appear in the ACP update stream; Genkit streams them as `AgentStreamChunk.artifact`.
- No inspect/download contract (Genkit pairs `artifactStrategy: 'session'` with `read_artifact` tools for orchestrators).

**Risk of leaving as-is.** Clients cannot render "the report the agent just wrote" without scraping tool output; multi-agent artifact merge (4.5) has no identity to merge on.

**Action.** Add a `name` (with replace-by-name event semantics) to the artifact contract, emit artifact create/replace stream notifications, and expose a read/download endpoint at the edge. Keep the durability model; it is a differentiator (see 6.6).

### 5.3 Interrupts and human-in-the-loop

**What exists.**
- Tool-approval as first-class events: `ToolCallRequestedPayload`, `ToolCallApprovedPayload`, `ToolCallStartedPayload` in the session contract, with `ToolCallStatus` including `REQUIRES_RECONCILIATION` (`common.proto:22-29`).
- Runtime enforcement: `PermissionChecker`, `TROGON_MODE=acceptEdits`, `EgressPolicy` in `trogon-runner-tools`; human approval flows in `trogon-vault-approvals`.
- Cancellation as events: `OperationCancelRequested/Cancelled/CancelFailed`, `RunnerCancelRequested/Cancelled`.

**What is incomplete.**
- **The turn cannot end in an interrupted state.** ACP's `StopReason` has exactly five variants: `EndTurn`, `MaxTokens`, `MaxTurnRequests`, `Refusal`, `Cancelled` (agent-client-protocol crate, `agent.rs:3088-3107`). There is no `Interrupted`, `Detached`, or `Failed`. Genkit's `AgentFinishReason` has nine, and `interrupted` is the pivot of its whole HITL story.
- Approval is *synchronous*: the runner blocks mid-turn on an ACP permission request while holding the session lease. The user must be present; there is no durable paused turn another client can pick up hours later.
- No respond-vs-restart distinction: we can only approve/deny execution, not supply the tool result from outside (`respond`) or re-run with changed environment (`restart`).
- Resume-payload validation is not formalized, although we have better ingredients than Genkit (idempotency keys, `causation_id`, append-only history).

**Risk of leaving as-is.** "Approve this payment from your phone tomorrow morning" class workflows are impossible; every approval requires a live, connected client holding the turn open.

**Action.** Add interrupt request/resolution payload variants to the session event contract; end the turn durably with an `interrupted` finish reason; validate resolutions against the interrupting event by `event_id`/`causation_id`. The ACP `StopReason` limitation is an upstream protocol constraint: short-term, carry the extended reason in ACP's `meta` field; longer-term, propose the variant upstream. Housekeeping note: `trogon-acp-runner` pins `agent-client-protocol = 0.10.2` while the workspace pins `=0.10.4` (schema crate 0.11.4); worth unifying before extending the protocol.

### 5.4 Prompt management

**What exists.** A deterministic Prompt Compiler that is more sophisticated than anything in Genkit, but solves a different problem: `PromptProjection` records `included_blocks` *and* `excluded_blocks` with 13 block kinds, per-block `token_estimate` and `source_seq` provenance, and explicit `DegradationMetadata` (9 kinds, e.g. `IMAGES_OMITTED`, `HISTORY_SUMMARIZED`, `REASONING_NOT_PORTABLE`) (`rsworkspace/proto/trogonai/session/v1/prompt_projection.proto:54-65`). This is context *compilation* with an audit trail.

**What is incomplete.** Prompt *authoring*: Genkit's dotprompt gives app developers templated, versioned, testable prompt files with typed input schemas. We have no equivalent for user-authored prompts; system prompts live in runner code.

**Action.** Watch. If/when external developers define agents on the platform (4.1 makes that likely), a versioned prompt artifact type is the natural extension; it would slot into the existing catalog (`trogonai-catalog`) rather than copying dotprompt.

### 5.5 Provider abstraction

**What exists.** Deliberate runner-per-provider process isolation (`trogon-acp-runner`, `trogon-xai-runner`, `trogon-openrouter-runner`, `trogon-codex-runner`) behind one protocol, which enables cross-runner session switching (`trogonai-switching`), a genuine differentiator.

**What is incomplete.** Non-runner LLM consumers each hand-roll a provider: `AnthropicMemoryProvider` (`trogon-memory`), `AnthropicEvaluationProvider` (`trogon-outcomes`), plus the orchestrator's own calls. Three-plus implementations of "call Claude with retries and parse the result."

**Action.** Adapt, not adopt: a shared `trogon-llm`-style crate for the embedded consumers. Do not flatten the runner architecture into it; the process boundary is the feature.

### 5.6 HTTP serving surfaces

**What exists.** Several purpose-built HTTP surfaces: `trogon-gateway` (webhook ingress), `trogon-console` (admin, port 8090), `a2a-nats-http` (A2A REST + SSE), `mcp-nats-server` (MCP streamable HTTP), `acp-nats-ws` (WebSocket), `ard-registry` (discovery).

**What is incomplete.** No single blessed surface for "talk to an agent from an app." Genkit converged on three endpoints per agent (turn, snapshot fetch, abort); each of our surfaces solves an adjacent problem.

**Action.** Define the app-facing edge as part of 4.1: one documented surface (WS and/or SSE) implementing the turn contract, snapshot fetch, and abort, fronting the NATS backbone. Everything else stays as-is.

## 6. Data model comparison

Genkit's entire wire model lives in one file (`js/ai/src/agent-types.ts`); ours spans the session protos (`rsworkspace/proto/trogonai/session/v1/`), the kernel crates, and the external ACP schema crate. The philosophical difference drives everything below:

> **Genkit persists state; we persist history.** A Genkit snapshot is a mutable row holding the cumulative conversation state. A TrogonAI session is an append-only log of 46 typed event kinds, from which state (snapshot, Context Twin, prompt projection) is derived by deterministic folds. Genkit's model is simpler to consume; ours can answer "why", audit, replay, migrate, and branch anywhere. The right move is to *project* Genkit-shaped views out of our log, not to replace the log.

### 6.1 Session state container

Genkit `SessionState<S>` vs our `SessionSnapshotState` (`session_snapshot.proto:71-88`):

| Genkit `SessionState<S>` | TrogonAI `SessionSnapshotState` | Notes |
|---|---|---|
| `sessionId?: string` | `session_id` (on the `SessionSnapshot` envelope) | Equivalent; ours is a validated `sess_`-prefixed newtype (`SessionId`) |
| `messages?: MessageData[]` | `conversation: repeated CanonicalMessage` | Equivalent role+content history |
| `custom?: S` (app-typed, schema published via `AgentMetadata.stateSchema`) | No equivalent. Closest: `context_twin`, `todos`, but both are platform-owned, not app-defined | **The gap** (4.6) |
| `artifacts?: Artifact[]` | `artifacts: repeated ArtifactMetadata` | Ours is metadata + claim check; theirs is inline content (6.6) |
| (none) | `config`, `tool_calls`, `summaries`, `switch_adaptation_plan`, `switch_safety`, `continuity_checkpoint`, `audit_log`, `usage`, `nonportable`, `active_runner_binding`, `terminal` | Ours only: model switching, continuity, audit, usage accounting have no Genkit counterpart |

Verdict: our state container is a strict superset except for the single most app-developer-visible field, the typed `custom` slot.

### 6.2 The event log (no Genkit equivalent)

Our fundamental unit, `SessionEvent` (`session_event.proto:331-343`), has no counterpart in Genkit at all:

```
schema_version, event_id (evt_*), session_id (sess_*), seq (u64, gapless per session),
operation_id, correlation_id, causation_id?, idempotency_key,
created_at, actor, payload (oneof, 46 variants)
```

The 46 payload variants cover conversation, tool lifecycle, artifacts and their GC, compaction, branching, model switching, runner lifecycle, permissions, cancellation, redaction, and continuity. Genkit's only record of "what happened" is the message array inside the latest snapshot; it cannot express causality (`causation_id`), attribution (`actor`), idempotency, or schema evolution (`schema_version`). This is the asset every recommendation in section 8 builds on.

### 6.3 Snapshot / checkpoint

| Genkit `SessionSnapshot<S>` (`agent-types.ts`) | TrogonAI `SessionSnapshot` (`session_snapshot.proto:91-97`) | Notes |
|---|---|---|
| `snapshotId: string` (client-addressable resume token) | None. KV-keyed by session; checkpoints are events, not addressable objects | **Missing on our side** (4.2, 5.1) |
| `sessionId?` | `session_id` | Equivalent |
| `parentId?` (lineage for history trees) | None on the snapshot; lineage lives in `SessionBranchedPayload` events | Ours is recoverable but not queryable |
| `createdAt`, `updatedAt` | `materialized_at` | Equivalent |
| `heartbeatAt?` (stale heartbeat => status `expired` computed on read) | None; the session lease TTL (`SessionKvLease`, `LeaseState::Expired`) plays this role | Different mechanism, same job; ours is already multi-instance safe |
| `status?: pending/completed/aborted/failed/expired` | None on the snapshot; `SessionOperationState` exists at operation level | **Missing at the checkpoint level** (4.2) |
| `finishReason?`, `error?` (last-good-state semantics on failure) | None | **Missing** |
| `state?: SessionState<S>` (empty while pending) | `state: SessionSnapshotState` | Equivalent role |
| (none) | `schema_version`, `last_applied_seq` | Ours only: exact event provenance; Genkit snapshots cannot say what they are derived from |

Verdict: Genkit's snapshot is an *identity* (resume token, branch point, status carrier). Ours is a *cache* (materialization of the log). We should add the identity layer without giving up the cache semantics: a checkpoint object carrying `{checkpoint_id, session_id, parent, seq, status, finish_reason, error}` referencing the log.

### 6.4 Turn input/output and finish reasons

Turn input, Genkit `AgentInput` + `AgentInit` vs ACP `PromptRequest` (`agent.rs:2912-2948`):

| Genkit | ACP (ours) | Notes |
|---|---|---|
| `message?: Message` | `prompt: Vec<ContentBlock>` | Equivalent (ContentBlock: Text/Image/Audio/ResourceLink/Resource) |
| `resume: { respond?: ToolResponsePart[], restart?: ToolRequestPart[] }` | None | **Missing**: the HITL resume channel (5.3) |
| `detach?: boolean` | None | **Missing**: background turns (4.4) |
| Init: `snapshotId?`, `sessionId?` (ownership guard), or full `state` (client-managed) | `session_id` only | **Missing**: checkpoint resume; client-managed state we deliberately skip |

Turn output, Genkit `AgentOutput` vs ACP `PromptResponse` (`agent.rs:2997-3027`):

| Genkit | ACP (ours) | Notes |
|---|---|---|
| `sessionId`, `snapshotId` | Implicit in the NATS subject; nothing returned | Missing resume token in the response |
| `state?` | None | By design ours streams instead; acceptable |
| `message?`, `artifacts?` | Streamed via `SessionNotification` during the turn | Different shape, same information (except artifacts, 5.2) |
| `finishReason` | `stop_reason` | Vocabulary gap below |
| `error?` with "state/snapshotId hold the last-good state" semantics | JSON-RPC error, no state anchoring | **Missing**: failure-with-recovery-point semantics |

Finish reason vocabularies, Genkit `AgentFinishReason` (9 values) vs ACP `StopReason` (5 values, `agent.rs:3088-3107`):

| Genkit | ACP (ours) | Status |
|---|---|---|
| `stop` | `EndTurn` | Equivalent |
| `length` | `MaxTokens` | Equivalent |
| `blocked` | `Refusal` | Roughly equivalent |
| `aborted` | `Cancelled` | Roughly equivalent |
| (none) | `MaxTurnRequests` | Ours only (a good one) |
| `interrupted` | **none** | **Missing; blocks the whole HITL story** (5.3) |
| `detached` | **none** | **Missing; blocks background turns** (4.4) |
| `failed` | **none** | Missing; errors are transport errors, not data |
| `other`, `unknown` | none | Catch-alls, low value |

### 6.5 Stream chunks

Genkit `AgentStreamChunk` vs ACP `SessionUpdate` (`client.rs:82-113`):

| Genkit `AgentStreamChunk` | ACP `SessionUpdate` (ours) | Notes |
|---|---|---|
| `modelChunk` | `AgentMessageChunk`, `AgentThoughtChunk`, `UserMessageChunk` | Ours richer: separate thought channel |
| `customPatch` (RFC 6902 JSON Patch on custom state, auto-emitted) | None | **Missing** (4.6) |
| `artifact` | None (`ToolCallContent::Diff` is the nearest cousin) | **Missing** (5.2) |
| `turnEnd { snapshotId, finishReason }` | None in-stream; the `PromptResponse` ends the request | Missing the in-stream resume token |
| (none) | `ToolCall` / `ToolCallUpdate` with `kind` (10 values), `status`, `locations` (path:line), `content` including structured `Diff` and `Terminal` | **Ours far richer**: Genkit tool activity is opaque parts |
| (none) | `Plan` (`PlanEntry{content, priority, status}`) | Ours only |
| (none) | `AvailableCommandsUpdate`, `CurrentModeUpdate`, `ConfigOptionUpdate`, `SessionInfoUpdate`, `UsageUpdate` | Ours only |

Verdict: our stream is substantially richer for agent-coding UX (diffs, plans, thoughts, tool locations); Genkit's is richer for app-state UX (custom patches, artifacts, turn end). The union is the right target for the turn contract in 4.1.

### 6.6 Artifacts

| Genkit `Artifact` | TrogonAI (`ArtifactMetadata` + `ArtifactRef` + store types) | Notes |
|---|---|---|
| `name?: string` (identity; same-name write replaces) | None; identity is opaque `artifact_id` | **Missing on ours** (5.2) |
| `parts: Part[]` (content inline in session state) | Content via `storage_ref` (claim check) or `inline_content`; `ArtifactRef{artifact_id, sha256, size_bytes, mime, preview, truncated}` carried in events (`common.proto:165-172`) | Ours scales; theirs bloats snapshots (their own docs warn about growth) |
| `metadata?: record` | Dedicated typed fields: `sha256`, `size_bytes`, `mime`, `preview`, `storage_ref`, `retention_policy`, `permission_scope`, `encryption_status`, `truncated`, `availability`, `source_url`, `fetched_at`, `source_encoding`, `declared_mime`, `decoded_mime`, plus `session_id`/`event_id`/`tool_execution_id` provenance | **Ours far richer**: durability, security, and provenance are first-class |
| (implicit) | GC lifecycle (`ArtifactGcMarked/Deleted` events), checksum-verified retrieval (`ArtifactAvailability`), redaction | Ours only |

### 6.7 Interrupt/resume payloads

| Genkit | TrogonAI | Notes |
|---|---|---|
| `resume.respond: ToolResponsePart[]` (client is the tool) | None | Missing |
| `resume.restart: ToolRequestPart[]` (re-run, inputs preserved) | None | Missing |
| Validation: resume must match interrupted request by name and reference | Ingredients exist and are stronger: `event_id`, `causation_id`, `idempotency_key` on every event; approval already evented (`ToolCallRequested/Approved`) | Formalize, don't invent (5.3) |

### 6.8 Persistence and concurrency

| Concern | Genkit `SessionStore` | TrogonAI kernel |
|---|---|---|
| Unit of persistence | Mutable snapshot rows | Append-only `SessionEvent` on JetStream; snapshots as KV cache (`SnapshotStore`, size-checked) |
| Read model | Get by `snapshotId`; "latest leaf" resolution per session | `read_session_events` + `materialize_from_events` fold; `last_seq` |
| Write concurrency | Atomic read-modify-write mutator; branch-conflict rejection varies by store (in-memory/file yes, Firestore no) | Session lease with TTL (`SessionKvLease`), typed mutating operations (`SessionMutatingOperation`: `PromptTurn/SwitchModel/Compact/Fork/Restore/Close/MutatingToolCall`), `LeaseState` including `Contended`; NATS `Nats-Msg-Id` dedup via `idempotency_key` | 
| Change feed | `onSnapshotStateChange` (optional per store) | JetStream consumers, native and multi-instance | 
| Causality/attribution | `parentId` lineage only | `operation_id`, `correlation_id`, `causation_id`, `actor` on every event |
| Schema evolution | None | `schema_version` on every event and snapshot; `ValidatedSessionEvent`/`ValidatedSessionSnapshot` enforcement |
| Retention | "Plan retention before launch" (manual) | `retention_policy` on artifacts, `EventsArchived` + GC events |

Verdict: our persistence layer dominates on every row except that none of it is reachable by an application client. Separately, the decider substrate (`Decider` trait with `evolve`/`decide`, `Decision::Events|Act`, `WritePrecondition::Any|StreamExists|NoStream`, `StreamPosition` high-watermark, typed `Snapshot<T>` at `trogon-decider*`) generalizes this same model to arbitrary business logic, including WASM-sandboxed deciders; Genkit has no concept in this territory.

### 6.9 Status vocabularies

We have the pieces of Genkit's snapshot status enum scattered across four internal enums; the adoption work (4.2) is mapping, not invention:

| Genkit `SnapshotStatus` | Our nearest equivalents |
|---|---|
| `pending` | `SessionOperationState::Pending/InProgress`; ACP `ToolCallStatus::Pending/InProgress` |
| `completed` | `SessionOperationState::Completed` |
| `failed` | `SessionOperationState::Failed`; `RunnerFailedPayload` |
| `aborted` | `SessionOperationState::Cancelled`; `OperationCancelledPayload`; ACP `StopReason::Cancelled` |
| `expired` (computed from stale `heartbeatAt`) | `LeaseState::Expired` (lease TTL) |
| (none) | `SessionOperationState::RequiresReconciliation`, `LeaseState::Contended`, `RecoveryState::StaleSnapshot`: ours only, and worth keeping in the public taxonomy |

### 6.10 Summary of the model comparison

What Genkit's model has that ours lacks (all adoptable as projections/extensions of the log):

1. Client-addressable checkpoint identity (`snapshotId`) with lineage, status, and finish reason.
2. An app-typed `custom` state slot with schema publication and JSON Patch streaming.
3. Turn-level finish reasons `interrupted`, `detached`, `failed`, and the `resume.respond/restart` input channel.
4. Artifact `name` identity with replace-by-name and in-stream artifact delivery.
5. Failure semantics that anchor to a last-good state instead of a bare transport error.

What our model has that Genkit's lacks (none of it should be sacrificed):

1. The event log itself: 46 typed payloads, causality, attribution, idempotency, schema versioning, gapless `seq`.
2. Leases and typed mutating operations for multi-writer safety; contention and reconciliation states.
3. Durable, content-addressed, GC'd, permission-scoped, redactable artifacts.
4. Model switching, cross-runner continuity, compaction, audit, and usage accounting in the state model.
5. Rich tool-activity streaming (diffs, terminals, file locations, plans, thoughts).
6. The decider substrate generalizing event sourcing to sandboxed business logic.

## 7. Where we are ahead (keep investing, do not chase Genkit)

- **Event-sourced sessions and deciders**: Genkit stores snapshots; we store *why*. Append-only history, WASM-sandboxed zero-import deciders (`trogon-decider-wasm-runtime`), simulation harness (`trogon-decider-sim`), migration/rollout machinery. No Genkit counterpart exists or is planned.
- **Transport/protocol formalization**: one JSON-RPC-over-NATS codec under ACP/MCP/A2A (ADR 0011) vs Genkit's HTTP-only story. Cross-runner session continuity (`trogonai-switching`) has no Genkit analogue.
- **Production trust boundary features**: secret detokenization proxy, A2A policy tiers (declarative + CEL), signed federation, auth callouts. Genkit delegates all of this to "your infrastructure."
- **Runtime evaluation**: rubric-graded outcomes plus the Ralph self-correction loop (`trogon-outcomes`) is ahead of Genkit's batch eval story for production agents.
- **Memory**: the Dreamer (`trogon-memory`) extracts durable facts across sessions; Genkit has no long-term memory primitive.
- **Ingestion**: `trogon-gateway`'s webhook-to-JetStream fabric (10+ sources) has no Genkit equivalent.

Client-managed state (Genkit's stateless mode where clients round-trip full `SessionState`) is the one announced feature I recommend **skipping**: it exists to serve stateless deployments, contradicts our event-sourced model, and server-managed state is Genkit's own default recommendation anyway.

## 8. Recommendation summary

| # | Item | Action | Where it lands |
|---|---|---|---|
| 1 | Turn contract + TypeScript client SDK (`remoteAgent` equivalent) | **Adopt** | New `tsworkspace/` (ADR 0005), edge via `acp-nats-ws` / `a2a-nats-http` |
| 2 | Checkpoint identity + status taxonomy + branch/resume API | **Adopt** | `trogonai-session-contracts`, `trogonai-session-kernel` |
| 3 | Interrupt contract with respond/restart + history validation; extend finish reasons (`interrupted`, `detached`, `failed`) | **Adopt** | `trogon-agent-core`, `trogon-runner-tools`, session contracts; ACP `meta` short-term, upstream proposal long-term |
| 4 | Detach/poll/wait/abort background turns | **Adopt** (native on JetStream) | Session kernel + turn contract; builds on `docs/research/durable-agent-runs.md` |
| 5 | `delegate_to_*` delegation tools + artifact merge namespaced by `operation_id` | **Adapt** | `trogon-orchestrator` + `trogon-registry` |
| 6 | Typed custom state + JSON Patch streaming; artifact `name` identity + replace-by-name + in-stream delivery | **Adopt** | Session contracts, `trogonai-artifacts` |
| 7 | Web Agent Runner | **Adopt after #1** | On top of `trogon-console` |
| 8 | Shared LLM provider crate for embedded consumers | **Adapt** (internal refactor) | New `trogon-llm`-style crate |
| 9 | Vercel AI / `useChat` adapter | **Watch, then adopt** (cheap once #1 ships) | `tsworkspace/` |
| 10 | Prompt templating (dotprompt-style authoring) | **Watch** | Only if app-level prompt authoring becomes a user need; slots into `trogonai-catalog` |
| 11 | RAG/embeddings/vector search | **Watch** | Revisit with ARD catalog vector indexing (ADR 0012) |
| 12 | Client-managed state mode | **Skip** | Contradicts event-sourced design |

### Suggested sequencing

1. **Contracts first** (#2, #3, #6): they are protocol/schema work on the session kernel we already have, and everything else depends on them.
2. **Edge + SDK** (#1, #4): expose the contract over WS/SSE and ship the TS SDK; implement detached turns natively on JetStream in the same pass since the SDK must speak them.
3. **Ergonomics** (#5, #7, #9): delegation tools, web runner, Vercel adapter, each a fast follow that consumes the SDK.
4. **Hygiene** (#8, plus the `agent-client-protocol` version unification noted in 5.3) opportunistically as runners/memory/outcomes get touched.

The strategic takeaway: Genkit just published the vocabulary that application developers will use to judge agent backends. Every word of that vocabulary (turn, snapshot, interrupt, detach, delegate) is something our event-sourced NATS substrate can implement with stronger guarantees than a snapshot store behind HTTP. The data model comparison in section 6 makes it concrete: five things to project out of the log, six things to protect. The gap is not capability, it is contract and client experience, and closing it is mostly schema and SDK work, not runtime work.
