# Tool Approval Gates + Elicitations — Implementation Plan

## Context

The boss requirement: **"We must have tool approval gates, that is required. The exact same way Claude asks you for doing some actions."**

Approval gates are a **security feature**, not a UX feature. This means they must cover every path an agent can take — both the NATS runner and the HTTP agent. A gate that only exists on one path is not a gate.

Two concepts to implement:
- **Approval gate** — agent is about to do something sensitive (merge to production, delete data, post to Slack), pauses and asks: Allow Once / Allow Always / Deny.
- **Elicitation** — agent needs a piece of information to continue, asks the user a structured question, waits for a real answer, then resumes.

---

## Architecture Principle

One set of traits (`PermissionChecker`, `ElicitationProvider`), two transport implementations (NATS and HTTP). The gate logic lives once in `trogon-agent-core`, wired differently per transport.

---

## What Exists Today

### Already implemented
- `PermissionChecker` trait in `trogon-agent-core/src/agent_loop.rs:22` — hook fires at lines 734 and 785 before every tool dispatch
- `ChannelPermissionChecker` in `trogon-acp-runner/src/permission.rs` — sends `PermissionReq` via mpsc, 60s timeout, auto-allow list
- `NatsClientProxy::request_permission` in `acp-nats/src/client_proxy.rs:73` — fully implemented, NATS request-reply
- `RequestPermissionOutcome` in `agent-client-protocol` — `AllowOnce`, `AllowAlways`, `Denied`, `Cancelled`

### What is missing

| | NATS path | HTTP path |
|---|---|---|
| Permission hook in agent loop | ✅ exists in agent-core | ❌ missing in trogon-agent |
| Channel + bridge task | ❌ never created in main.rs | ❌ N/A |
| WebSocket bridge forwards request | ❌ returns `Cancelled` | ❌ N/A |
| Streaming (required for HTTP gates) | ✅ NATS is naturally async | ❌ missing |
| Client-facing approval endpoint | ❌ N/A | ❌ missing |
| Elicitations (any part) | ❌ | ❌ |

---

## Phase 1 — NATS: Wire the Permission Bridge (1 day)

**Files:** `trogon-acp-runner/src/main.rs`, new `trogon-acp-runner/src/permission_bridge.rs`

Nobody creates the `mpsc::channel::<PermissionReq>()` today — so `permission_tx` is always `None` and gates never fire.

**Changes:**

1. In `main.rs`, create the channel and pass `perm_tx` to `TrogonAgent::new()`:
   ```rust
   let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(32);
   ```

2. Spawn a bridge task inside the LocalSet that:
   - Receives `PermissionReq` from the channel
   - Creates `NatsClientProxy` for that session
   - Calls `proxy.request_permission(...)` — already works
   - Maps outcome: `AllowOnce` → `true`, `AllowAlways` → `true` + writes tool name to `session.allowed_tools` in store, everything else → `false`
   - Sends the bool back on `req.response_tx`

3. New `permission_bridge.rs` contains `handle_permission_request_nats()` — mirrors what already exists in `trogon-acp/src/main.rs` (the stdio runner has it wired correctly).

**Tests:** AllowOnce (true, no store write), AllowAlways (true, store updated), Denied (false), channel closed (false).

---

## Phase 2 — NATS: Fix the WebSocket Bridge (1 day)

**Files:** `acp-nats-ws/src/connection.rs`, `acp-nats/src/agent/bridge.rs`

Today `Bridge::request_permission` returns `Cancelled` immediately — the request never reaches the real client.

**Fix:** Add `perm_req_tx: mpsc::Sender<(RequestPermissionRequest, oneshot::Sender<Result<RequestPermissionResponse>>)>` to `Bridge`. Spawn a task in `connection.rs` that drains it and calls `connection.request_permission(req).await`, sends result back on the oneshot.

`Bridge::request_permission` becomes: send on `perm_req_tx`, await oneshot.

**Integration test:** mock WS client receives `session/request_permission`, replies `AllowOnce`, assert NATS reply arrives correctly.

> Phases 1 and 2 are independent — can be done in parallel.

---

## Phase 3 — NATS: Elicitations (3 days)

