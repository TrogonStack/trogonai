# Gap C — spawn_agent Sub-Agent Full Tool Loop: Implementation Plan

## Problem

PR 12 and PR 16 implement `agent.spawn` as a one-shot API call — no tool-use loop.
For programming sub-agents that need to read files or run commands, this is
insufficient. A full tool-using session requires a different spawn architecture.

### Current state by runner

| Runner | Current behavior | Problem |
|---|---|---|
| xai-runner | Rejects spawn_agent with error | Does not pass NATS or prefix to tool dispatch |
| openrouter-runner | Rejects spawn_agent with error | Same |
| trogon-acp-runner (Claude) | SpawnAgentTool not registered — spawn_agent returns error | Claude never reaches spawn_handler |
| codex-runner | Subprocess handles tools internally | Protocol is observational — no external tool dispatch |

---

## Solution

Replace the one-shot HTTP spawn with full NATS sub-sessions for all runners.

**Unified mechanism**: when spawn_agent is called, the runner creates a new session
on the same runner prefix via the standard ACP NATS protocol (`session.new` →
`session.prompt` → `session.close`). The sub-agent runs in its own session with
the full tool loop.

The implementation uses the existing `Bridge` from `acp-nats`. Bridge already
implements JetStream, keepalives, heartbeats, response subjects, and
`MaxTurnRequests` handling — nothing is re-implemented. Two shared functions
in `trogon-runner-tools/src/spawn_session.rs` — `create_sub_session` and
`run_sub_session` — accept a pre-instantiated Bridge and handle the sub-session
lifecycle. They are split so the interceptor (in each runner) can write
`spawn_depth` to the sub-session between the two calls.

```
spawn_agent tool call
        │
        ▼
interceptor in runner (before dispatch_tool)
        │
        ├─ 1. check spawn_depth — reject if >= MAX_SPAWN_DEPTH
        ├─ 2. read parent cwd, mode, permissions from session
        ├─ 3. git worktree add  (filesystem isolation)
        ├─ 4. create notification channel → spawn forwarding task → create Bridge
        ├─ 5. Bridge: session.new  (plain NATS request-reply)
        ├─ 6. set spawn_depth on sub-session
        ├─ 7. Bridge: session.prompt  (JetStream, full ReAct loop with tools)
        │         └─ sub-agent notifications → Bridge → forwarding task → parent
        ├─ 8. Bridge: session.close
        └─ 9. git worktree remove
```

The call to `session.new` on the runner's own prefix is self-referential: the same
runner instance receives and handles it. This is safe because each message is
dispatched in its own `spawn_local` task (verified in acp-nats-agent/connection.rs),
and the sessions mutex is never held across an await point (explicit pattern in
xai-runner/src/agent.rs:1906).

---

## Phase 1 — Shared spawn implementation using Bridge

### `create_sub_session` and `run_sub_session` in trogon-runner-tools

The spawn logic is split into two shared functions in
`trogon-runner-tools/src/spawn_session.rs`. Neither function takes any
runner-specific types. The interceptor (in each runner) orchestrates the two
calls and handles the runner-specific part (setting `spawn_depth`) in between.

This split is required because spawn_depth must be written to the sub-session
AFTER `session.new` returns and BEFORE `session.prompt` starts — a window that
only exists between the two calls, which the interceptor controls.

