# IDE Multi-Runner — Implementation Plan
# Resolve `trogon-acp` Claude-only limitation

> All work is additive. NATS remains as backend without changes.
> `agent-client-protocol` is an external crate at fixed version `=0.10.4` — cannot be modified.
> `acp-nats` is a local workspace crate — can be modified.

---

## Objective

`trogon-acp` (IDE client for Zed) is currently Claude-only. Its `prompt()` and `cancel()` methods
are hardcoded to a single `Bridge` pointing at `acp-runner`. `set_session_model()` only knows
the 3 Claude models in `AVAILABLE_MODELS`.

After this PR, the IDE can:
- See models from all 4 runners (acp, xai, openrouter, codex) in the model selector
- Route prompts and cancels to the correct runner for each session
- Switch runners mid-session (export history → new session on target runner → import)
- Receive streaming notifications from any runner correctly labeled with the ACP session id

**Default runner.** `new_session` automatically creates a runner session for `default_model`
(read from `AGENT_MODEL` env var, set by the operator at deploy time). This mirrors industry
practice: Copilot, Cursor, Continue.dev, Aider, and OpenCode all start with a pre-selected
model so the user can prompt immediately. Because all 4 runners start together, `default_model`
is guaranteed to resolve at session creation time.

The user can change the model at any point via `set_session_model` — this triggers history
migration to the new runner. If `new_session` cannot resolve the default (registry temporarily
unavailable), session creation still succeeds and the user must select a model before the
first prompt.

---

## Why existing approaches don't work

| Approach | Problem |
|---|---|
| Modify `PromptRequest.session_id` | External crate, fixed version — impossible |
| Use `CrossRunnerSwitcher` directly | Drops `_rx` immediately — all streaming notifications lost |
| New NATS router microservice | Over-engineering, doesn't solve id remapping |
| Move routing into `acp-runner` | Same complexity, wrong layer — acp-runner only knows Anthropic API |
| acp-runner as default runner | Creates a session that is immediately abandoned when user picks a non-Claude model |

---

## Solution: two additive layers

### Layer 1 — `acp-nats` (local crate)

Add two new methods to `Bridge` that accept an explicit `target_session_id`, bypassing
`args.session_id`. The existing `prompt()` and `cancel()` methods are unchanged.

### Layer 2 — `trogon-acp/src/agent.rs`

Add a bridge pool + routing table. `TrogonAcpAgent` keeps one `Bridge` per runner prefix,
all created on demand. `new_session` registers the acp_sid only. `set_session_model` creates
the runner session. `prompt()` and `cancel()` consult the routing table. Notifications from
all bridges share a single channel; a remap loop translates `runner_sid → acp_sid` before
forwarding to the IDE.

Because `Bridge` is `!Send` (contains `RefCell`), all shared state uses `Rc<RefCell<...>>`
and the entire agent runs on a `tokio::task::LocalSet`, consistent with the existing design.

---

## Files to modify

| File | Change |
|---|---|
| `acp-nats/src/agent/bridge.rs` | Add `prompt_to`, `cancel_to` methods |
| `trogon-acp/src/agent.rs` | Bridge pool, routing table, dynamic models, auto-create runner for default model |
| `trogon-acp/src/main.rs` | Pass shared `notification_sender` to all bridges; remap loop |
| `trogon-acp/Cargo.toml` | Add `trogon-registry` dependency |

`acp-nats/src/agent/prompt.rs` and `acp-nats/src/agent/cancel.rs` are **not modified**.
`prompt_to` and `cancel_to` mutate `args.session_id` before calling the existing
`prompt()` / `cancel()` — no changes to the underlying `run()` functions needed.

---

## Layer 1 — `acp-nats/src/agent/bridge.rs`

### New methods on `Bridge`

`PromptRequest.session_id` and `CancelNotification.session_id` are both `pub` fields.
`prompt_to` and `cancel_to` mutate `args.session_id = target_session_id` before
delegating to the existing `prompt()` / `cancel()`. This ensures both the NATS subject
and the serialized payload carry `runner_sid` — the ID the runner uses to look up the
session in its store (`acp-runner/src/agent.rs:432`: `let session_id = req.session_id`).

```rust
pub async fn prompt_to(
    &self,
    mut args: PromptRequest,
    target_session_id: &str,
) -> Result<PromptResponse> {
    // SessionId has no From<&str> for non-static lifetimes — use SessionId::new which
    // accepts impl Into<Arc<str>>, and Arc<str>: From<&str> is in stdlib.
    args.session_id = SessionId::new(target_session_id);
    agent_client_protocol::Agent::prompt(self, args).await
}

pub async fn cancel_to(
    &self,
    mut args: CancelNotification,
    target_session_id: &str,
) -> Result<()> {
    args.session_id = SessionId::new(target_session_id);
    agent_client_protocol::Agent::cancel(self, args).await
}
```

Existing `prompt()` and `cancel()` are unchanged. `prompt::run` and `cancel::run`
are not modified.

---

## Layer 2 — `trogon-acp/src/agent.rs`

### Updated fields on `TrogonAcpAgent`

The old `bridge: Bridge<N, C, J>` field (hardcoded to acp-runner) is removed. acp-runner
is just another entry in `runner_bridges`, created on demand like any other runner.

```rust
pub struct TrogonAcpAgent<N, C, J, S, Notif> {
    // ── existing fields kept as-is ─────────────────────────────────────────
    store: S,
    notifier: Notif,
    prefix: String,
    notification_sender: mpsc::Sender<SessionNotification>,
    default_model: String,
    gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    terminal_output_cap: Cell<bool>,
    stdio_bridges: Arc<Mutex<HashMap<String, Vec<StdioMcpBridge>>>>,

    // ── REMOVED ────────────────────────────────────────────────────────────
    // bridge: Bridge<N, C, J>   ← removed; acp-runner joins the pool below

    // ── NEW: bridge factory fields ─────────────────────────────────────────
    /// Cloned into each Bridge created on demand (N: Clone required)
    nats: N,
    /// Cloned into each Bridge created on demand (J: Clone required)
    js: J,
    /// Cloned into each Bridge created on demand (C: Clone — SystemClock is Clone)
    clock: C,
    /// Template config. Use .for_prefix(acp_prefix) to create per-runner configs.
    /// Config::for_prefix() preserves all timeouts and only replaces the prefix.
    base_config: acp_nats::Config,

    // ── NEW: multi-runner routing (Rc because Bridge is !Send, agent on LocalSet) ──
    /// acp_sid → (runner_prefix, runner_sid)
    /// Sessions absent here have no runner assigned yet.
    active_sessions: Rc<RefCell<HashMap<String, (String, String)>>>,
    /// runner_sid → acp_sid — used by the notification remap logic in main.rs
    pub(crate) id_remap: Rc<RefCell<HashMap<String, String>>>,
    /// runner_prefix → Rc<Bridge<N, C, J>> (one bridge per runner, created on demand)
    /// Stored as Rc so callers clone the Rc and release the RefCell borrow before
    /// any .await — holding a Ref<Bridge> across .await panics at runtime when
    /// another spawn_local task calls borrow_mut during the suspension.
    runner_bridges: Rc<RefCell<HashMap<String, Rc<Bridge<N, C, J>>>>>,

    // ── NEW: registry ──────────────────────────────────────────────────────
    registry: trogon_registry::Registry<async_nats::jetstream::kv::Store>,
}
```

### `TrogonAcpAgent::new` — updated constructor

`bridge` is removed; the factory fields and `registry` are added. All new `Rc` and `Arc`
fields are initialized to empty collections.

```rust
pub fn new(
    nats: N,
    js: J,
    clock: C,
    base_config: acp_nats::Config,
    store: S,
    notifier: Notif,
    prefix: impl Into<String>,
    notification_sender: mpsc::Sender<SessionNotification>,
    default_model: impl Into<String>,
    gateway_config: Arc<RwLock<Option<GatewayConfig>>>,
    registry: trogon_registry::Registry<async_nats::jetstream::kv::Store>,
) -> Self {
    Self {
        nats,
        js,
        clock,
        base_config,
        store,
        notifier,
        prefix: prefix.into(),
        notification_sender,
        default_model: default_model.into(),
        gateway_config,
        terminal_output_cap: Cell::new(false),
        stdio_bridges: Arc::new(Mutex::new(HashMap::new())),
        active_sessions: Rc::new(RefCell::new(HashMap::new())),
        id_remap: Rc::new(RefCell::new(HashMap::new())),
        runner_bridges: Rc::new(RefCell::new(HashMap::new())),
        registry,
    }
}
```

---

### `new_session()` — create session + auto-assign default runner

`InitializeResponse` has no `models` field — model state lives on `NewSessionResponse`
(and `LoadSessionResponse`, `ForkSessionResponse`) as `Option<SessionModelState>`.
`initialize()` needs **no changes** for multi-runner support.

`new_session()` saves the KV entry, then best-effort creates a runner session for
`default_model`. The runner creation is wrapped in `if let Ok` — if the registry is
temporarily unavailable, the session still succeeds and the user selects a model later.

Because `new_session` calls `create_runner_session` (which calls `get_or_create_bridge`),
the `impl Agent for TrogonAcpAgent<...>` block must add `J: Clone` and `C: Clone` to its
where clause. `N: Clone` is already present (line 624 of the current `agent.rs`).

```rust
async fn new_session(&self, args: NewSessionRequest) -> Result<NewSessionResponse>
// (requires J: Clone, C: Clone added to the impl Agent for TrogonAcpAgent where clause)
{
    let acp_sid = Uuid::new_v4().to_string();
    let state = SessionState {
        cwd: args.cwd.to_string_lossy().to_string(),
        ..Default::default()
    };
    self.store.save(&acp_sid, &state).await?;

    // Auto-assign default runner — best effort. If registry is unavailable, the session
    // still succeeds and the user must select a model before the first prompt.
    if let Ok(prefix) = self.resolve_prefix_for_model(&self.default_model).await {
        if let Ok(runner_sid) = self.create_runner_session(
            &prefix, &self.default_model, &acp_sid,
        ).await {
            self.update_routing(&acp_sid, &prefix, &runner_sid);
            // Persist chosen model to KV so load_session / fork_session see it.
            if let Ok(mut s) = self.store.load(&acp_sid).await {
                s.model = Some(self.default_model.clone());
                let _ = self.store.save(&acp_sid, &s).await;
            }
        }
    }

    // NewSessionResponse is #[non_exhaustive] — cannot use struct literal from outside crate.
    let model_state = self.build_model_state_from_registry(&self.default_model).await;
    Ok(NewSessionResponse::new(acp_sid).models(model_state))
}
```

### `build_model_state_from_registry()` helper — dynamic model list