Approval = "I'm about to do X, allow?" (yes/no). Elicitation = "I need to know Y to continue" (structured answer, not just yes/no). Both pause the agent and wait for a human.

**New `trogon-acp-runner/src/elicitation.rs`:**
```rust
pub struct ElicitationReq {
    pub request: ElicitationRequest,
    pub response_tx: oneshot::Sender<Result<ElicitationResponse>>,
}
pub type ElicitationTx = mpsc::Sender<ElicitationReq>;
```

**New NATS subject:** `acp.session.{id}.client.session.elicitation`

**New `acp-nats/src/client/session_elicitation.rs`:** `ElicitationClient` trait + `handle()` — same pattern as `request_permission.rs`.

**`Cargo.toml` (acp-nats, trogon-acp-runner, acp-nats-ws):** enable `unstable_elicitation` feature on `agent-client-protocol`. The types already exist in the schema crate behind that flag.

Wire in `main.rs` and WS bridge using the same channel pattern as Phases 1 and 2.

---

## Phase 4 — HTTP: Add the Permission Hook to `trogon-agent`'s AgentLoop (1 day)

`trogon-agent/src/agent_loop.rs` has its own `AgentLoop` struct — different from `trogon-agent-core`'s — with no permission support at all.

**`trogon-agent/src/agent_loop.rs`:**
- Add `permission_checker: Option<Arc<dyn PermissionChecker>>` field (import trait from `trogon-agent-core`)
- Hook into tool dispatch at line 2143 — same pattern as `trogon-agent-core/src/agent_loop.rs:734`

No user-visible effect yet. Just puts the hook in place for Phase 5.

> This phase is independent of 1–3 and can be done alongside Phase 3.

---

## Phase 5 — HTTP: Streaming + Approval Gates (3–4 days)

These two features are **coupled on the HTTP path**. Without streaming, there is no clean way to pause a `send_message` request, surface an approval request to the client, and resume.

**How the flow works:**

1. Client calls `POST /sessions/{id}/messages` — response is now an SSE stream
2. Agent runs, hits a sensitive tool
3. Stream emits:
   ```
   event: approval_required
   data: {"tool_call_id":"…","tool_name":"…","tool_input":{…}}
   ```
4. Client calls `POST /sessions/{id}/approvals/{tool_call_id}` with `{"decision": "allow_once"|"allow_always"|"deny"}`
5. Agent resumes, stream continues, eventually emits `event: done`

**New `trogon-agent/src/http_permission_checker.rs`:**
```rust
pub struct HttpPermissionChecker {
    sse_tx: SseSender,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
}
```

**New endpoint:** `POST /sessions/{id}/approvals/{tool_call_id}` — looks up the pending oneshot by `tool_call_id`, sends the decision.

**`chat_api.rs`:** install `HttpPermissionChecker` before running the agent loop when the client requests SSE (`Accept: text/event-stream`).

---

## Phase 6 — HTTP: Elicitations (2 days)

Same pattern as Phase 5:

- SSE event:
  ```
  event: elicitation_required
  data: {"elicitation_id":"…","question":"…","schema":{…}}
  ```
- New endpoint: `POST /sessions/{id}/elicitations/{elicitation_id}` — client sends structured answer
- Agent receives the answer and continues

---

## Sequencing and Dependencies

```
Phase 1 (NATS permission bridge)  ─┐
Phase 2 (NATS WS bridge fix)      ─┴─ parallel ──→ Phase 3 (NATS elicitations)

Phase 4 (HTTP AgentLoop hook)         ← independent, start alongside Phase 3
                                                          ↓
                                               Phase 5 (HTTP streaming + gates)
                                                          ↓
                                               Phase 6 (HTTP elicitations)
```

NATS path fully functional after Phases 1–3 (~5 days).
HTTP path completes coverage after Phases 4–6 (~6–7 days more).

---

## Effort Summary

| Phase | What | Effort |
|---|---|---|
| 1 | NATS permission bridge | 1 day |
| 2 | NATS WebSocket bridge fix | 1 day |
| 3 | NATS elicitations | 3 days |
| 4 | HTTP AgentLoop hook | 1 day |
| 5 | HTTP streaming + approval gates | 3–4 days |
| 6 | HTTP elicitations | 2 days |
| **Total** | | **~11–12 days** |