```rust
// trogon-runner-tools/src/spawn_session.rs

/// Creates a sub-session and returns its session_id.
/// On failure the caller's TempWorktree Drop impl handles cleanup.
pub async fn create_sub_session<N, C, J>(
    bridge: &Bridge<N, C, J>,
    cwd: &str,
    mode: &str,
    permission_rules_text: Option<&str>,
) -> Result<String, String>
where /* Bridge bounds */
{
    let resp = bridge.new_session(NewSessionRequest {
        cwd: cwd.to_string(),
        mode: Some(mode.to_string()),
        permission_rules_text: permission_rules_text.map(|s| s.to_string()),
        ..Default::default()
    }).await.map_err(|e| e.to_string())?;
    Ok(resp.session_id.to_string())
}

/// Runs the sub-session's full ReAct loop and closes the session.
/// close_session always runs — prompt result is collected without ?.
/// The outer timeout is a safety net: the runner's own prompt_timeout
/// is the real constraint. The safety net fires only if both the runner
/// and the Bridge JetStream keepalives fail simultaneously.
pub async fn run_sub_session<N, C, J>(
    bridge: &Bridge<N, C, J>,
    sub_sid: &str,
    task: &str,
    timeout: Duration,
) -> Result<String, String>
where /* Bridge bounds */
{
    let text = tokio::time::timeout(timeout,
        bridge.prompt(PromptRequest {
            session_id: SessionId::new(sub_sid),
            text: task.to_string(),
        })
    ).await
    .map_err(|_| format!("spawn_agent safety-net timeout after {}s", timeout.as_secs()))
    .and_then(|r| r.map(|r| r.text).map_err(|e| e.to_string()));

    bridge.close_session(CloseSessionRequest {
        session_id: SessionId::new(sub_sid),
    }).await.ok();  // always runs regardless of prompt result

    text
}
```

### Notification forwarding via Bridge

Bridge reads sub-agent notifications from the runner's NATS notification subjects
and delivers them to `notification_sender` (verified in acp-nats/agent/prompt.rs).

`NatsSessionNotifier.notify()` routes dynamically based on `notification.session_id`
(verified in trogon-xai-runner/src/session_notifier.rs:47). To forward sub-agent
notifications to the parent's notification stream, the interceptor:

1. Clones `self.notifier` (the runner's shared `Arc<NatsSessionNotifier>`)
2. Rewrites the `session_id` in each notification to the parent's session_id
3. Calls `notifier.notify(modified_notif)` which publishes to the parent's subjects

```rust
// In the interceptor, before creating Bridge:
let (notif_tx, mut notif_rx) = mpsc::channel::<SessionNotification>(64);
let notifier = self.notifier.clone();      // Arc<NatsSessionNotifier> — cloneable
let parent_sid_for_fwd = session_id.clone();

// spawn_local because NatsSessionNotifier is ?Send (uses async_trait(?Send))
tokio::task::spawn_local(async move {
    while let Some(mut notif) = notif_rx.recv().await {
        // Route to parent's notification subjects, not sub-session's
        notif.session_id = SessionId::new(parent_sid_for_fwd.clone());
        notifier.notify(notif).await;
    }
});
```

**Note for implementation**: verify that the subject published by
`NatsClientProxy::session_notification` (which `NatsSessionNotifier.notify()`
uses internally) matches the subject that the parent's consumer filters on.
`session_notification` publishes to `{prefix}.session.{sid}.client.session.update`
(verified in acp-nats/src/client_proxy.rs). Confirm the parent's notification
consumer subscribes to this subject.

### Interceptor in each runner

In xai-runner and openrouter-runner, before calling `dispatch_tool`, add:

```rust
if name == "spawn_agent" {
    let task       = input["prompt"].as_str().unwrap_or("").to_string();
    let capability = input["capability"].as_str().unwrap_or("").to_string();

    // Read parent context — lock released before any await
    let (cwd, mode, perm_text, depth) = {
        let sessions = self.sessions.lock().await;
        let s = sessions.get(&session_id).ok_or("session not found")?;
        (s.cwd.clone(), s.session_mode.clone(),
         s.permission_rules_text.clone(), s.spawn_depth)
    };

    // Guard against unbounded recursive spawning
    const MAX_SPAWN_DEPTH: u32 = 3;
    if depth >= MAX_SPAWN_DEPTH {
        return Ok(format!("spawn_agent: max nesting depth ({MAX_SPAWN_DEPTH}) reached"));
    }

    // Worktree for filesystem isolation
    let worktree = create_worktree(&cwd).await;
    let sub_cwd: String = worktree.as_ref()
        .map(|w| w.path.clone())
        .unwrap_or_else(|| cwd.clone());

    // Notification forwarding (see Phase 3)
    let (notif_tx, mut notif_rx) = mpsc::channel::<SessionNotification>(64);
    let notifier = self.notifier.clone();
    let parent_sid_for_fwd = session_id.clone();
    tokio::task::spawn_local(async move {
        while let Some(mut notif) = notif_rx.recv().await {
            notif.session_id = SessionId::new(parent_sid_for_fwd.clone());
            notifier.notify(notif).await;
        }
    });

    // Build Bridge pointing at this runner's own prefix.
    // Bridge::new signature (verified in acp-nats/src/agent/bridge.rs:46):
    //   (nats, js, clock, meter, config, notification_sender)
    let nats = self.execution_nats.as_ref().ok_or("no nats")?.clone();
    let js = NatsJetStreamClient::new(async_nats::jetstream::new(nats.clone()));
    let bridge = Bridge::new(
        nats,
        js,
        SystemClock,
        &opentelemetry::global::meter("trogon-xai-runner"),
        self.runner_config.clone(),   // Config carries the runner's own prefix
        notif_tx,
    );

    // Step 1: create sub-session (shared, no runner-specific types)
    let sub_sid = match spawn_session::create_sub_session(
        &bridge, &sub_cwd, &mode, perm_text.as_deref(),
    ).await {
        Ok(sid) => sid,
        Err(e) => {
            drop(worktree);  // Drop fires cleanup
            return Ok(format!("spawn_agent: session creation failed: {e}"));
        }
    };

    // Step 2: set spawn_depth — runner-specific, between create and run
    {
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get_mut(&sub_sid) {
            s.spawn_depth = depth + 1;
        }
    }

    // Step 3: run full ReAct loop (shared, no runner-specific types)
    let result = spawn_session::run_sub_session(
        &bridge, &sub_sid, &task,
        Duration::from_secs(3600), // safety net only
    ).await;

    // Worktree cleanup — Drop fires on all exit paths
    drop(worktree);

    return result;
}
```

### New field: `runner_config` in XaiAgent / OpenRouterAgent

Replace the proposed `runner_prefix: String` with `runner_config: Config`. Config
carries the prefix AND enables Bridge construction without additional parameters.

```rust
// In XaiAgent struct:
runner_config: acp_nats::Config,

// In main.rs (AcpPrefix and nats_url are already available):
let nats_config = acp_nats::NatsConfig {
    url: nats_url.clone(),
    auth: acp_nats::NatsAuth::None,
};
let runner_config = acp_nats::Config::new(acp_prefix.clone(), nats_config);
let agent = XaiAgent::new(runner_config, /* other existing params */);
```

`Config::for_prefix(acp_prefix)` (verified in acp-nats/src/config.rs:40) can
create runner-specific configs from a base config — same pattern as
`CrossRunnerSwitcher` in trogon-cli/src/cross_runner.rs.

### spawn_depth: why the split design is required

`spawn_depth` must be written to the sub-session AFTER `session.new` returns and
BEFORE `session.prompt` starts. A single `dispatch_spawn_session` function cannot
expose this window without taking runner-specific session types as parameters
(breaking the shared function contract).

The split into `create_sub_session` + `run_sub_session` exposes this window to
the interceptor, which is the only place that knows the runner-specific session
struct. The shared functions remain generic. The interceptor contains the only
runner-specific code: `sessions.get_mut(&sub_sid).spawn_depth = depth + 1`.
```

### trogon-acp-runner (Claude runner)

**Current state** (verified in trogon-acp-runner/src/agent.rs:519-604):
`SpawnAgentTool` is defined and exported but never registered as an MCP tool.
When Claude calls spawn_agent, `agent_loop` finds no entry in `mcp_dispatch`,
falls through to `dispatch_tool()`, and returns the error "spawn_agent requires
a NATS client". Claude receives an error — spawn_agent does not work at all.

The fix has two parts:

**Part 1 — Extend SpawnAgentTool with `session_id`** and register it in
`agent.rs` during session setup, following the same pattern as
`WasmRuntimeBashTool` (lines 544-563).

`SpawnAgentTool` needs one new field — `session_id` — so the spawn handler
can load the current session state (cwd, mode, permission_rules_text,
spawn_depth) from the store at call time. Storing cwd/mode directly in
SpawnAgentTool would produce stale values if the user changes mode mid-session
via `set_session_mode`.

```rust
// Extended SpawnAgentTool:
pub struct SpawnAgentTool {
    nats: async_nats::Client,
    prefix: String,
    session_id: String,   // new — used by spawn handler to load state
}

// In call_tool payload (alongside existing capability/prompt):
serde_json::json!({
    "capability": capability,
    "prompt":     prompt,
    "session_id": self.session_id,   // new
})
```

Registration in `agent.rs` (session_id is already a local variable there):

```rust
if let Some(ref nats) = self.execution_nats {
    let spawn = SpawnAgentTool::new(
        nats.clone(), acp_prefix.clone(), session_id.clone(),
    );
    let (name, orig, client) = spawn.into_dispatch();
    a.add_mcp_tools(vec![SpawnAgentTool::tool_def()], vec![(name, orig, client)]);
}
```

**Part 2 — Add a spawn handler** in `trogon-acp-runner/src/main.rs` subscribed
to `{acp_prefix}.agent.spawn`. This handler runs on the existing LocalSet
(verified in main.rs:216 — `spawn_local` is already used), so Bridge (`!Send`)
is safe to use. The handler loads current session state from the store (cwd,
mode, permission_rules_text, spawn_depth) using the `session_id` in the
request. The store is available in main.rs where it is constructed before being
passed to the agent — clone it for the handler:

```rust
// In main.rs LocalSet — can use Bridge (!Send OK in spawn_local context)
tokio::task::spawn_local(async move {
    let mut sub = nats.subscribe(format!("{acp_prefix}.agent.spawn")).await
        .expect("failed to subscribe to agent.spawn");
    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply else { continue };
        let Ok(req) = serde_json::from_slice::<serde_json::Value>(&msg.payload)
            else { continue };

        let task       = req["prompt"].as_str().unwrap_or("").to_string();
        let session_id = req["session_id"].as_str().unwrap_or("").to_string();

        // Load current state — cwd/mode/perm are always fresh from store
        let Ok(parent_state) = store.load(&session_id).await else {
            nats.publish(reply, "spawn_agent: session not found".into()).await.ok();
            continue;
        };

        // spawn_depth guard
        const MAX_SPAWN_DEPTH: u32 = 3;
        if parent_state.spawn_depth >= MAX_SPAWN_DEPTH {
            nats.publish(reply, "spawn_agent: max nesting depth reached".into()).await.ok();
            continue;
        }

        // Filesystem isolation: worktree isolates sub-agent from parent.
        // Handler is serial so parallel conflicts are not possible, but the
        // sub-agent and parent share cwd concurrently — worktree prevents
        // mutual interference. trogon-acp-runner already depends on
        // trogon-runner-tools (lib.rs:33) so create_worktree is available.
        let worktree = worktree::create_worktree(&parent_state.cwd).await;
        let sub_cwd: String = worktree.as_ref()
            .map(|w| w.path.clone())
            .unwrap_or_else(|| parent_state.cwd.clone());

        // Notification channel — forwarding is best-effort for MVP
        let (notif_tx, _notif_rx) = mpsc::channel(64);
        let bridge = Bridge::new(
            nats.clone(), js.clone(), SystemClock,
            &opentelemetry::global::meter("trogon-acp-runner"),
            config.clone(),
            notif_tx,
        );

        let sub_sid = match spawn_session::create_sub_session(
            &bridge,
            &sub_cwd,
            &parent_state.mode,
            parent_state.permission_rules_text.as_deref(),
        ).await {
            Ok(sid) => sid,
            Err(e) => {
                drop(worktree);  // Drop fires cleanup on failure
                nats.publish(reply, format!("spawn_agent error: {e}").into()).await.ok();
                continue;
            }
        };

        // Set spawn_depth on sub-session
        if let Ok(mut sub_state) = store.load(&sub_sid).await {
            sub_state.spawn_depth = parent_state.spawn_depth + 1;
            store.save(&sub_sid, &sub_state).await.ok();
        }

        let result = spawn_session::run_sub_session(
            &bridge, &sub_sid, &task, Duration::from_secs(3600),
        ).await;

        drop(worktree);  // cleanup always runs after run_sub_session

        let text = result.unwrap_or_else(|e| format!("spawn_agent error: {e}"));
        nats.publish(reply, text.into()).await.ok();
    }
});
```

The sub-session is created on `{acp_prefix}`. If MultiRunnerAgent is active,
`session.new` is routed to the correct external runner automatically — no
additional changes needed.

Deprecate `trogon-xai-runner/src/spawn_handler.rs` and
`trogon-openrouter-runner/src/spawn_handler.rs` — no longer called when
those runners are the primary model (the interceptor handles it instead).

---

## Phase 2 — Filesystem isolation: git worktrees

New shared module: `trogon-runner-tools/src/worktree.rs`

`TempWorktree` uses two cleanup paths:
- **Normal path**: `async fn cleanup()` — async, awaited on success
- **Safety net**: `impl Drop` with `std::thread::spawn` — fires on any exit path
  including early returns via `?`, cancellation, or future drop. Uses
  `std::thread::spawn` (not `std::process::Command` directly) to avoid blocking
  the LocalSet thread (xai-runner uses `spawn_local` on a LocalSet —
  confirmed in main.rs:161,166).

```rust
pub struct TempWorktree {
    pub path: String,
    parent_cwd: String,
    cleaned: bool,
}