`Registry` has no `list_models()` method. Use `list_all()` and extract
`metadata["models"]` from each `AgentCapability`. Runners register their model list
in metadata at startup (see "Runner registration" below).

`current_model` is a required parameter — it differs per call site:
- `new_session`: default model already assigned → pass `&self.default_model` (runner session created in `new_session`)
- `load_session`, `fork_session`, `resume_session`: pass `state.model.as_deref().unwrap_or(&self.default_model)`

```rust
async fn build_model_state_from_registry(&self, current_model: &str) -> SessionModelState {
    let mut available: Vec<ModelInfo> = vec![];

    if let Ok(all) = self.registry.list_all().await {
        available = all.into_iter()
            .flat_map(|cap| {
                cap.metadata.get("models")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter()
                        .filter_map(|m| m.as_str())
                        // ModelId::new accepts impl Into<Arc<str>>; Arc<str>: From<&str> (stdlib).
                        // Cannot use id.into() — ModelId only has From<Arc<str>, String, &'static str>.
                        .map(|id| ModelInfo::new(ModelId::new(id), id))
                        .collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect();
    }

    // Fallback: static Claude models if registry unavailable.
    if available.is_empty() {
        available = AVAILABLE_MODELS
            .iter()
            .map(|(id, name)| ModelInfo::new(*id, *name))
            .collect();
    }

    SessionModelState::new(current_model.to_string(), available)
}
```

### `set_session_model()` — assign or migrate runner

The routing logic lives in a private helper `set_session_model_impl`. The ACP trait
method (required by the `Agent` trait: `async fn set_session_model(&self, args:
SetSessionModelRequest) -> Result<SetSessionModelResponse>`) extracts the IDs, calls
the helper, and returns `SetSessionModelResponse::new()`.

```rust
// ACP trait implementation.
async fn set_session_model(&self, args: SetSessionModelRequest) -> Result<SetSessionModelResponse> {
    let acp_sid = args.session_id.to_string();
    let model_id = args.model_id.0.as_ref();
    self.set_session_model_impl(&acp_sid, model_id).await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    Ok(SetSessionModelResponse::new())
}
```

The private helper contains the routing logic:

```rust
async fn set_session_model_impl(&self, acp_sid: &str, model_id: &str) -> Result<()> {
    let target_prefix = self.resolve_prefix_for_model(model_id).await?;

    let current = self.active_sessions.borrow().get(acp_sid).cloned();

    match current {
        None => {
            // No runner assigned yet. Happens for: forked sessions (never had a runner) and
            // new sessions where default-model initialization failed at creation time.
            // Forked sessions already have history in KV — must import it so the LLM has context.
            let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
            let state = self.store.load(acp_sid).await?;
            if !state.messages.is_empty() {
                let portable = portable_from_messages(&state.messages);
                self.import_history(&target_prefix, &runner_sid, &portable).await?;
            }
            self.update_routing(acp_sid, &target_prefix, &runner_sid);
        }
        Some((ref current_prefix, ref current_runner_sid)) if current_prefix == &target_prefix => {
            // Same runner — update model on runner session (runner reads state.model from its
            // own KV on every prompt; acp-side KV update below does not touch the runner's store).
            let bridge = self.get_or_create_bridge(current_prefix)?;
            let _ = bridge.set_session_model(
                SetSessionModelRequest::new(current_runner_sid.clone(), model_id.to_string()),
            ).await;
        }
        Some((current_prefix, current_runner_sid)) => {
            // Different runner — clear routing first so any failure leaves session in
            // "no runner" state (recoverable via set_session_model) rather than stuck
            // pointing at a closed session.
            self.active_sessions.borrow_mut().remove(acp_sid);
            self.id_remap.borrow_mut().remove(&current_runner_sid);
            let history = self.export_history(&current_prefix, &current_runner_sid).await?;
            self.close_runner_session(&current_prefix, &current_runner_sid).await;
            let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
            self.import_history(&target_prefix, &runner_sid, &history).await?;
            self.update_routing(acp_sid, &target_prefix, &runner_sid);
            self.sync_session_to_kv(acp_sid, &history).await?;
        }
    }

    // SessionStore has no update_model — load, mutate, and save instead.
    if let Ok(mut state) = self.store.load(acp_sid).await {
        state.model = Some(model_id.to_string());
        let _ = self.store.save(acp_sid, &state).await;
    }
    Ok(())
}
```

### `load_session()`, `fork_session()`, `resume_session()` — model list from registry

These three methods currently call `Self::build_model_state(current_model)` which uses
the hardcoded `AVAILABLE_MODELS` constant. After this PR that list only contains Claude
models, so the IDE model picker would show only Claude models when loading or forking a
session — preventing the user from selecting an xai/openrouter/codex model.

**Fix:** replace `Self::build_model_state(current_model)` with
`self.build_model_state_from_registry(current_model).await` at all three call sites.
`current_model` already exists at each site:

```rust
// load_session — existing line 830:
let current_model = state.model.as_deref().unwrap_or(&self.default_model);
// replace line 834:
let models = self.build_model_state_from_registry(current_model).await;

// fork_session — existing line 1194:
let current_model = new_state.model.as_deref().unwrap_or(&self.default_model);
// replace line 1199:
.models(self.build_model_state_from_registry(current_model).await)

// resume_session — existing line 1230:
let current_model = state.model.as_deref().unwrap_or(&self.default_model);
// replace line 1235:
.models(self.build_model_state_from_registry(current_model).await)
```

All three methods are `async fn` — calling `.await` requires no structural change.

---

### `prompt()` — routing

```rust
async fn prompt(&self, args: PromptRequest) -> Result<PromptResponse> {
    let acp_sid = args.session_id.to_string();

    let assigned = self.active_sessions.borrow().get(&acp_sid).cloned();

    let (prefix, runner_sid) = match assigned {
        Some(entry) => entry,
        None => {
            // set_session_model must be called before prompt.
            return Err(anyhow!("no model selected for session {acp_sid}"));
        }
    };

    let resp = self.get_or_create_bridge(&prefix)?.prompt_to(args, &runner_sid).await?;
    self.post_prompt_sync_kv(&acp_sid, &prefix, &runner_sid).await;
    Ok(resp)
}
```

### `cancel()` — routing

```rust
async fn cancel(&self, args: CancelNotification) -> Result<()> {
    let acp_sid = args.session_id.to_string();
    let entry = self.active_sessions.borrow().get(&acp_sid).cloned();

    match entry {
        None => Ok(()), // no runner active, nothing to cancel
        Some((prefix, runner_sid)) => {
            self.get_or_create_bridge(&prefix)?.cancel_to(args, &runner_sid).await
        }
    }
}
```

### `create_runner_session()` helper

`SessionStore` has no `get_cwd` method — use `store.load().cwd` instead.
`NewSessionRequest` has no `model` field — set the model after creating the session
via `set_session_model`. `resp.session_id` is `SessionId` (not `String`) — call `.to_string()`.

```rust
async fn create_runner_session(
    &self,
    prefix: &str,
    model_id: &str,
    acp_sid: &str,
) -> anyhow::Result<String>
where
    N: Clone,
    J: Clone,
    C: Clone,
{
    let cwd = self.store.load(acp_sid).await
        .map_err(|e| anyhow::anyhow!("load session: {e}"))?.cwd;
    let bridge = self.get_or_create_bridge(prefix)?;
    let resp = bridge.new_session(NewSessionRequest::new(cwd))
        .await.map_err(|e| anyhow::anyhow!("new_session on runner: {e}"))?;
    let runner_sid = resp.session_id.to_string();
    // Set the model now — NewSessionRequest has no model field.
    // runner_sid.clone(): &String is not Into<SessionId>; String is. Clone to keep runner_sid for return.
    // model_id.to_string(): non-static &str is not Into<ModelId>; only &'static str is. String is.
    let _ = bridge.set_session_model(
        SetSessionModelRequest::new(runner_sid.clone(), model_id.to_string()),
    ).await;
    Ok(runner_sid)
}
```

### `update_routing()` helper

```rust
fn update_routing(&self, acp_sid: &str, prefix: &str, runner_sid: &str) {
    self.active_sessions
        .borrow_mut()
        .insert(acp_sid.to_string(), (prefix.to_string(), runner_sid.to_string()));
    self.id_remap
        .borrow_mut()
        .insert(runner_sid.to_string(), acp_sid.to_string());
}
```

### `close_runner_session()` helper

Closes the session on the runner and removes the `id_remap` entry. Called on migration
and on `close_session` if a runner is assigned.

```rust
async fn close_runner_session(&self, prefix: &str, runner_sid: &str) {
    let bridge = self.runner_bridges.borrow().get(prefix).map(Rc::clone);
    if let Some(bridge) = bridge {
        // CloseSessionRequest::new accepts impl Into<SessionId>.
        // runner_sid is &str (non-static) — not Into<SessionId>; use .to_string() → String: Into<SessionId>.
        let _ = bridge.close_session(CloseSessionRequest::new(runner_sid.to_string())).await;
    }
}
```

### `close_session_impl()` — updated cleanup

The existing `close_session_impl` only shuts down stdio bridges, publishes cancel/cancelled,
and deletes from KV. After this PR it must also remove the session from `active_sessions`,
remove its `runner_sid` from `id_remap`, and close the runner session to free runner-side memory.

Both `borrow_mut()` calls are statement-level temporaries — the `RefMut` guard is dropped
before any `.await`, so no `RefCell` panic.

```rust
async fn close_session_impl(&self, session_id: &str) {
    // NEW: clean up routing and close runner session.
    let entry = self.active_sessions.borrow_mut().remove(session_id);
    if let Some((prefix, runner_sid)) = entry {
        self.id_remap.borrow_mut().remove(&runner_sid);
        self.close_runner_session(&prefix, &runner_sid).await;
    }

    // existing: shut down stdio MCP bridges
    if let Some(bridges) = self.stdio_bridges.lock().await.remove(session_id) {
        for bridge in bridges { bridge.shutdown().await; }
    }
    // existing: publish cancel/cancelled and delete from KV
    let acp_prefix = AcpPrefix::new(&self.prefix).expect("valid prefix");
    let acp_session_id = AcpSessionId::new(session_id).expect("valid session_id");
    let cancel_subject = session_subjects::agent::CancelSubject::new(&acp_prefix, &acp_session_id).to_string();
    self.notifier.publish(cancel_subject, bytes::Bytes::new()).await;
    let cancelled_subject = session_subjects::agent::CancelledSubject::new(&acp_prefix, &acp_session_id).to_string();
    self.notifier.publish(cancelled_subject, bytes::Bytes::new()).await;
    if let Err(e) = self.store.delete(session_id).await {
        Self::warn_delete_session_failed(session_id, &e);
    }
}
```

---

