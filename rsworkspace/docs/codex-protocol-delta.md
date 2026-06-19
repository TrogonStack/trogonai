# Codex Protocol Delta — runner assumptions vs real `codex-cli 0.138.0`

**Phase 0 deliverable (PG-2 + PG-4).** Generated 2026-06-17 from the authoritative schema
(`codex app-server generate-json-schema --experimental`, committed under
`docs/codex-protocol-0.138/`) plus a live `initialize` smoke test against the real binary.

**Source of truth:** the schema is emitted by the codex binary itself, so these shapes are
exact, not inferred. Method lists: `ClientRequest` 114, `ServerRequest` 10, `ServerNotification`
65, `ClientNotification` 1.

---

## TL;DR / PG-4 verdict

**There IS a foundational protocol bump** (the Phase-0.5 the plan warned about). The runner's
request *method names* mostly match, but its **server→client event model in
`process.rs::parse_event` is built around a protocol that does not exist in 0.138** — most
notably it keys streaming on `item/updated`, **which is not a real method**. Almost every
fidelity fix (B2, P3, P4, P5) is therefore *not* a tweak but part of a `parse_event` rewrite,
and the mock + integration tests (which encode the wrong protocol) must be rewritten too.

**But the upside is large:** native protocol support exists for things previously scoped as
"build it ourselves" or "architecturally infeasible" — **mid-turn steering, compaction, token
usage, per-tool approvals, elicitation, MCP, skills, and thread persistence/resume**. Several
gaps collapse from "implement" to "wire to the native method."

---

## What MATCHES (good news)

- Transport: real binary is a stdio JSON-RPC server. Live `initialize` returned
  `{userAgent, codexHome, platformFamily, platformOs}` and then proactively emitted a
  `remoteControl/status/changed` notification. Handshake works.
- Request methods the runner sends all exist by name: `initialize`, `thread/start`,
  `thread/resume`, `thread/fork`, `turn/start`, `turn/interrupt`.
- `turn/completed` and `error` notifications exist.
- `approvalPolicy` is a real `turn/start` + `thread/start` field (our coarse mode mapping lands).
- `model` is a real `turn/start` field (our model forwarding lands).

## What is WRONG or MISSING (the bump)

### 1. Handshake response shape — `thread.id`, not `threadId`
`thread_start` (process.rs:182) reads `result["threadId"]`. Real **`ThreadStartResponse`** has
no `threadId`; it returns `thread: Thread` where `Thread.id` (and `Thread.sessionId`) live.
Also our params send `{workingDirectory}`; real **`ThreadStartParams`** uses `cwd`. → thread
creation silently fails against real codex (the documented 2026-06-10 mismatch, now pinpointed).

### 2. `turn/start` input is an array, not a string
`turn_start` sends `user_input: &str`. Real **`TurnStartParams`** requires `{threadId, input:
array}` (input items), plus optional `model`, `approvalPolicy`, `effort`, `summary`,
`collaborationMode`, `personality`, `outputSchema`, `sandboxPolicy`, … → turns malformed.

### 3. Streaming model is entirely different — `item/updated` does not exist
`parse_event` (process.rs:398+) routes on `item/updated` / `item/completed` and reads
`item.content[].output_text`. Real 0.138 has **no `item/updated`**. Instead:

| Need | Real notification | Shape |
|------|-------------------|-------|
| Assistant text (B2) | **`item/agentMessage/delta`** | `{delta, itemId, threadId, turnId}` — **true incremental deltas** |
| Item lifecycle | `item/started`, `item/completed` | `{item: ThreadItem, startedAtMs/completedAtMs, threadId, turnId}` |
| Reasoning/thoughts (P3) | `item/reasoning/textDelta`, `item/reasoning/summaryTextDelta`, `item/reasoning/summaryPartAdded` | reasoning deltas |
| Tool output (P4) | `item/commandExecution/outputDelta`, `command/exec/outputDelta`, `process/outputDelta` | streaming tool output |
| Token usage (P5) | **`thread/tokenUsage/updated`** | `{tokenUsage: ThreadTokenUsage, threadId, turnId}` |
| Diffs/plan | `turn/diff/updated`, `turn/plan/updated`, `item/plan/delta` | |