impl TempWorktree {
    /// Normal async cleanup — call on the success/error path when in async context.
    pub async fn cleanup(mut self) {
        self.cleaned = true;
        let _ = tokio::process::Command::new("git")
            .args(["worktree", "remove", "--force", &self.path])
            .current_dir(&self.parent_cwd)
            .status().await;
    }
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        if self.cleaned { return; }
        // Best-effort cleanup on unexpected drop (cancel, early return, etc.).
        // std::thread::spawn avoids blocking the LocalSet thread.
        let path = self.path.clone();
        let parent_cwd = self.parent_cwd.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("git")
                .args(["worktree", "remove", "--force", &path])
                .current_dir(&parent_cwd)
                .status();
        });
    }
}

pub async fn create_worktree(parent_cwd: &str) -> Option<TempWorktree> {
    let is_git = tokio::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(parent_cwd)
        .output().await.ok()?.status.success();

    if !is_git { return None; }

    let uuid = uuid::Uuid::new_v4().to_string();
    let path = format!("/tmp/trogon-sub-{}", &uuid[..8]);

    let ok = tokio::process::Command::new("git")
        .args(["worktree", "add", &path, "HEAD"])
        .current_dir(parent_cwd)
        .status().await.ok()?.success();

    if ok {
        Some(TempWorktree { path, parent_cwd: parent_cwd.to_string(), cleaned: false })
    } else {
        None
    }
}
```

In `dispatch_spawn_session`, worktree cleanup is automatic via Drop:

```rust
let worktree = create_worktree(parent_cwd).await;
let sub_cwd: String = worktree.as_ref()
    .map(|w| w.path.clone())
    .unwrap_or_else(|| parent_cwd.to_string());