### `export_history()` / `import_history()` helpers

`session/export` and `session/import` are not part of the standard ACP trait — they are
extension methods dispatched via `bridge.ext_method(ExtRequest::new("session/export", ...))`.
`ExtRequest::new` takes `(method: impl Into<Arc<str>>, params: Arc<RawValue>)`.
`RawValue::from_string` returns `Box<RawValue>`; `.into()` converts to `Arc<RawValue>` via
`impl<T: ?Sized> From<Box<T>> for Arc<T>` (stdlib).

```rust
async fn export_history(&self, prefix: &str, runner_sid: &str) -> Result<Vec<PortableMessage>> {
    let bridge = self.get_or_create_bridge(prefix)?;
    // Runners expect { "sessionId": "..." } — acp-runner uses params["sessionId"].as_str()
    let params = serde_json::value::RawValue::from_string(
        serde_json::json!({ "sessionId": runner_sid }).to_string()
    )?;
    let resp = bridge.ext_method(ExtRequest::new("session/export", params.into())).await?;
    // Response is a JSON array of PortableMessage — resp.0 is Arc<RawValue>, .get() gives &str
    Ok(serde_json::from_str(resp.0.get())?)
}

async fn import_history(
    &self,
    prefix: &str,
    runner_sid: &str,
    history: &[PortableMessage],
) -> Result<()> {
    let bridge = self.get_or_create_bridge(prefix)?;
    let messages_json = serde_json::to_string(history)?;
    // Runners expect { "sessionId": "...", "messages": [...] } — same format as CrossRunnerSwitcher
    let params = serde_json::value::RawValue::from_string(
        format!(r#"{{"sessionId":"{}","messages":{}}}"#, runner_sid, messages_json)
    )?;
    bridge.ext_method(ExtRequest::new("session/import", params.into())).await?;
    Ok(())
}
```

---

### `get_or_create_bridge()` helper

Returns `Rc<Bridge>` — NOT `Ref<Bridge>` — so the `RefCell` borrow is released before
any `.await` call on the bridge. Holding `Ref<Bridge>` across `.await` would panic at
runtime if another `spawn_local` task calls `borrow_mut` during the suspension.

Returns `anyhow::Result` because `AcpPrefix::new` can fail on an invalid prefix.

```rust
fn get_or_create_bridge(&self, prefix: &str) -> anyhow::Result<Rc<Bridge<N, C, J>>>
where
    N: Clone,
    J: Clone,
    C: Clone,
{
    if let Some(b) = self.runner_bridges.borrow().get(prefix) {
        return Ok(Rc::clone(b)); // borrow released here, Rc keeps bridge alive
    }
    // Bridge not yet created — build config then insert it.
    // AcpPrefix::new can fail on invalid prefix strings.
    let acp_prefix = acp_nats::AcpPrefix::new(prefix.to_string())
        .map_err(|e| anyhow::anyhow!("invalid acp prefix {prefix}: {e}"))?;
    let config = self.base_config.for_prefix(acp_prefix);
    let meter = opentelemetry::global::meter("trogon-acp");
    let bridge = Rc::new(Bridge::new(
        self.nats.clone(),
        self.js.clone(),
        self.clock.clone(),
        &meter,
        config,
        self.notification_sender.clone(), // shared channel — all runners notify through one mpsc
    ));
    self.runner_bridges.borrow_mut().insert(prefix.to_string(), Rc::clone(&bridge));
    Ok(bridge)
}
```

Callers propagate the error with `?`:

```rust
let bridge = self.get_or_create_bridge(&prefix)?; // Rc<Bridge>, borrow released
let resp = bridge.prompt_to(args, &runner_sid).await?; // no RefCell borrow held
```

### `post_prompt_sync_kv()` helper

After each prompt, export history from the runner and write to KV under `acp_sid`.
This keeps `load_session`, `list_sessions`, and `fork_session` consistent.

```rust
async fn post_prompt_sync_kv(&self, acp_sid: &str, prefix: &str, runner_sid: &str) {
    if let Ok(history) = self.export_history(prefix, runner_sid).await {
        let _ = self.sync_session_to_kv(acp_sid, &history).await;
    }
}
```

### `sync_session_to_kv()` helper

`session/export` returns `Vec<PortableMessage>`. KV stores `SessionState` with
`Vec<trogon_tools::Message>`. This helper converts before writing.

```rust
async fn sync_session_to_kv(&self, acp_sid: &str, history: &[PortableMessage]) -> Result<()> {
    let mut state = self.store.load(acp_sid).await?;
    state.messages = messages_from_portable(history);
    self.store.save(acp_sid, &state).await
}
```

### `messages_from_portable()` / `portable_from_messages()` helpers

Free functions in `agent.rs`. Both require `use trogon_runner_tools::portable_session::{PortableBlock, PortableMessage}`.

`messages_from_portable` converts `Vec<PortableMessage>` → `Vec<trogon_tools::Message>`.
If a `PortableMessage` has no blocks, falls back to a single `ContentBlock::Text` from the
`text` field (backward-compatible with old sessions that pre-date `PortableBlock`).

```rust
fn messages_from_portable(history: &[PortableMessage]) -> Vec<trogon_tools::Message> {
    history.iter().map(|pm| {
        let content = if pm.blocks.is_empty() {
            vec![trogon_tools::ContentBlock::Text { text: pm.text.clone() }]
        } else {
            pm.blocks.iter().map(|b| match b {
                PortableBlock::Text { text } =>
                    trogon_tools::ContentBlock::Text { text: text.clone() },
                PortableBlock::ToolCall { id, name, input } =>
                    trogon_tools::ContentBlock::ToolUse {
                        id: id.clone(), name: name.clone(), input: input.clone(),
                        parent_tool_use_id: None,
                    },
                PortableBlock::ToolResult { tool_call_id, content } =>
                    trogon_tools::ContentBlock::ToolResult {
                        tool_use_id: tool_call_id.clone(), content: content.clone(),
                    },
            }).collect()
        };
        trogon_tools::Message { role: pm.role.clone(), content }
    }).collect()
}
```

`portable_from_messages` is the reverse — converts `Vec<trogon_tools::Message>` →
`Vec<PortableMessage>`. Used in `set_session_model_impl` (None branch) to import KV
history into a new runner session. `ContentBlock::Thinking` and `ContentBlock::Image`
are discarded — no equivalent in other APIs.

```rust
fn portable_from_messages(messages: &[trogon_tools::Message]) -> Vec<PortableMessage> {
    messages.iter().map(|m| {
        let blocks: Vec<PortableBlock> = m.content.iter().filter_map(|b| match b {
            trogon_tools::ContentBlock::Text { text } =>
                Some(PortableBlock::Text { text: text.clone() }),
            trogon_tools::ContentBlock::ToolUse { id, name, input, .. } =>
                Some(PortableBlock::ToolCall {
                    id: id.clone(), name: name.clone(), input: input.clone(),
                }),
            trogon_tools::ContentBlock::ToolResult { tool_use_id, content } =>
                Some(PortableBlock::ToolResult {
                    tool_call_id: tool_use_id.clone(), content: content.clone(),
                }),
            _ => None, // Thinking, Image — discarded
        }).collect();
        let text = blocks.iter().filter_map(|b| match b {
            PortableBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("\n");
        PortableMessage { role: m.role.clone(), text, blocks }
    }).collect()
}
```

Both conversions are lossless for Text, ToolUse, and ToolResult.

---

## Layer 2 — `trogon-acp/src/main.rs`

### Shared `notification_sender`

The existing `main.rs` constructor call must be updated: remove the `bridge` argument and
add the factory fields (`nats`, `js`, `clock`, `base_config`) plus `registry`. The exact
parameter order must match the new `TrogonAcpAgent::new` signature.

```rust
let (notification_tx, mut notification_rx) =
    tokio::sync::mpsc::channel::<SessionNotification>(64);

// Provision registry — needed to query find_by_model and list_all.
// Standalone runners (trogon-acp-runner, trogon-xai-runner, etc.) self-register in
// their own main.rs via the same KV bucket; trogon-acp only reads, never writes.
// trogon_registry::provision opens (or creates) the "agent-registry" JetStream KV bucket.
// `js` is the jetstream::Context already created above (line ~86 in main.rs).
let reg_store = trogon_registry::provision(&js).await
    .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
let registry = trogon_registry::Registry::new(reg_store);

// Replace: let acp_agent = agent::TrogonAcpAgent::new(bridge, store, notifier, prefix, notification_tx, model, gateway_config);
// With (bridge removed, factory fields + registry added):
let acp_agent = agent::TrogonAcpAgent::new(
    nats.clone(),                           // nats  (factory field, replaces bridge)
    js_client,                              // js — not cloned: Bridge::new block removed, no prior consumer
    trogon_std::time::SystemClock,          // clock (SystemClock: Clone)
    config,                                 // base_config — existing acp_nats::Config (line ~168 in main.rs)
    // Config::new(AcpPrefix, NatsConfig) takes two args; acp_prefix is String not AcpPrefix.
    // The config variable already exists and is correct — pass it directly instead of reconstructing.
    store.clone(),
    NatsSessionNotifier::new(nats.clone()),
    acp_prefix,
    notification_tx.clone(),
    model.clone(),
    gateway_config,
    registry,                               // new
);
```

### Notification remap loop

`acp_stdio_sender` does not exist in `main.rs`. The remap logic must be inlined into the
**existing** notification-forwarding match arm — the one that already calls
`conn.session_notification(notification)`. No separate `spawn_local` needed.

```rust
// Inside the existing select! loop in main.rs:
maybe_notification = notification_rx.recv() => {
    match maybe_notification {
        Some(mut notification) => {
            // Remap runner_sid → acp_sid before forwarding to IDE.
            // SessionNotification.session_id is pub — mutate directly.
            // SessionId has no as_str() — access the inner Arc<str> field (.0) and call .as_ref().
            // Arc<str>: AsRef<str> → &str. HashMap<String,_>::get(&str) works via String: Borrow<str>.
            let remapped = id_remap.borrow().get(notification.session_id.0.as_ref()).cloned();
            if let Some(acp_id) = remapped {
                notification.session_id = acp_id.into();
            }
            if let Err(e) = conn.session_notification(notification).await {
                tracing::warn!(error = %e, "failed to forward session notification");
            }
        }
        None => break,
    }
}
```

`id_remap` is cloned as `Rc::clone(&acp_agent.id_remap)` before the select loop. Because
`spawn_local` / `LocalSet` keeps everything on one thread, the `Rc` is safe here.

---

### `resolve_prefix_for_model()` helper

Uses `registry.find_by_model()` (already implemented in `trogon-registry`) and extracts
`acp_prefix` from the capability metadata.