**B2 is moot as written:** its premise ("`item/updated` is a cumulative snapshot") is false —
the real method doesn't exist and the actual text events (`item/agentMessage/delta`) are true
deltas. The real work is consuming the real notification set.

### 4. Server→client REQUESTS are ignored — per-tool gating is FEASIBLE
`read_loop` only routes responses + notifications. Real **`ServerRequest`** (10 methods) are
server→client *requests* awaiting a reply:
`execCommandApproval`, `applyPatchApproval`, `item/commandExecution/requestApproval`,
`item/fileChange/requestApproval`, `item/permissions/requestApproval`,
`item/tool/requestUserInput`, `mcpServer/elicitation/request`, `item/tool/call`, …
e.g. `ExecCommandApprovalParams = {callId, command, conversationId, cwd, parsedCmd, approvalId?, reason?}`.
→ **Per-tool permission gating + elicitation become achievable**, flipping the headline
"architectural-infeasible" item. `read_loop` must grow an inbound-request branch that bridges
these to Trogon's permission/elicitation path and replies.

### 5. Events are keyed by `turnId`/`itemId`, not just `threadId`
`parse_event` extracts only `threadId`; `turn_senders` is keyed by thread. Real notifications
carry `threadId` + `turnId` + `itemId`. Multi-turn / per-item routing needs the richer key.

## Native capabilities that shrink later phases

| Gap (plan) | Was scoped as | Native method available |
|------------|---------------|-------------------------|
| Mid-turn steering | next-turn fallback only | **`turn/steer`** (`TurnSteerParams`) — real mid-turn steer |
| `/compact` (E1) | export→compactor→import dance | **`thread/compact/start`** + `thread/compacted` notification — native compaction |
| Token usage / `/cost` (P5) | build parsing | **`thread/tokenUsage/updated`** notification |
| Persistence (Phase 5) | mirror history to NATS KV | **`thread/list`, `thread/read`, `thread/resume`, `thread/turns/list`** — native thread store |
| MCP | none | `mcpServer/tool/call`, `mcpServer/resource/read`, `mcpServerStatus/list`, `mcpServer/oauth/login` |
| Skills | none | `skills/list`, `skills/config/write`, `skills/extraRoots/set` |
| Model list | static env list | `model/list`, `modelProvider/capabilities/read` |

---

## PG-4 re-scope decisions

1. **Foundational protocol bump is REQUIRED and is now Phase 0.5** — rewrite `process.rs`
   (`thread_start` → `thread.id`/`cwd`; `turn_start` → `input` array; `parse_event` → real
   notification set; `read_loop` → inbound-request branch + `turnId` keying) and a new
   `CodexEvent` set. The mock (`mock_codex_server.rs`) and integration tests must be rebuilt
   against the real schema. **This is the gate for everything else.**
2. **Per-tool approval gating is feasible** → moves from "architectural-infeasible" to a
   planned feature (handle `ServerRequest` approvals). Plan-mode read-only *may* also become
   enforceable via approvals — re-evaluate after 0.5.
3. **B2 collapses**; **P3/P5 become straightforward** (exact notifications identified);
   **steering and `/compact` get native paths**.
4. **Image input:** `turn/start.input` is a typed array (`InputModality`, `ImageDetail`,
   `ContentItem` exist in the schema) → multimodal is likely supported; confirm in 0.5.

## Estimate impact

- **Added:** the Phase 0.5 protocol bump + mock/test rewrite — **~3–5 ideal days**, and it is
  now on the critical path (was implicit risk, now explicit work).
- **Removed/reduced:** steering, `/compact`, usage, and persistence each get a native method
  instead of a from-scratch build — claw back **~2–4 ideal days** across Phases 4–6 and the
  deferred steering/persistence work.
- **Net:** roughly a wash on total effort, but the *shape* changes — there is one big
  foundational rewrite up front, after which the remaining phases are smaller and lower-risk.
  Revised single-dev calendar for real-codex parity: **~5–6 weeks** (the bump replaces the old
  speculative tail with concrete, scoped work).

## How to reproduce
```
codex app-server generate-json-schema --experimental --out docs/codex-protocol-0.138/json-schema
codex app-server generate-ts          --experimental --out docs/codex-protocol-0.138/ts
# live handshake smoke test: pipe an `initialize` JSON-RPC line into `codex app-server`
```