let result = async { /* ... */ }.await;

// Drop fires here — cleanup() on success path, Drop impl on failure path.
// Both paths are covered.
drop(worktree);

result
```

---

## Phase 3 — Streaming: forward sub-agent notifications to parent

Notifications from the sub-agent flow via Bridge → `notification_sender` →
forwarding task → `self.notifier.notify(modified_notif)` with `session_id`
rewritten to the parent's session_id.

`NatsSessionNotifier.notify()` reads `notification.session_id` dynamically
(verified in session_notifier.rs:47) and routes to the corresponding NATS subjects.
The JetStream notifications stream covers `{prefix}.session.*.agent.update.>`
(verified in acp-nats/src/jetstream/streams.rs test line 105).

**Verify during implementation**: confirm that `NatsClientProxy::session_notification`
(which `NatsSessionNotifier.notify()` calls internally) publishes to a subject that
the parent's notification consumer is already subscribed to. The published subject
is `{prefix}.session.{sid}.client.session.update` (verified in
acp-nats/src/client_proxy.rs test line 195). The parent's consumer subject filter
must cover this. If there is a mismatch, use direct NATS publish to the parent's
known update subject instead.

---

## Phase 4 — Permission inheritance

Permissions are passed to the sub-session via `parent_mode` and
`parent_permission_rules_text` in the `session.new` request. The sub-agent
cannot exceed the parent's permission level.

Optional extension (Phase 4b): restrict by capability type:

```rust
let sub_mode = match capability.as_str() {
    "explore" => "plan",       // read-only: no writes, no bash
    "build"   => parent_mode,  // same as parent
    _         => parent_mode,
};
```

---

## Phase 5 — codex-runner: excluded

codex-runner has a fundamentally different architecture. The `codex app-server`
subprocess runs the model AND executes tools internally. TrogonAI communicates
via JSON-RPC over stdin/stdout and only observes tool execution:

```
TrogonAI → subprocess:  thread/start, turn/start, turn/interrupt
subprocess → TrogonAI:  item/updated (tool started — observational)
                        item/completed (tool result — observational)
                        turn/completed