```rust
async fn resolve_prefix_for_model(&self, model_id: &str) -> Result<String> {
    let cap = self.registry
        .find_by_model(model_id).await
        .map_err(|e| anyhow!("{e}"))?
        .ok_or_else(|| anyhow!("no runner registered for model {model_id}"))?;
    cap.metadata
        .get("acp_prefix")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("runner for model {model_id} has no acp_prefix in metadata"))
}
```

---

## Runner registration requirement

Each runner must register at startup with `models` and `acp_prefix` in its
`AgentCapability` metadata. Without `acp_prefix`, `resolve_prefix_for_model` cannot
route the session to the correct Bridge.

```rust
// Example: xai-runner startup
let cap = AgentCapability {
    agent_type: "xai".to_string(),
    capabilities: vec!["chat".to_string()],
    nats_subject: "trogon.xai.>".to_string(),
    current_load: 0,
    metadata: serde_json::json!({
        "models": ["grok-3", "grok-4"],
        "acp_prefix": "trogon.xai",
    }),
};
registry.register(&cap).await?;
```

| Runner | `acp_prefix` | `models` |
|---|---|---|
| `trogon-acp-runner` | `"trogon.acp"` (or its configured prefix) | Claude models |
| `trogon-xai-runner` | `"trogon.xai"` | xAI Grok models |
| `trogon-openrouter-runner` | `"trogon.openrouter"` | OpenRouter models |
| `trogon-codex-runner` | `"trogon.codex"` | OpenAI Codex/GPT models |

**No runner changes needed.** All 4 runners already register with both fields at startup:

| Runner | Metadata at startup |
|---|---|
| `trogon-acp-runner` | `"acp_prefix": &acp_prefix, "models": [hardcoded Claude list]` |
| `trogon-xai-runner` | `"acp_prefix": &prefix, "models": parsed from XAI_MODELS env var` |
| `trogon-openrouter-runner` | `"acp_prefix": &cfg.prefix, "models": parsed from cfg.models_str` |
| `trogon-codex-runner` | `"acp_prefix": &prefix, "models": parsed from CODEX_MODELS env var` |

This PR requires no changes to any runner crate.

---

## `trogon-acp/Cargo.toml`

`trogon-runner-tools` is required for `PortableMessage` and `PortableBlock`.
`trogon-acp-runner/src/lib.rs` does not re-export `portable_session`, so the
dependency must be declared explicitly.

`trogon-tools` is required for `Message` and `ContentBlock` used in
`messages_from_portable` and `portable_from_messages`. `trogon-tools` is a
transitive dependency (via `trogon-agent-core`) but transitive crate names are not
in scope in Rust 2021 — `use trogon_tools::ContentBlock` would fail with
`error[E0433]: failed to resolve: use of undeclared crate or module trogon_tools`.

```toml
trogon-registry     = { path = "../trogon-registry" }
trogon-runner-tools = { path = "../trogon-runner-tools" }
trogon-tools        = { path = "../trogon-tools" }
```

---

## Session lifecycle summary

| Event | Before | After |
|---|---|---|
| `new_session` | Creates session on acp-runner → KV | Saves acp_sid in KV; best-effort creates runner session for `default_model` |
| `set_session_model` (no runner yet) | Not applicable | Creates session on target runner; imports KV history if any (forked sessions) |
| `set_session_model` (same runner) | Updates KV model field | Same |
| `set_session_model` (different runner) | Not supported | Export → create on target → import → update routing + KV |
| `prompt` (runner assigned) | Sends to acp-runner | Routes to assigned runner via `prompt_to` |
| `prompt` (no runner assigned) | Sends to acp-runner | Returns error: "no model selected for session {id}" |
| `cancel` (runner assigned) | Sends to acp-runner | Routes to assigned runner via `cancel_to` |
| `cancel` (no runner) | Sends to acp-runner | No-op — nothing active |
| `load_session` | Reads from KV | Unchanged — KV kept in sync by `post_prompt_sync_kv` |
| `fork_session` | Reads from KV, creates new | Unchanged — KV in sync |
| `close_session` | Closes on acp-runner | Closes on assigned runner if any; removes from `active_sessions` and `id_remap` |
| `set_session_model` (different runner) — cleanup | Not applicable | Closes old runner session + removes old `runner_sid` from `id_remap` before migrating |
| Notification arrives | Forwarded directly | Remapped `runner_sid → acp_sid` before forwarding |

---

## What is NOT in scope

- Modifying `agent-client-protocol` — external fixed crate
- Changing NATS as backend — all new bridges use NATS
- Modifying `CrossRunnerSwitcher` — CLI tool, not involved
- Changing existing acp-runner behavior — all changes are additive

---

## Gap found and fixed in this document

### Gap — `registry` used in `main.rs` but never created

**File:** `trogon-acp/src/main.rs`

**Problem:** The `main.rs` section passed `registry` to `TrogonAcpAgent::new` but never showed
how to create it. The code would not compile: `error[E0425]: cannot find value 'registry' in
this scope`.

**Fix:** Add registry provisioning before the `TrogonAcpAgent::new` call:

```rust
let reg_store = trogon_registry::provision(&js).await
    .map_err(|e| anyhow::anyhow!("registry provisioning failed: {e}"))?;
let registry = trogon_registry::Registry::new(reg_store);
```

`trogon_registry::provision` opens (or creates) the `"agent-registry"` JetStream KV bucket —
the same bucket that all 4 runners write to at startup. `trogon-acp` only reads from it
(via `find_by_model` and `list_all`); no registration or heartbeat task needed here.

**Verified against deployment model:** `docker-compose.yml` shows `trogon-acp-runner` running
as a separate service at `ACP_PREFIX: acp.claude`. It self-registers in the registry via its
own `main.rs`. Same for `trogon-xai-runner` (`acp.xai`), `trogon-openrouter-runner`
(`acp.openrouter`), `trogon-codex-runner` (`acp.codex`). `trogon-acp` is not a Docker
service — it runs on the developer's machine and connects to the server NATS. No conflict.

### Gap — where clause: only `J: Clone` and `C: Clone` are missing

**File:** `trogon-acp/src/agent.rs` — `impl Agent for TrogonAcpAgent` (current line 618)

**Problem:** The plan originally stated "add `N: Clone, J: Clone, C: Clone`". Reading the
actual where clause reveals `N` already carries `+ Clone` (line 624). Only `C` (line 628,
`GetElapsed`) and `J` (line 629, `JetStreamPublisher + JetStreamGetStream`) are missing
`+ Clone`. Generating the wrong diff would fail to compile (duplicate bound) or add
unnecessary noise.

**Fix:** The correct change to the where clause is:

```rust
// Before
C: GetElapsed + Send + Sync + 'static,
J: JetStreamPublisher + JetStreamGetStream + Send + Sync + 'static,

// After
C: GetElapsed + Clone + Send + Sync + 'static,
J: JetStreamPublisher + JetStreamGetStream + Clone + Send + Sync + 'static,
```

`N: Clone` stays as-is. Both `NatsJetStreamClient` and `SystemClock` derive `Clone` ✅.
`MockJs` in the test module also derives `Clone` ✅.

---

### Gap — `set_session_config_option("model")` broken for non-Claude models

**File:** `trogon-acp/src/agent.rs` — lines 956-969

**Problem:** The existing `"model"` branch in `set_session_config_option` calls
`Self::resolve_model(&value)`, which only knows the 3 Claude models in `AVAILABLE_MODELS`.
Any xAI, OpenRouter, or Codex model string returns `Err("Unknown model: ...")`.
Even if `resolve_model` succeeded it would only update `state.model` in KV and return —
never calling `set_session_model_impl`, so no bridge is created and no routing update happens.

**Fix:** Replace the branch body so it delegates to `set_session_model_impl`:

```rust
} else if config_id == "model" {
    self.set_session_model_impl(&session_id, &value).await
        .map_err(|e| Error::new(ErrorCode::InvalidParams.into(), e.to_string()))?;
    // Reload state — set_session_model_impl already persisted the updated model in KV.
    state = self.store.load(&session_id).await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
}
```

`set_session_model_impl` handles model resolution, bridge acquisition, runner session
creation, cross-runner export/import, and KV sync. After this change `set_session_config_option`
and `set_session_model` share the same code path for the model field.

This fix requires that `J: Clone` and `C: Clone` are already present on the `impl Agent`
where clause (see gap above), because `set_session_config_option` and
`set_session_model_impl` live in the same `impl` block.

---

### Gap — missing `where N: Clone, J: Clone, C: Clone` on four helper methods

**File:** `trogon-acp/src/agent.rs` — helper impl block (current lines 107-112)

**Problem:** The existing helper impl block has only the base bounds
(`N: RequestClient + ...`, `C: GetElapsed`, `J: JetStreamPublisher + ...`) without `Clone`.
In Rust, when method A calls method B that declares additional where clause bounds, A must
also declare those bounds — the impl block's bounds are necessary but not sufficient.

`get_or_create_bridge` and `create_runner_session` correctly declare
`where N: Clone, J: Clone, C: Clone`. But four methods that call them transitively do not:

| Method | Calls (transitively) |
|--------|----------------------|
| `export_history` | `get_or_create_bridge` directly |
| `import_history` | `get_or_create_bridge` directly |
| `post_prompt_sync_kv` | `export_history` |
| `set_session_model_impl` | `create_runner_session`, `export_history`, `import_history` |

Without the bounds, the compiler rejects the call:
`error[E0277]: the trait bound N: Clone is not satisfied`

**Fix:** Add `where N: Clone, J: Clone, C: Clone` to each of the four methods:

```rust
async fn export_history(&self, prefix: &str, runner_sid: &str) -> Result<Vec<PortableMessage>>
where N: Clone, J: Clone, C: Clone
{ ... }

async fn import_history(&self, prefix: &str, runner_sid: &str, history: &[PortableMessage]) -> Result<()>
where N: Clone, J: Clone, C: Clone
{ ... }

async fn post_prompt_sync_kv(&self, acp_sid: &str, prefix: &str, runner_sid: &str)
where N: Clone, J: Clone, C: Clone
{ ... }

async fn set_session_model_impl(&self, acp_sid: &str, model_id: &str) -> Result<()>
where N: Clone, J: Clone, C: Clone
{ ... }
```

`close_runner_session` is NOT affected — it reads an already-stored `Rc<Bridge>` from
`runner_bridges` without calling `get_or_create_bridge`, so it needs no Clone bounds.

---

### Gap — `Err(anyhow!(...))` in `prompt()` is the wrong error type

**File:** `trogon-acp/src/agent.rs` — `prompt()` in `impl Agent for TrogonAcpAgent`

**Problem:** `prompt()` returns `Result<PromptResponse>` where `Result` is
`agent_client_protocol::Result` (line 19: `use agent_client_protocol::{..., Result, ...}`),
i.e. `std::result::Result<PromptResponse, agent_client_protocol::Error>`.

The plan's `None` branch uses:
```rust
return Err(anyhow!("no model selected for session {acp_sid}"));
```

`anyhow!` produces `anyhow::Error`, not `agent_client_protocol::Error`. Rust does not
apply `From` automatically inside `Err(...)` — only `?` and `.into()` trigger `From`.
This fails to compile:
```
error[E0308]: mismatched types
  expected `agent_client_protocol::Error`, found `anyhow::Error`
```

Note: the `?` operators elsewhere in `prompt()` are correct — `get_or_create_bridge(&prefix)?`
converts `anyhow::Error → agent_client_protocol::Error` via the `From` impl on
`agent_client_protocol::Error`. Only the explicit `Err(anyhow!(...))` is wrong.

**Fix:** Use `Error::new` directly, consistent with the rest of the impl block
(e.g. `set_session_config_option` uses `Error::new(ErrorCode::InvalidParams.into(), ...)`):

```rust
None => {
    return Err(Error::new(
        ErrorCode::InternalError.into(),
        format!("no model selected for session {acp_sid}"),
    ));
}
```

---

### Gap — `trogon-tools` missing from `trogon-acp/Cargo.toml`

**File:** `trogon-acp/Cargo.toml`

**Problem:** `messages_from_portable` and `portable_from_messages` use
`trogon_tools::Message` and `trogon_tools::ContentBlock` explicitly. `trogon-tools` is
not a direct dependency of `trogon-acp`. In Rust 2021, transitive crate names are not
automatically in scope — `use trogon_tools::ContentBlock` fails with:
`error[E0433]: failed to resolve: use of undeclared crate or module trogon_tools`.

Note: `trogon_agent_core::agent_loop` re-exports the same types
(`pub use trogon_tools::{ContentBlock, Message, ...}`), so they are technically
reachable via that path — but using a re-exporting intermediary for the primary types
is confusing. The direct declaration is cleaner.

**Fix:** Add `trogon-tools` to `trogon-acp/Cargo.toml`:

```toml
trogon-tools = { path = "../trogon-tools" }
```

---

### Gap — `Bridge::new` block must be removed from `main.rs`

**File:** `trogon-acp/src/main.rs` — lines 170-179

**Problem:** The plan's `main.rs` section says "Replace: `TrogonAcpAgent::new(bridge, ...)`"
but does not say to remove the `Bridge::new(...)` block that sits above it. Without removing
it, two variables needed by `TrogonAcpAgent::new` are already consumed:

```rust
// lines 170-179 of current main.rs — MUST BE REMOVED:
let meter = opentelemetry::global::meter("trogon-acp");
let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(js.clone());
let bridge = Bridge::new(
    nats.clone(),
    js_client,       // ← js_client MOVED here (not cloned)
    trogon_std::time::SystemClock,
    &meter,
    config,          // ← config MOVED here (not cloned)
    notification_tx.clone(),
);
```

After the move into `Bridge::new`, `config` and `js_client` are no longer in scope.
Passing `config` and `js_client.clone()` to `TrogonAcpAgent::new` would fail to compile.

**Fix:** Delete the entire block (lines 170-179). `js_client` is redeclared at the same
point so it remains available for `TrogonAcpAgent::new`. `meter` is only used by
`Bridge::new` — once the block is removed, `let meter = ...` is dead code and must also
be deleted. `get_or_create_bridge` creates its own meter internally via
`opentelemetry::global::meter("trogon-acp")`, so no functionality is lost.

After deletion, `config` is still in scope from line 168 and can be passed directly to
`TrogonAcpAgent::new`. `js_client` is declared fresh (without consuming it in
`Bridge::new`) and passed directly — no `.clone()` needed.

---

### Gap — runner registration: all 4 runners already have both fields

**Files:** `trogon-acp-runner/src/main.rs`, `trogon-xai-runner/src/main.rs`,
`trogon-openrouter-runner/src/main.rs`, `trogon-codex-runner/src/main.rs`

**Problem:** The "Runner registration requirement" section said "If runners already
register... add `acp_prefix` and `models` to their existing metadata. If not, add the
registration call as part of this PR." This conditional wording implies the fields might
be missing and that runner changes might be needed.

**Finding:** All 4 runners already register with both `acp_prefix` and `models` at
startup. No changes to any runner crate are needed. The section has been updated above
to state this explicitly.

---

### Gap — `prompt_to`/`cancel_to` need a new impl block in `bridge.rs`

**File:** `acp-nats/src/agent/bridge.rs`

**Problem:** The Layer 1 section shows the method bodies for `prompt_to` and `cancel_to`
but does not specify which impl block they belong to. Placing them in any of the three
existing non-Agent impl blocks causes a compilation failure:

| Line | Bounds on `N` | Compiles? |
|---|---|---|
| 45 | (none) | ✗ — `N: SubscribeClient` not provable |
| 89 | `PublishClient + FlushClient + Clone + Send + 'static` | ✗ — missing `SubscribeClient`, JetStream |
| 141 | `RequestClient + PublishClient + FlushClient` | ✗ — missing `SubscribeClient` |

`prompt_to` calls `agent_client_protocol::Agent::prompt(self, args)` (UFCS) or equivalently
`prompt::handle(self, args, &serializer)` — both require `N: SubscribeClient`. The line 141
impl block, which is otherwise the closest match, lacks this bound.

**Fix:** Add a new inherent impl block with the same bounds as `impl Agent for Bridge<N,C,J>`:

```rust
impl<
    N: RequestClient + PublishClient + SubscribeClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
> Bridge<N, C, J>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    pub async fn prompt_to(
        &self,
        mut args: PromptRequest,
        target_session_id: &str,
    ) -> Result<PromptResponse> {
        args.session_id = SessionId::new(target_session_id);
        agent_client_protocol::Agent::prompt(self, args).await
    }

    pub async fn cancel_to(
        &self,
        mut args: CancelNotification,
        target_session_id: &str,
    ) -> Result<()> {
        args.session_id = SessionId::new(target_session_id);
        agent_client_protocol::Agent::cancel(self, args).await
    }
}
```

`cancel::handle` technically only needs `N: PublishClient + FlushClient`, so `cancel_to`
alone could go in a weaker impl block. But since both methods travel together, sharing one
impl block with the full bounds is cleaner and avoids splitting two related methods.

The `#[async_trait]` macro is **not** needed — `prompt_to` and `cancel_to` are regular
inherent async methods, not trait implementations. The `.await` on the UFCS call works
because `async_trait` transforms trait methods into `Pin<Box<dyn Future<...>>>`, which
is awaitable from any async context.

---

### Gap — `bridge.as_ref()` does not implement `Agent` — 4 call sites

**File:** `trogon-acp/src/agent.rs`

**Problem:** The plan passes `bridge.as_ref()` as the receiver in four UFCS `Agent::` calls:

| Method | Call |
|---|---|
| `set_session_model_impl` (same runner) | `agent_client_protocol::Agent::set_session_model(bridge.as_ref(), ...)` |
| `create_runner_session` | `agent_client_protocol::Agent::new_session(bridge.as_ref(), ...)` |
| `create_runner_session` | `agent_client_protocol::Agent::set_session_model(bridge.as_ref(), ...)` |
| `close_runner_session` | `agent_client_protocol::Agent::close_session(bridge.as_ref(), ...)` |

`bridge: Rc<Bridge<N,C,J>>`. `Rc<T>: AsRef<T>`, so `bridge.as_ref()` gives `&Bridge<N,C,J>`.

The `agent-client-protocol` crate provides exactly two blanket `Agent` impls:
```
impl<T: Agent> Agent for Rc<T>   // line 242
impl<T: Agent> Agent for Arc<T>  // line 309
```
There is no `impl<T: Agent> Agent for &T`. Therefore `&Bridge<N,C,J>` does not implement
`Agent`, and all four UFCS calls fail to compile:
```
error[E0277]: the trait bound `Bridge<N, C, J>: Agent` is not satisfied
  for `&Bridge<N, C, J>`
```

**Fix:** Call the trait methods directly on `bridge` (which is `Rc<Bridge<N,C,J>>` and
satisfies `Rc<Bridge>: Agent`). Direct method call syntax resolves to the trait impl
without needing UFCS. `Agent::new_session` / `set_session_model` / `close_session` all
take `&self`, so `bridge` is not moved:

```rust
// set_session_model_impl — same runner branch
let bridge = self.get_or_create_bridge(current_prefix)?;
let _ = bridge.set_session_model(
    SetSessionModelRequest::new(current_runner_sid.clone(), model_id.to_string()),
).await;

// create_runner_session
let bridge = self.get_or_create_bridge(prefix)?;
let resp = bridge.new_session(NewSessionRequest::new(cwd))
    .await.map_err(|e| anyhow::anyhow!("new_session on runner: {e}"))?;
let runner_sid = resp.session_id.to_string();
let _ = bridge.set_session_model(
    SetSessionModelRequest::new(runner_sid.clone(), model_id.to_string()),
).await;

// close_runner_session
let bridge = self.runner_bridges.borrow().get(prefix).map(Rc::clone);
if let Some(bridge) = bridge {
    let _ = bridge.close_session(CloseSessionRequest::new(runner_sid.to_string())).await;
}
```

The `Rc` blanket impl (`impl<T: Agent> Agent for Rc<T>`) delegates every method to
`T::method(&**self, args)`, so calling `bridge.new_session(args)` on an `Rc<Bridge>` is
equivalent to calling it on the inner `Bridge` — no observable difference.

---

### Gap — partial failure in runner migration leaves session stuck

**File:** `trogon-acp/src/agent.rs` — `set_session_model_impl`, different-runner branch

**Problem:** The original order of operations is:

```
1. export_history(old)   → OK
2. close_runner_session  → old session closed
3. id_remap.remove       → remap entry gone
4. create_runner_session → FAILS HERE (?)
5. import_history        → never reached
6. update_routing        → never reached
```

`active_sessions` is populated only by `update_routing` (insert). Since `update_routing`
is never reached, the entry `{acp_sid: (old_prefix, old_runner_sid)}` remains in
`active_sessions` after the early `?` return.

The old runner session is closed (step 2) but `active_sessions` still points at it.
On the next prompt, `prompt()` reads the stale entry and routes to the closed session —
the runner returns an error. The user calls `set_session_model` again:

- If same target model AND same prefix → hits "same runner" branch → tries `set_session_model`
  on closed session → may or may not succeed (runner-dependent).
- If different prefix → hits "different runner" branch → `export_history(old_runner_sid)`
  fails ("session not found") → `?` returns — **same stale entry remains, user stuck in loop**.