```

There is no external tool dispatch mechanism. The subprocess never sends a
request asking TrogonAI to execute a tool. TrogonAI cannot intercept tool calls
or inject tool results mid-turn. `spawn_agent` cannot be registered as an
external tool with the current protocol.

The proactive multi-turn delegation alternative (model signals delegation at end
of turn, TrogonAI orchestrates a sub-thread and injects result as a new turn) is
not recommended: it has different semantics from reactive spawn_agent, requires
model cooperation to emit structured signals consistently, and is fragile in
practice.

**Decision**: codex-runner is excluded from Gap C. If the Codex CLI protocol adds
external tool dispatch (MCP-style), codex-runner can adopt the same Bridge pattern
as the other runners with no changes to shared code.

---

## Phase 6 — Tests

### Unit tests

| Test | File |
|---|---|
| `spawn_creates_sub_session_and_runs_tools` | `trogon-runner-tools/tests/spawn_session.rs` |
| `spawn_inherits_parent_permissions` | same |
| `spawn_cleans_up_worktree_on_new_session_failure` | same |
| `spawn_cleans_up_worktree_on_prompt_failure` | same |
| `spawn_closes_session_on_prompt_failure` | same |
| `spawn_times_out_after_safety_net_duration` | same |
| `spawn_depth_guard_rejects_at_max` | same |
| `spawn_depth_set_on_sub_session` | same |
| `spawn_worktree_created_and_removed_on_success` | `trogon-runner-tools/tests/worktree.rs` |
| `spawn_worktree_drop_triggers_cleanup` | same |
| `spawn_no_worktree_for_non_git_cwd` | same |
| `spawn_xai_interceptor_calls_dispatch` | `trogon-xai-runner/tests/spawn_integration.rs` |
| `spawn_openrouter_interceptor_calls_dispatch` | `trogon-openrouter-runner/tests/spawn_integration.rs` |
| `spawn_acp_runner_handler_creates_full_session` | `trogon-acp-runner/tests/spawn_handler_integration.rs` |
| `spawn_acp_runner_handler_depth_guard_fires` | same |
| `spawn_acp_runner_handler_cleans_up_worktree_on_failure` | same |

### Integration tests (live)

```
T172 — spawn_agent reads file (xai-runner): parent prompts "use spawn_agent to
       read src/main.rs and summarize its purpose"; verify sub-agent executed
       read_file tool; verify parent received sub-agent notifications; verify
       result contains actual file content.