There is no self-healing path. The only recovery is `close_session` + new session.

**Root cause:** `active_sessions` is modified last (in `update_routing`) but the failure
happens in the middle of the sequence.

**Fix:** Clear routing as the very first step. Any subsequent failure leaves
`active_sessions` with no entry for `acp_sid` — the session is in "no runner" state.
The `None` branch of the next `set_session_model` call creates a fresh runner session and
imports history from KV (which was kept in sync by `post_prompt_sync_kv`):

```rust
Some((current_prefix, current_runner_sid)) => {
    // Clear routing first — any failure leaves "no runner" state (recoverable).
    self.active_sessions.borrow_mut().remove(acp_sid);
    self.id_remap.borrow_mut().remove(&current_runner_sid);
    let history = self.export_history(&current_prefix, &current_runner_sid).await?;
    self.close_runner_session(&current_prefix, &current_runner_sid).await;
    let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
    self.import_history(&target_prefix, &runner_sid, &history).await?;
    self.update_routing(acp_sid, &target_prefix, &runner_sid);
    self.sync_session_to_kv(acp_sid, &history).await?;
}
```

**Recovery path after fix:** any `?` return → `active_sessions` has no entry →
next prompt returns "no model selected" → user calls `set_session_model` → `None` branch →
`store.load(acp_sid)` reads KV history (last synced by `post_prompt_sync_kv`) → creates new
runner session + imports history → routing restored. At most one prompt's history lost
(the same bound as the "restart" known limitation).

---

### Gap — `new_session` plan snippet is a delta, not a replacement

**File:** `trogon-acp/src/agent.rs` — `new_session` in `impl Agent for TrogonAcpAgent`

**Problem:** The `new_session` code shown in the plan looks like a self-contained replacement.
A reader implementing it verbatim would drop all of the existing logic:

- Meta parsing from `args.meta` (systemPrompt, additionalRoots, disableBuiltInTools)
- `Self::spawn_stdio_bridges(&args.mcp_servers)` + `stdio_bridges.insert(...)`
- Full `SessionState` initialization (created_at, mode, mcp_servers, system_prompt, additional_roots, disable_builtin_tools)
- `self.publish_session_ready(&session_id).await`
- `self.send_available_commands_update(&sid, &state.mcp_servers).await`
- `Self::build_mode_state(...)` + `Self::build_config_options(...)` in the response

**Fix:** The plan snippet shows only the new additions. The actual implementation is a
**minimal delta** on the existing `new_session`:

1. Keep the entire existing function body unchanged up through `store.save`, `stdio_bridges.insert`,
   `publish_session_ready`, and `send_available_commands_update`.
2. After `store.save(...)`, splice in the new runner creation block:
   ```rust
   if let Ok(prefix) = self.resolve_prefix_for_model(&self.default_model).await {
       if let Ok(runner_sid) = self.create_runner_session(
           &prefix, &self.default_model, &session_id,
       ).await {
           self.update_routing(&session_id, &prefix, &runner_sid);
           if let Ok(mut s) = self.store.load(&session_id).await {
               s.model = Some(self.default_model.clone());
               let _ = self.store.save(&session_id, &s).await;
           }
       }
   }
   ```
3. Replace only `Self::build_model_state(&self.default_model)` with
   `self.build_model_state_from_registry(&self.default_model).await`.
4. All other lines — `modes`, `config_options`, the `NewSessionResponse` chain — stay as-is.

---

### Gap — `set_session_model` drops `ConfigOptionUpdate` notification

**File:** `trogon-acp/src/agent.rs` — `set_session_model` in `impl Agent for TrogonAcpAgent`

**Problem:** The current `set_session_model` (lines 1022-1026) sends a `ConfigOptionUpdate`
notification after updating the model so the IDE refreshes its config panel:

```rust
let config_options =
    Self::build_config_options(current_mode, &model, !is_running_as_root());
let config_notification = SessionNotification::new(
    args.session_id.clone(),
    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
);
let _ = self.notification_sender.send(config_notification).await;
```

The plan's replacement:

```rust
async fn set_session_model(&self, args: SetSessionModelRequest) -> Result<SetSessionModelResponse> {
    let acp_sid = args.session_id.to_string();
    let model_id = args.model_id.0.as_ref();
    self.set_session_model_impl(&acp_sid, model_id).await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    Ok(SetSessionModelResponse::new())
}
```

omits the notification entirely. The IDE model selector would stop refreshing when
`set_session_model` is called directly (e.g. from `new_session`'s post-creation call).

**Fix:** After `set_session_model_impl` returns, reload state and send the notification:

```rust
async fn set_session_model(&self, args: SetSessionModelRequest) -> Result<SetSessionModelResponse> {
    let acp_sid = args.session_id.to_string();
    let model_id = args.model_id.0.as_ref();
    self.set_session_model_impl(&acp_sid, model_id).await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    // Notify IDE to refresh config panel with the updated model.
    if let Ok(state) = self.store.load(&acp_sid).await {
        let current_mode = if state.mode.is_empty() { "default" } else { &state.mode };
        let config_options =
            Self::build_config_options(current_mode, model_id, !is_running_as_root());
        let config_notification = SessionNotification::new(
            args.session_id.clone(),
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options)),
        );
        let _ = self.notification_sender.send(config_notification).await;
    }
    Ok(SetSessionModelResponse::new())
}
```

Note: `set_session_config_option("model")` is NOT affected — it already sends a
`ConfigOptionUpdate` outside all branches (after reloading state), so the updated model
is included regardless of which branch ran.

---

---

### Gap — `TrogonAcpAgent` hardcodes `Registry<KvStore>` — unit tests break, no mock path

**File:** `trogon-acp/src/agent.rs` (struct + impls) and `trogon-acp/Cargo.toml`

**Problem:** The plan defines:
```rust
registry: trogon_registry::Registry<async_nats::jetstream::kv::Store>,
```

`async_nats::jetstream::kv::Store` requires a live NATS connection to create. Four existing
unit/integration test helpers (lines 2157, 2546, 4716, 4773) construct `TrogonAcpAgent::new`
with mock types (`AdvancedMockNatsClient`, `MockJs`, etc.) and no NATS server. With a
hardcoded `KvStore` type they cannot compile: there is no way to create a `KvStore` without
a real NATS connection.

**Fix:** Follow the existing `S = NatsSessionStore` / `Notif = NatsSessionNotifier` pattern
and add a sixth type parameter `R` with default `async_nats::jetstream::kv::Store`:

```rust
pub struct TrogonAcpAgent<
    N, C, J,
    S    = NatsSessionStore,
    Notif = NatsSessionNotifier,
    R    = async_nats::jetstream::kv::Store,   // default keeps main.rs unchanged
> {
    ...
    registry: trogon_registry::Registry<R>,
}
```

`trogon_registry::RegistryStore` is already publicly re-exported (`pub use store::RegistryStore`),
so the bound `R: trogon_registry::RegistryStore` compiles without additional imports.

All `impl` blocks on `TrogonAcpAgent` gain `R: trogon_registry::RegistryStore` in their
where clause. `RegistryStore: Send + Sync + Clone + 'static` — no conflict with existing bounds.

`TrogonAcpAgent::new` becomes:
```rust
pub fn new(..., registry: trogon_registry::Registry<R>) -> Self
where R: trogon_registry::RegistryStore
```

The `TestAgent` type alias in tests becomes:
```rust
type TestAgent = TrogonAcpAgent<
    AdvancedMockNatsClient, SystemClock, MockJs,
    MemorySessionStore, MockSessionNotifier,
    trogon_registry::MockRegistryStore,  // ← added
>;
```

`make_mock_agent` passes `trogon_registry::Registry::new(trogon_registry::MockRegistryStore::new())`.

`MockRegistryStore` is gated behind `#[cfg(any(test, feature = "test-support"))]`.
Add to `trogon-acp/Cargo.toml`:
```toml
[dev-dependencies]
trogon-registry = { path = "../trogon-registry", features = ["test-support"] }
```

Production `main.rs` uses `Registry<KvStore>` (the default) — no changes needed there.

---

### Gap — test helper updates incomplete: wrong type alias named, `Bridge::new` block not removed

**File:** `trogon-acp/src/agent.rs` — `make_mock_agent` (lines 2137-2167), `make_agent` (line 2518),
`make_agent_with_nats` (line 4709), and the `MockAgent` type alias (line 2121)

**Problem:** The existing gap says "The `TestAgent` type alias in tests becomes
`TrogonAcpAgent<..., MockRegistryStore>`" and "`make_mock_agent` passes
`Registry::new(MockRegistryStore::new())`". Both statements are incomplete or wrong:

**Wrong type alias.** `TestAgent` (line 1598) specifies only 3 explicit type params and is
exclusively used for static-method calls (`TestAgent::build_mode_state`,
`TestAgent::build_config_options`, `TestAgent::resolve_model`) — it is never instantiated.
With default `R = KvStore`, those calls compile fine without any change.

The alias that needs updating is `MockAgent` (line 2121), which is what `make_mock_agent`
returns. `MockAgent` specifies 5 explicit params and currently defaults to
`R = async_nats::jetstream::kv::Store`. After the PR, `make_mock_agent` passes
`Registry<MockRegistryStore>` — type mismatch → compile error:
```
error[E0308]: mismatched types
  expected `Registry<kv::Store>`, found `Registry<MockRegistryStore>`
```

Fix: update `MockAgent`, not `TestAgent`:
```rust
type MockAgent = TrogonAcpAgent<
    AdvancedMockNatsClient, SystemClock, MockJs,
    MemorySessionStore, MockSessionNotifier,
    trogon_registry::MockRegistryStore,  // ← added (R)
>;
```

**`Bridge::new` block not removed.** In `make_mock_agent`, lines 2149-2156:
```rust
let bridge = Bridge::new(
    nats,       // ← nats MOVED
    js_client,  // ← js_client MOVED (= js.clone())
    SystemClock,
    &opentelemetry::global::meter("unit-test"),
    config,     // ← config MOVED
    notif_tx.clone(),
);
```
`nats`, `js_client`, and `config` are moved into `Bridge::new`. After the PR,
`TrogonAcpAgent::new` needs those same three as factory fields. If the `Bridge::new` block
is not removed, all three are consumed before `TrogonAcpAgent::new` can use them →
compile error.

Fix: remove `let js_client = js.clone()` and the entire `let bridge = Bridge::new(...)` block;
pass the components directly:
```rust
fn make_mock_agent() -> (MockAgent, mpsc::Receiver<SessionNotification>) {
    let nats = AdvancedMockNatsClient::new();
    let js = MockJs::new();
    let store = MemorySessionStore::new();
    let notifier = MockSessionNotifier::new();
    let (notif_tx, notif_rx) = mpsc::channel(64);
    let gateway_config = Arc::new(RwLock::new(None));
    let config = Config::new(
        AcpPrefix::new("acp").unwrap(),
        NatsConfig { servers: vec!["unused".into()], auth: NatsAuth::None },
    );
    // Bridge::new block REMOVED — factory fields passed directly to TrogonAcpAgent::new
    let agent = TrogonAcpAgent::new(
        nats,
        js,
        SystemClock,
        config,              // base_config — was moved into Bridge::new, now passed here
        store,
        notifier,
        "acp",
        notif_tx,
        "claude-opus-4-6",
        gateway_config,
        trogon_registry::Registry::new(trogon_registry::MockRegistryStore::new()),
    );
    (agent, notif_rx)
}
```

**Integration test helpers** (`make_agent` at line 2518, `make_agent_with_nats` at line 4709)
have the same `Bridge::new` move problem. Additionally, `RealAgent` defaults to
`R = KvStore`, so those helpers need a real `Registry<KvStore>` — provision it from
the existing `js` handle before constructing `TrogonAcpAgent::new`:
```rust
let reg_store = trogon_registry::provision(js).await.unwrap();
let registry = trogon_registry::Registry::new(reg_store);
// Then pass registry as last arg to TrogonAcpAgent::new
```
`trogon_registry::provision` already used in `main.rs`; no new import needed in the
test module beyond adding `trogon-registry` to `[dev-dependencies]`.

---

### Gap — `resolve_model` becomes dead code; `/model <alias>` breaks after this PR

**File:** `trogon-acp/src/agent.rs` — `resolve_model` (line 453) and its two call sites

**Problem:** `resolve_model` normalizes human aliases to canonical model IDs
(e.g. `"claude opus 4"` → `"claude-opus-4-6"`, `"opus"` → `"claude-opus-4-6"`). It is
called at exactly two sites:

- **Line 957** (`set_session_config_option` model branch) — replaced by `set_session_model_impl`
  in the `set_session_config_option` gap fix
- **Line 996** (`set_session_model`) — replaced by the new `set_session_model` body

After both replacements, `resolve_model` has no production callers. More importantly,
`set_session_model_impl` calls `resolve_prefix_for_model(model_id)` which does an exact
registry lookup. Aliases like `"claude opus 4"` are not registered, so the lookup returns
`Err("no runner registered for model claude opus 4")`.

The Zed `/model` slash command sends the user's text directly to
`set_session_config_option("model", <text>)`. A user typing `/model claude opus 4` gets an
error after this PR.

**Fix for v1 — alias breakage:** document as Known Limitation. The IDE model picker sends
canonical IDs from the registry, so the normal workflow is unaffected. Only free-text alias
input is broken.

**Follow-up fix for aliases:** before calling `resolve_prefix_for_model`, apply alias
normalisation. Since alias resolution is now cross-runner (grok models, codex models, etc.),
the right long-term fix is for runners to include aliases in their `"models"` metadata list.
That way the registry itself resolves `"grok 3"` → finds a runner → routing works.

**Fix for dead code warning:** `resolve_model` is called only from five test functions
inside the `#[cfg(test)]` module (lines 1780–1827). In non-test builds those callers don't
exist, so `cargo build` emits `warning: method resolve_model is never used`. There is no
`#![deny(warnings)]` in the crate so it is a warning, not an error. However, keeping tests
that exercise removed production code is misleading.

Delete `resolve_model` (line 453) and its five test functions:

```
fn resolve_model_exact_id_match()       // ~line 1780
fn resolve_model_case_insensitive_name() // ~line 1796
fn resolve_model_substring_match()      // ~line 1808
fn resolve_model_tokenized_match()      // ~line 1821
fn resolve_model_empty_returns_none()   // ~line 1826
```

`TestAgent` (line 1598) is used only for static calls to `build_mode_state` and
`build_config_options` — it does not need changes.

---

### Gap — `fork_session` first prompt fails if IDE skips `set_session_model`

**File:** `trogon-acp/src/agent.rs` — `fork_session` in `impl Agent for TrogonAcpAgent`

**Problem:** `fork_session` copies the parent's model into `new_state.model` and includes it
in `ForkSessionResponse` as `SessionModelState.current_model`. The IDE shows that model
pre-selected in the fork panel. No entry is added to `active_sessions` for the new fork.

If the user does not change the model (never triggers `set_session_model`), `active_sessions`
has no entry for the fork's `new_id`. The first `prompt()` returns
`"no model selected for session {id}"`.

This is the same root cause as the "restart" Known Limitation, but it applies to every
new fork in the same process — not just after restarts.

**Fix option A (v1 — acceptable):** Document as Known Limitation alongside the restart
case. Same workaround: touch the model picker to trigger `set_session_model`. The lazy
re-initialization follow-up PR already noted in Known Limitations would fix both.

**Fix option B (complete):** In `fork_session`, after `store.save`, best-effort create
a runner session and import messages — mirroring what `new_session` does:

```rust
// After store.save(&new_id, &new_state):
if let Some(ref model) = new_state.model {
    if let Ok(prefix) = self.resolve_prefix_for_model(model).await {
        if let Ok(runner_sid) = self.create_runner_session(&prefix, model, &new_id).await {
            if !new_state.messages.is_empty() {
                let portable = portable_from_messages(&new_state.messages);
                let _ = self.import_history(&prefix, &runner_sid, &portable).await;
            }
            self.update_routing(&new_id, &prefix, &runner_sid);
        }
    }
}
```

This requires `J: Clone, C: Clone` on `fork_session` — already added to the `impl Agent`
where clause in this PR, so no additional bound change needed.

---

### Gap — `id_remap` clone must happen before `AgentSideConnection::new`, not before the select loop

**File:** `trogon-acp/src/main.rs`

**Problem:** The plan says "id_remap is cloned as `Rc::clone(&acp_agent.id_remap)` before the
select loop." The select loop is inside the `spawn_local` closure at ~line 214. But
`AgentSideConnection::new(acp_agent, stdout, stdin, ...)` at ~line 205 **moves** `acp_agent`.
After that call, `acp_agent` no longer exists.

Placing `Rc::clone(&acp_agent.id_remap)` between line 205 and 214 fails to compile:
```
error[E0382]: use of moved value: `acp_agent`
```

**Fix:** Clone `id_remap` immediately after `TrogonAcpAgent::new` and before
`AgentSideConnection::new`:

```rust
let acp_agent = agent::TrogonAcpAgent::new(
    nats.clone(), js_client, trogon_std::time::SystemClock, config,
    store.clone(), NatsSessionNotifier::new(nats.clone()),
    acp_prefix, notification_tx.clone(), model.clone(),
    gateway_config, registry,
);
// Clone BEFORE acp_agent is moved into AgentSideConnection::new below.
let id_remap = Rc::clone(&acp_agent.id_remap);

let (conn, io_task) = AgentSideConnection::new(acp_agent, stdout, stdin, |fut| {
    tokio::task::spawn_local(fut);
});

tokio::task::spawn_local(async move {
    loop {
        tokio::select! {
            maybe_notification = notification_rx.recv() => {
                let remapped = id_remap.borrow().get(...).cloned();
                // ...
```

`id_remap` (an `Rc`) is `!Send` but safe here because everything stays on the
`LocalSet` single thread.

---

### Gap — helper methods must return `anyhow::Result<...>` explicitly, not bare `Result<...>`

**File:** `trogon-acp/src/agent.rs` — helper impl block

**Problem:** `agent.rs` has `use agent_client_protocol::{..., Result, ...}` at the top
(line 19). Bare `Result<T>` in that file means `agent_client_protocol::Result<T>`. The
plan writes `Result<String>`, `Result<Vec<PortableMessage>>`, `Result<()>` on all helpers
without qualification. A reader implementing this would use `agent_client_protocol::Result`.

The helpers use `anyhow!()` macros. `From<anyhow::Error> for agent_client_protocol::Error`
exists (verified), so this compiles. But the conversion path is:

```
anyhow!("no runner registered for model grok-3")
  → ? → From<anyhow::Error>
  → Error { message: "Internal error", data: Some("no runner registered for model grok-3") }
```

Display of that error: `"Internal error: \"no runner registered for model grok-3\""` (with
escaped quotes). Then `set_session_model` wraps it again:

```rust
.map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))
```

Result: `Error { message: "Internal error: \"no runner registered for model grok-3\"" }`.

The IDE sees the doubled prefix and escaped quotes in the error message.

**Fix:** All private helper methods must use `anyhow::Result<...>` explicitly:

```rust
async fn set_session_model_impl(&self, acp_sid: &str, model_id: &str) -> anyhow::Result<()>
async fn resolve_prefix_for_model(&self, model_id: &str) -> anyhow::Result<String>
async fn export_history(&self, prefix: &str, runner_sid: &str) -> anyhow::Result<Vec<PortableMessage>>
async fn import_history(&self, prefix: &str, runner_sid: &str, history: &[PortableMessage]) -> anyhow::Result<()>
async fn create_runner_session(&self, prefix: &str, model_id: &str, acp_sid: &str) -> anyhow::Result<String>
async fn sync_session_to_kv(&self, acp_sid: &str, history: &[PortableMessage]) -> anyhow::Result<()>
```

The `impl Agent` trait methods (`set_session_model`, `set_session_config_option`, `prompt`,
`cancel`) return `agent_client_protocol::Result<...>` — they are the conversion boundary
where `.map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))` produces
clean error messages from the `anyhow::Error`'s `Display`.

---

### Gap — orphaned runner session if `import_history` fails in None branch

**File:** `trogon-acp/src/agent.rs` — `set_session_model_impl`, None branch

**Problem:** The None branch creates a runner session before importing history:

```rust
let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
let state = self.store.load(acp_sid).await?;
if !state.messages.is_empty() {
    let portable = portable_from_messages(&state.messages);
    self.import_history(&target_prefix, &runner_sid, &portable).await?;  // ← failure here
}
self.update_routing(acp_sid, &target_prefix, &runner_sid);
```

If `import_history` fails, the `?` returns before `update_routing`. The `runner_sid` is never
inserted into `active_sessions` or `id_remap` — it is unreachable from any subsequent call.
The runner session leaks until TTL expiry.

Note: the `store.load` at line 2 is not a realistic orphan risk — `create_runner_session`
already called `store.load` internally to obtain `cwd`. If the KV entry doesn't exist,
`create_runner_session` itself fails first (before any runner session is created).

**Fix:** Explicitly close the runner session if `import_history` fails. The bridge for
`target_prefix` already exists in `runner_bridges` (created by `create_runner_session` via
`get_or_create_bridge`), so `close_runner_session` can find it. `close_runner_session`
ignores errors internally — if the close itself fails, TTL handles it.

```rust
None => {
    let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
    let state = self.store.load(acp_sid).await?;
    if !state.messages.is_empty() {
        let portable = portable_from_messages(&state.messages);
        if let Err(e) = self.import_history(&target_prefix, &runner_sid, &portable).await {
            self.close_runner_session(&target_prefix, &runner_sid).await;
            return Err(e);
        }
    }
    self.update_routing(acp_sid, &target_prefix, &runner_sid);
}
```

`set_session_model_impl` returns `anyhow::Result<()>` (see gap above) — `return Err(e)` where
`e: anyhow::Error` compiles correctly. `close_runner_session` needs no Clone bounds (reads
an already-stored `Rc<Bridge>` from `runner_bridges`).

---

### Gap — orphaned runner session if `import_history` fails in different-runner branch

**File:** `trogon-acp/src/agent.rs` — `set_session_model_impl`, different-runner branch

**Problem:** Same pattern as the None branch gap. After `create_runner_session` succeeds,
`import_history` can fail:

```rust
let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
self.import_history(&target_prefix, &runner_sid, &history).await?;  // ← failure here
self.update_routing(acp_sid, &target_prefix, &runner_sid);
```

The `?` returns before `update_routing`. `runner_sid` is never entered into `active_sessions`
or `id_remap` and leaks until TTL. The session remains in "no runner" state (routing was
cleared at the top of the branch) — recoverable via retry — but the runner session is wasted.

The bridge for `target_prefix` is guaranteed to exist in `runner_bridges` when
`close_runner_session` is called: `create_runner_session` calls `get_or_create_bridge(prefix)`
(line 460) which inserts it before returning.

**Fix:** Same cleanup pattern as the None branch:

```rust
let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
if let Err(e) = self.import_history(&target_prefix, &runner_sid, &history).await {
    self.close_runner_session(&target_prefix, &runner_sid).await;
    return Err(e);
}
self.update_routing(acp_sid, &target_prefix, &runner_sid);
```

`return Err(e)` correctly skips the model update at the bottom of `set_session_model_impl` —
saving `model_id` to KV when no runner is assigned would be wrong.

---

### Gap — `sync_session_to_kv` with `?` after `update_routing` returns false error to IDE

**File:** `trogon-acp/src/agent.rs` — `set_session_model_impl`, different-runner branch

**Problem:** `sync_session_to_kv` is called with `?` after `update_routing` has already
succeeded:

```rust
self.update_routing(acp_sid, &target_prefix, &runner_sid);
self.sync_session_to_kv(acp_sid, &history).await?;   // ← propagates error
```

At this point the migration is complete: routing is correct and the new runner has the
imported history. If `sync_session_to_kv` fails (e.g. transient NATS KV write error),
`set_session_model_impl` returns `Err` → `set_session_model` maps it to
`Error::new(InternalError, ...)` and returns it to the IDE. The IDE receives an error and
likely rolls back its model selection UI — but internally the session is fully usable.

`post_prompt_sync_kv` (the other call site for `sync_session_to_kv`) already treats the
same operation as best-effort with `let _ = ...`. The inconsistency is unjustified.

**Note on interaction with model update:** `sync_session_to_kv` only mutates
`state.messages`, not `state.model`. The model update block at the bottom of
`set_session_model_impl` does a fresh `store.load` and sets `state.model`. If
`sync_session_to_kv` runs successfully, the model update picks up the updated messages and
adds the correct model. If `sync_session_to_kv` is skipped (best-effort, no error), the KV
messages are stale but `post_prompt_sync_kv` syncs them after the next prompt.

**Fix:** Make `sync_session_to_kv` best-effort after `update_routing`, consistent with
`post_prompt_sync_kv`:

```rust
self.update_routing(acp_sid, &target_prefix, &runner_sid);
let _ = self.sync_session_to_kv(acp_sid, &history).await;
```

The full corrected different-runner branch after both fixes:

```rust
Some((current_prefix, current_runner_sid)) => {
    self.active_sessions.borrow_mut().remove(acp_sid);
    self.id_remap.borrow_mut().remove(&current_runner_sid);
    let history = self.export_history(&current_prefix, &current_runner_sid).await?;
    self.close_runner_session(&current_prefix, &current_runner_sid).await;
    let runner_sid = self.create_runner_session(&target_prefix, model_id, acp_sid).await?;
    if let Err(e) = self.import_history(&target_prefix, &runner_sid, &history).await {
        self.close_runner_session(&target_prefix, &runner_sid).await;
        return Err(e);
    }
    self.update_routing(acp_sid, &target_prefix, &runner_sid);
    let _ = self.sync_session_to_kv(acp_sid, &history).await;
}
```

---

## Known limitations after this PR

### `new_session` latency increased by runner setup round-trips

`new_session` now performs up to 3 NATS round-trips before returning:
`list_all()` (registry lookup) + `new_session()` (runner) + `set_session_model()` (runner).
On a local NATS cluster this adds ~5–50 ms. On a cold Docker runner or under NATS load
the delay may reach several hundred milliseconds.

The `if let Ok(...)` wrapping prevents a hung `new_session` if runners are entirely
unreachable (failure is silently swallowed and the session is created without a runner).
But a slow runner that eventually responds still delays the `new_session` return for the
full round-trip duration.

**Workaround (follow-up PR):** move runner session creation out of `new_session` entirely.
Create the runner session lazily on the first `prompt()` call — the same lazy
re-initialization path already planned for the restart Known Limitation. This restores
`new_session` to sub-millisecond latency and keeps the "no runner" graceful-degradation
path as the only code path for session creation.

---

### `active_sessions` lost on restart

`active_sessions` and `id_remap` are in-memory (`Rc<RefCell<HashMap<...>>>`). If `trogon-acp`
restarts, all routing state is lost. Sessions that had a runner assigned before the restart will
get "no model selected for session {id}" on the next prompt, even though their model is recorded
in KV.

**Workaround:** re-select the model in the IDE picker after a restart. This triggers
`set_session_model` which re-creates the runner session and re-imports history from KV.

A follow-up PR could add lazy re-initialization in `prompt()`: if `active_sessions` has no
entry for the session but KV has a model recorded, call `set_session_model_impl` automatically
before routing.

### In-process `TrogonAgent` becomes unreachable — IDE permission dialogs broken for external runners

`main.rs` (lines 144-158) embeds a `TrogonAgent` that listens at `acp_prefix` (e.g. `"acp"`).
In v1, `self.bridge` pointed directly at it. After this PR the bridge pool only creates
bridges for registry-registered prefixes. The in-process `TrogonAgent` **never registers
in the registry** — the bridge pool never points at it.

**Consequence — routing:** For Docker deployments, `trogon-acp-runner` is registered at
`"acp.claude"` and handles Claude models correctly. For local development without Docker,
`resolve_prefix_for_model` returns an error, `new_session` creates the session without a
runner (graceful fallback), and the first prompt fails with "no model selected". Running
`trogon-acp-runner` separately before starting `trogon-acp` restores full functionality.

**Consequence — permission dialogs:** The `perm_tx` channel exists specifically so the
in-process `TrogonAgent` can ask the IDE to show tool-approval dialogs
(`handle_permission_request` → `conn.request_permission()`). External runners (Docker)
have no equivalent channel back to `trogon-acp`. After this PR, IDE permission dialogs
work only when using the in-process runner; all external runners process tools without
user confirmation in v1.

**Long-term fix (follow-up PR):** Move permission requests from the in-process `perm_tx`
channel to a NATS request/reply pattern:

1. Runner publishes a permission request to a dedicated NATS subject
   (e.g. `{runner_prefix}.session.{session_id}.permission`) with a NATS `reply-to`
   subject.
2. `trogon-acp` subscribes to `{runner_prefix}.session.*.permission`, calls
   `conn.request_permission()` on the IDE connection, and publishes the allow/deny
   response to the reply subject.
3. The runner receives the response and continues or cancels the tool.

This pattern is symmetric with how prompt/cancel already work (trogon-acp publishes,
runner responds). Implementation touches `acp-nats-agent` (runner side, local crate) and
`trogon-acp/src/main.rs` (subscriber side). It does not require modifying
`agent-client-protocol` (external fixed crate). Once implemented the in-process
`TrogonAgent` in `main.rs` can be removed, as all runners — including Claude — will
support IDE permission dialogs via NATS.

### MCP tools not transferred when using Docker `acp-runner`

The in-process `TrogonAgent` shares `NatsSessionStore` with `trogon-acp`. When
`trogon-acp`'s `new_session` stores `mcp_servers` (the HTTP URLs of the started stdio
bridges) in KV, the in-process runner reads them on the next prompt and calls
`build_session_mcp`. This works because both sides share the same store.