T173 — spawn_agent runs bash: sub-agent executes bash command; result returned
       to parent.

T174 — spawn_agent in plan mode inherits mode: parent in plan mode spawns
       sub-agent; sub-agent cannot write files.

T175 — parallel spawn_agents use separate worktrees: two spawn_agent calls
       in same parent session; each gets its own worktree path.

T176 — spawn_agent via trogon-acp: Claude runner spawns sub-agent; sub-agent
       routed to correct external runner via MultiRunnerAgent.

T177 — spawn_depth guard: sub-agent that tries to spawn another sub-agent at
       depth >= MAX_SPAWN_DEPTH receives error, does not recurse.
```

---

## Implementation order

```
Phase 1 (core: Bridge-based dispatch + runner_config + interceptor + spawn_depth)
        ─┬─► Phase 3 (verify notification subject routing)
          └─► Phase 4 (permissions)  ──────────────────────► Phase 6 (tests)

Phase 2 (worktrees + Drop) ──► independent, parallel with Phase 1

Phase 5 (codex-runner) ──► excluded pending upstream protocol change
```

Phase 1 is the MVP that resolves Gap C for xai-runner, openrouter-runner, and
trogon-acp. Phase 2 adds filesystem isolation with safe cleanup. Phase 3 requires
a one-time subject routing verification before finalizing the notification forward.

---

## Files modified

| File | Change |
|---|---|
| `trogon-runner-tools/src/spawn_session.rs` | New — `create_sub_session` + `run_sub_session` |
| `trogon-runner-tools/src/worktree.rs` | New — git worktree with Drop safety net |
| `trogon-runner-tools/src/lib.rs` | Export `spawn_session` and `worktree` modules; re-export `create_worktree` from `spawn_session` |
| `trogon-xai-runner/src/agent.rs` | Add `runner_config: Config` + `spawn_depth` in XaiSession + interceptor |
| `trogon-xai-runner/src/main.rs` | Construct `Config` and pass to `XaiAgent::new` |
| `trogon-openrouter-runner/src/agent.rs` | Same changes as xai-runner |
| `trogon-openrouter-runner/src/main.rs` | Same as xai-runner main.rs |
| `trogon-runner-tools/src/spawn_agent_tool.rs` | Add `session_id` field + include in NATS payload |
| `trogon-runner-tools/src/session_store.rs` | Add `spawn_depth: u32` field with `#[serde(default)]` to `SessionState` |
| `trogon-acp-runner/src/agent.rs` | Register `SpawnAgentTool` with `session_id` (execution_nats guard) |
| `trogon-acp-runner/src/main.rs` | Add spawn handler in LocalSet — loads state from store, uses Bridge |
| `trogon-acp-runner/tests/spawn_handler_integration.rs` | New — spawn handler unit tests |
| `trogon-xai-runner/src/spawn_handler.rs` | Mark deprecated |
| `trogon-openrouter-runner/src/spawn_handler.rs` | Mark deprecated |
| `trogon-runner-tools/tests/spawn_session.rs` | New — unit tests for create/run functions |
| `trogon-runner-tools/tests/worktree.rs` | New — worktree + Drop tests |
| `trogon-xai-runner/tests/spawn_integration.rs` | New — interceptor tests |
| `trogon-openrouter-runner/tests/spawn_integration.rs` | New — interceptor tests |
| `rsworkspace/live_tests.py` | T172–T177 integration tests |