After this PR, `create_runner_session` calls `bridge.new_session(NewSessionRequest::new(cwd))`
on Docker runners. `trogon-acp-runner`'s `new_session` uses `..Default::default()` for
`mcp_servers` — it ignores what arrives in the request and creates a bare session. The
Docker runner's session always has `mcp_servers: []`, so `build_session_mcp` is never
called and MCP tools are unavailable.

`xai-runner`, `openrouter-runner`, and `codex-runner` have no MCP support — unaffected.
This limitation applies only to `acp-runner` Docker.

**Workaround:** use the in-process runner (local development, not Docker). MCP tools
continue to work there because the shared store path is unaffected by this PR.

**Long-term fix (follow-up PR, two steps):**

1. `trogon-acp-runner/src/agent.rs` — read `req.mcp_servers` in `new_session` and store
   them, instead of using `..Default::default()`:
   ```rust
   let state = SessionState {
       cwd: req.cwd.to_string_lossy().to_string(),
       mcp_servers: convert_mcp_servers(&req.mcp_servers), // ← new
       mode, system_prompt, ..Default::default()
   };
   ```

2. `trogon-acp/src/agent.rs` — `create_runner_session` loads `state.mcp_servers` from the
   `trogon-acp` store and passes the already-resolved HTTP URLs in the request:
   ```rust
   let state = self.store.load(acp_sid).await?;
   let resp = bridge.new_session(
       NewSessionRequest::new(state.cwd.clone())
           .mcp_servers(state.mcp_servers_as_acp())
   ).await?;
   ```

   The HTTP URLs in `state.mcp_servers` are already resolved (the stdio bridges are
   running in `trogon-acp`). The Docker runner can call them over the network.

This fix is excluded from the current PR because it requires modifying a runner crate
(`trogon-acp-runner`). The plan's constraint "no changes to any runner crate" is preserved
here; the runner change ships as a separate PR.

### `codex-runner` TROGON.md injection

`codex-runner` TROGON.md injection bug (Gap PR 4 from `programming-imple.md`) remains a
separate fix — not in scope here.
