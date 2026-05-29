# Programming A — Work plan
# Session migration + CLI bridge

> All work is additive. NATS remains as backend without changes.
> The names of all tools and the ToolContext contract are fixed in the "Tool contract" section at the end of this document.

---

## Wave summary

| Wave | PRs | Status |
|---|---|---|
| — | PR 1 + PR 2 + PR 4 + PR 11 + PR 12 | ✅ Implemented |
| Wave 1 | PR 7 — Session export/import | New work |
| Wave 2 | PR 8 — CrossRunnerSwitcher | New work — after PR 7 |

---

## Implemented

### PR 1 — Core tools in `trogon-agent-core` ✅

**`crates/trogon-agent-core/Cargo.toml` — new dependencies**
- `globset = "0.4"`
- `ignore = "0.4"`
- `walkdir = "2"`
- `html2text = "0.12"`

**`crates/trogon-agent-core/src/tools/mod.rs`**
- Add field `cwd: String` to `ToolContext`
- Complete `dispatch_tool()` with real routing:
  - `"read_file"` → `fs::read_file`
  - `"write_file"` → `fs::write_file`
  - `"list_dir"` → `fs::list_dir`
  - `"glob"` → `fs::glob_files`
  - `"str_replace"` → `editor::str_replace`
  - `"git_status"` → `git::status`
  - `"git_diff"` → `git::diff`
  - `"git_log"` → `git::log`
  - `"fetch_url"` → `web::fetch_url`
  - `"notebook_edit"` → `fs::notebook_edit`

**`crates/trogon-agent-core/src/tools/fs.rs` — new file**

`read_file`
- Parameters: `path: String`, `offset?: u32`, `limit?: u32`
- Resolves path relative to `ctx.cwd`
- Rejects paths outside `cwd` (path traversal protection)
- Returns content with line numbers in `cat -n` style
- If `offset`/`limit` present, returns only that line range
- If file does not exist, returns readable error

`write_file`
- Parameters: `path: String`, `content: String`
- Creates intermediate directories with `tokio::fs::create_dir_all`
- Atomic write: first writes to `.tmp`, then `rename`
- Returns `"OK"` or error message

`list_dir`
- Parameters: `path?: String` (default `"."`)
- Uses the `ignore` crate to respect `.gitignore` automatically
- Returns directory tree as text in `tree` style
- Limits to 500 entries

`glob`
- Parameters: `pattern: String`, `path?: String`
- Uses `globset` for matching (`src/**/*.rs`, `**/*.test.ts`)
- Filters entries ignored by `.gitignore` via `ignore::Walk`
- Returns list of paths relative to `cwd`

`notebook_edit`
- Parameters: `path: String`, `cell_index: u32`, `content: String`, `cell_type?: String`
- Reads `.ipynb` (pure JSON), edits the cell by index, writes the file back

**`crates/trogon-agent-core/src/tools/editor.rs` — new file**

`str_replace`
- Parameters: `path: String`, `old_str: String`, `new_str: String`
- Reads the full file
- Verifies that `old_str` appears **exactly once** — if there are 0 or >1 occurrences, returns error with exact count
- Replaces and writes the result
- Returns diff of changed lines with ±3 lines of context

**`crates/trogon-agent-core/src/tools/git.rs` — new file**

`git_status`, `git_diff`, `git_log`
- Execute `git status --short`, `git diff`, `git log --oneline -20`
- Use `tokio::process::Command` with `current_dir(&ctx.cwd)`
- Output limited to ~4KB; if exceeded, truncated with notice at the end

**`crates/trogon-agent-core/src/tools/web.rs` — new file**

`fetch_url`
- Parameters: `url: String`, `raw?: bool`
- Uses `ctx.http_client` (reuses existing client)
- If `raw: false` (default): converts HTML to plain text with `html2text`
- Truncates to 8KB with notice if response is larger

---

### PR 2 — Bash with real state ✅

**`crates/trogon-acp-runner/src/wasm_bash_tool.rs`**
- Add field `sandbox_dir: PathBuf` to the `WasmRuntimeBashTool` struct
- Pass `sandbox_dir` as `cwd` in `CreateTerminalRequest`
- `sandbox_dir` is initialized from `session.cwd` in `new_session`

**`crates/trogon-acp-runner/src/session_store.rs`**
- Add `terminal_id: Option<String>` to `SessionState` (stored in NATS KV)
- First bash call: create terminal, save `terminal_id` in `SessionState`
- Subsequent calls: reuse the same terminal with `WriteToTerminalRequest`
- Demarcation protocol: `<command>; echo "__EXIT_$?__"` — read stream until `__EXIT_N__` is found, extract exit code
- Configurable timeout, default 30s; on expiry, returns error with partial output

---

### PR 4 — Context and memory ✅

**`crates/trogon-acp-runner/src/trogon_md.rs` — new file**
- Search for `TROGON.md` from `cwd` toward the root of the filesystem
- Load `~/.config/trogon/TROGON.md` as global user configuration
- Concatenate in order: global → repo root → current directory
- Inject into `system_prompt` when creating or loading a session

**`crates/trogon-acp-runner/src/agent.rs`**
- Change unconditional compact to 85% threshold:
```rust
let token_estimate = estimate_token_count(&session.messages); // heuristic len_bytes / 4
if token_estimate > TOKEN_BUDGET * 85 / 100 {
    compact_messages(&mut session).await?;
}
// TOKEN_BUDGET configurable in SessionState, default 200_000
```

---

### PR 11 — MCP stdio as CLI HTTP proxy ✅

**`crates/trogon-cli/src/stdio_mcp_bridge.rs` — new file**
- Launch MCP stdio process as child (e.g. `npx @modelcontextprotocol/server-filesystem ./`)
- Start axum server on a random local port
- Proxy JSON-RPC ↔ stdin/stdout of the child process
- Register in the session as `StoredMcpServer::Http { url: "http://127.0.0.1:<port>" }`
- On CLI exit: kill child process and release port
- The NATS backend changes no lines

---

### PR 12 — Additional tools and specialized agents ✅

**`crates/trogon-agent-core/src/tools/mod.rs`**

`todo_write`
- Parameters: `id: String`, `content: String`, `status: String` (pending / in_progress / completed)
- Stored in NATS KV bucket per session

`todo_read`
- Returns list of active tasks for the current session

**`crates/trogon-agent/src/tools/mod.rs`**

`spawn_agent`
- Create `spawn_agent` ToolDef
- Dispatch: NATS request-reply to `{prefix}.agent.spawn`
- The registry resolves the correct agent and returns the result to the current model turn

**`trogon-registry`**
- Register `Explore` agent: only reads files and answers questions, never edits or executes
- Register `Plan` agent: only plans, does not execute destructive tools

---

## Day 0 — scaffolding (shared with Dev B, half-day)

Before starting Wave 1, create in the shared base branch `feat/programming-assistant`.
Either developer can do this.

**`trogon-runner-tools/src/portable_session.rs` — new file**
```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableMessage {
    pub role: String,  // "user" | "assistant"
    pub text: String,
}
```

**`trogon-runner-tools/src/lib.rs`** — add:
```rust
pub mod portable_session;
```

**`trogon-codex-runner/src/agent.rs`** — add fields to `CodexSession`:
```rust
history: Vec<trogon_runner_tools::portable_session::PortableMessage>,
pending_history: Option<Vec<trogon_runner_tools::portable_session::PortableMessage>>,
```

**All runners** — add empty arms to `ext_method`:
```rust
"session/export" => { todo!("PR 7") }
"session/import" => { todo!("PR 7") }
```

For openrouter-runner, add the full `ext_method` scaffold if missing:
```rust
async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse, Error> {
    match args.method.as_str() {
        "session/list_children" => { /* stub */ Ok(ExtResponse::new("[]".into())) }
        "session/export"        => { todo!("PR 7") }
        "session/import"        => { todo!("PR 7") }
        _ => Err(Error::new(ErrorCode::MethodNotFound.into(), args.method)),
    }
}
```

**`trogon-registry/src/registry.rs`** — add stub:
```rust
pub async fn find_by_model(&self, _model_id: &str) -> Result<Option<AgentCapability>, String> {
    todo!("PR 6 — Dev B")
}
```

After Day 0 both developers compile against the stubs and start Wave 1 immediately.

---

## Wave 1

### PR 7 — Cross-runner session export/import

Depends on: Day 0 scaffolding (PortableMessage + empty ext_method arms in all runners)

This PR enables mid-session model switching. Each runner gains the ability to export
its conversation history as portable messages and import history from another runner.

#### `session/export` in each runner

**xai-runner** (`trogon-xai-runner/src/agent.rs`):
```rust
"session/export" => {
    let params: serde_json::Value =
        serde_json::from_str(args.params.get()).unwrap_or_default();
    let session_id = params["sessionId"].as_str()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
    let sessions = self.sessions.lock().await;
    let s = sessions.get(session_id)
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "session not found"))?;
    let portable: Vec<PortableMessage> = s.history.iter()
        .map(|m| PortableMessage { role: m.role.clone(), text: m.content_str().to_string() })
        .collect();
    let raw = serde_json::to_string(&portable)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    Ok(ExtResponse::new(serde_json::value::RawValue::from_string(raw)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?.into()))
}
```

**openrouter-runner** — identical pattern.

**acp-runner** (`trogon-acp-runner/src/agent.rs`) — reads from KV:
```rust
"session/export" => {
    let params: serde_json::Value =
        serde_json::from_str(args.params.get()).unwrap_or_default();
    let session_id = params["sessionId"].as_str()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
    let state = self.store.load(session_id).await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    let portable: Vec<PortableMessage> = state.messages.iter()
        .map(|m| {
            let text = m.content.iter().filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
                _ => None,
            }).collect::<Vec<_>>().join("\n");
            PortableMessage { role: m.role.clone(), text }
        })
        .collect();
    let raw = serde_json::to_string(&portable)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    Ok(ExtResponse::new(serde_json::value::RawValue::from_string(raw)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?.into()))
}
```

Note: `ContentBlock::ToolUse` and `ContentBlock::Thinking` are intentionally dropped.
Code state is on disk — the new model re-reads files it needs.

**codex-runner** — reads from the accumulated `history` field (scaffolded on Day 0):
```rust
"session/export" => {
    let params: serde_json::Value =
        serde_json::from_str(args.params.get()).unwrap_or_default();
    let session_id = params["sessionId"].as_str()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
    let sessions = self.sessions.lock().await;
    let s = sessions.get(session_id)
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "session not found"))?;
    let raw = serde_json::to_string(&s.history)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    Ok(ExtResponse::new(serde_json::value::RawValue::from_string(raw)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?.into()))
}
```

#### History accumulation in codex-runner

Wire the `history` and `pending_history` fields (scaffolded on Day 0) in the prompt
handler — use `get_mut` to update `first_turn` and capture `pending_history` atomically:

```rust
let (thread_id, model, cwd, first_turn, pending_history) = {
    let mut sessions = self.sessions.lock().await;
    let s = sessions.get_mut(&session_id)
        .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
    let ft = s.first_turn;
    let ph = s.pending_history.take();
    s.first_turn = false;
    s.history.push(PortableMessage { role: "user".into(), text: user_input.clone() });
    (s.thread_id.clone(), s.model.clone(), s.cwd.clone(), ft, ph)
};

let user_input = if let Some(prior) = pending_history {
    let formatted = prior.iter()
        .map(|m| format!("[{}]: {}", m.role, m.text))
        .collect::<Vec<_>>()
        .join("\n");
    format!("Prior conversation:\n{formatted}\n\n---\n\n{user_input}")
} else if first_turn {
    match trogon_runner_tools::trogon_md::load_trogon_md(&cwd).await {
        Some(md) => format!("{md}\n\n{user_input}"),
        None => user_input,
    }
} else {
    user_input
};
```

In the event loop, accumulate `TextDelta` and flush on `TurnCompleted`:
```rust
let mut assistant_text = String::new();
// ...
CodexEvent::TextDelta { text } => {
    assistant_text.push_str(&text);
}
CodexEvent::TurnCompleted => {
    let mut sessions = self.sessions.lock().await;
    if let Some(s) = sessions.get_mut(&session_id) {
        s.history.push(PortableMessage {
            role: "assistant".into(),
            text: std::mem::take(&mut assistant_text),
        });
    }
    break StopReason::EndTurn;
}
```

#### `session/import` in each runner

**xai-runner**:
```rust
"session/import" => {
    let params: serde_json::Value =
        serde_json::from_str(args.params.get()).unwrap_or_default();
    let session_id = params["sessionId"].as_str()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
    let messages: Vec<PortableMessage> =
        serde_json::from_value(params["messages"].clone())
            .map_err(|e| Error::new(ErrorCode::InvalidParams.into(), e.to_string()))?;
    let mut sessions = self.sessions.lock().await;
    let s = sessions.get_mut(session_id)
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "session not found"))?;
    s.history = messages.into_iter()
        .map(|m| Message { role: m.role, content: Some(m.text), prompt_tokens: None, completion_tokens: None })
        .collect();
    let raw = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    Ok(ExtResponse::new(raw.into()))
}
```

**openrouter-runner** — identical pattern.

**acp-runner** — saves to KV:
```rust
"session/import" => {
    let params: serde_json::Value =
        serde_json::from_str(args.params.get()).unwrap_or_default();
    let session_id = params["sessionId"].as_str()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
    let messages: Vec<PortableMessage> =
        serde_json::from_value(params["messages"].clone())
            .map_err(|e| Error::new(ErrorCode::InvalidParams.into(), e.to_string()))?;
    let mut state = self.store.load(session_id).await.unwrap_or_default();
    state.messages = messages.into_iter()
        .map(|m| trogon_tools::Message {
            role: m.role,
            content: vec![ContentBlock::Text { text: m.text }],
        })
        .collect();
    state.updated_at = now_iso8601();
    self.store.save(session_id, &state).await
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    let raw = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    Ok(ExtResponse::new(raw.into()))
}
```

**codex-runner** — sets `pending_history` for first-turn prepend:
```rust
"session/import" => {
    let params: serde_json::Value =
        serde_json::from_str(args.params.get()).unwrap_or_default();
    let session_id = params["sessionId"].as_str()
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
    let messages: Vec<PortableMessage> =
        serde_json::from_value(params["messages"].clone())
            .map_err(|e| Error::new(ErrorCode::InvalidParams.into(), e.to_string()))?;
    let mut sessions = self.sessions.lock().await;
    let s = sessions.get_mut(session_id)
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "session not found"))?;
    s.pending_history = Some(messages);
    s.first_turn = true;  // reset so prepend fires
    let raw = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    Ok(ExtResponse::new(raw.into()))
}
```

### Known limitations of PR 7

- **Lossy export from acp-runner**: `ContentBlock::ToolUse` and `ContentBlock::Thinking`
  are dropped. Acceptable in v1 — code state is on disk.
- **Codex history is text-only**: tool calls accumulated as raw text, not structured blocks.
- **Codex pending_history requires empirical validation**: prepending imported history
  as text in the first `userInput` bypasses the Codex CLI's internal thread history
  management and has not been tested against the real Codex CLI. PR 7 must not be
  merged until this is validated. Fallback: fresh session when switching to codex.

**Branch:** `feat/session-export-import`

---

## Wave 2

### PR 8 — `CrossRunnerSwitcher` in `trogon-cli`

Depends on: PR 7 (Wave 1), PR 6 from Dev B (find_by_model — stub available from Day 0,
integration testing requires Dev B's PR 6 to be merged)

`Bridge<N,C,J>` is a `!Send` type (contains `RefCell<Vec<JoinHandle<()>>>` — verified
in `acp-nats/src/agent/bridge.rs:42`), so `CrossRunnerSwitcher` must live inside a
`tokio::task::LocalSet`.

#### Step 1 — Add dependencies to `trogon-cli`

```toml
acp-nats              = { workspace = true }
async-nats            = { workspace = true, features = ["jetstream"] }
trogon-std            = { workspace = true }
opentelemetry         = { workspace = true }
trogon-registry       = { workspace = true }
agent-client-protocol = { workspace = true }
```

Notes:
- `async-nats` with `features = ["jetstream"]` — not re-exported by `acp-nats`.
- `trogon-std` for `SystemClock` — not re-exported by `acp-nats`.
- `opentelemetry::global::meter("trogon-cli")` returns a no-op meter — acceptable for CLI.

#### Step 2 — `CrossRunnerSwitcher` struct

Add `trogon-cli/src/cross_runner.rs`:

```rust
use std::collections::HashMap;
use acp_nats::{AcpPrefix, Bridge, Config, NatsJetStreamClient};
use agent_client_protocol::{Agent, ExtRequest, NewSessionRequest};
use trogon_registry::{Registry, RegistryStore};
use trogon_std::time::SystemClock;

type ConcreteBridge = Bridge<async_nats::Client, SystemClock, NatsJetStreamClient>;

pub struct CrossRunnerSwitcher<S: RegistryStore> {
    nats: async_nats::Client,
    base_config: Config,
    registry: Registry<S>,
    bridges: HashMap<String, ConcreteBridge>,
}

impl<S: RegistryStore> CrossRunnerSwitcher<S> {
    pub fn new(nats: async_nats::Client, base_config: Config, registry: Registry<S>) -> Self {
        Self { nats, base_config, registry, bridges: HashMap::new() }
    }

    /// Switch the active session to whichever runner owns `model_id`.
    /// Returns `(target_prefix, new_session_id)`.
    /// Returns original pair unchanged if model is already on current runner.
    ///
    /// Must be called from within a `tokio::task::LocalSet` — Bridge is `!Send`.
    pub async fn switch_model(
        &mut self,
        current_prefix: &str,
        current_session_id: &str,
        model_id: &str,
        cwd: &str,
    ) -> Result<(String, String), String> {
        // 1. Resolve target runner from registry
        let cap = self.registry
            .find_by_model(model_id).await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no runner found for model: {model_id}"))?;
        let target_prefix = cap.metadata["acp_prefix"]
            .as_str()
            .ok_or("missing acp_prefix in registry metadata")?
            .to_string();
        if target_prefix == current_prefix {
            return Ok((current_prefix.to_string(), current_session_id.to_string()));
        }

        // 2. Ensure both bridges exist before any borrow
        self.ensure_bridge(current_prefix)?;
        self.ensure_bridge(&target_prefix)?;

        // 3. Export history as raw JSON
        let export_params = serde_json::value::RawValue::from_string(
            serde_json::json!({ "sessionId": current_session_id }).to_string()
        ).map_err(|e| e.to_string())?;
        let messages_json = {
            let bridge = self.bridges.get(current_prefix).unwrap();
            bridge.ext_method(ExtRequest { method: "session/export".into(), params: export_params })
                .await.map_err(|e| e.to_string())?
                .result
        };

        // 4. Open new session on target runner (same workspace path)
        let new_session_id = {
            let bridge = self.bridges.get(&target_prefix).unwrap();
            bridge.new_session(NewSessionRequest::new(cwd))
                .await.map_err(|e| e.to_string())?
                .session_id.to_string()
        };

        // 5. Import: pipe export JSON directly without re-serialization
        let import_params = serde_json::value::RawValue::from_string(format!(
            r#"{{"sessionId":"{new_session_id}","messages":{}}}"#,
            messages_json.get()
        )).map_err(|e| e.to_string())?;
        {
            let bridge = self.bridges.get(&target_prefix).unwrap();
            bridge.ext_method(ExtRequest { method: "session/import".into(), params: import_params })
                .await.map_err(|e| e.to_string())?;
        }

        Ok((target_prefix, new_session_id))
    }

    fn ensure_bridge(&mut self, prefix: &str) -> Result<(), String> {
        if self.bridges.contains_key(prefix) {
            return Ok(());
        }
        let acp_prefix = AcpPrefix::new(prefix).map_err(|e| e.to_string())?;
        let config = self.base_config.for_prefix(acp_prefix);
        let js = NatsJetStreamClient::new(async_nats::jetstream::new(self.nats.clone()));
        let (notification_tx, _rx) = tokio::sync::mpsc::channel(1);
        let bridge = Bridge::new(
            self.nats.clone(),
            js,
            SystemClock,
            &opentelemetry::global::meter("trogon-cli"),
            config,
            notification_tx,
        );
        self.bridges.insert(prefix.to_string(), bridge);
        Ok(())
    }
}
```

Key design decisions:
- **`ensure_bridge` then `bridges.get`**: avoids holding a `&Bridge` reference across
  an `ensure_bridge` call. Each block drops the borrow before the next block starts.
- **Raw JSON piping**: export result passed directly to import without deserializing.
  CrossRunnerSwitcher is agnostic to the message format.

#### Step 3 — Expose from `trogon-cli`

In `trogon-cli/src/lib.rs`:
```rust
pub mod cross_runner;
pub use cross_runner::CrossRunnerSwitcher;
```

### Known limitations of PR 8

- **LocalSet required**: `Bridge` is `!Send`. Terminal REPL runs in a `LocalSet`.
- **Session ID changes**: after migration, the REPL holds a new session ID and prefix.
- **trogon-acp stays Claude-only in v1**: IDE sessions use embedded acp-runner only.

**Branch:** `feat/cross-runner-switcher`

---

## Working branches

```
feat/programming-assistant      ← shared base branch with Dev B
  feat/session-export-import    ← PR 7
  feat/cross-runner-switcher    ← PR 8
```

Each feature branch makes a PR to `feat/programming-assistant`, not directly to `platform`.

### Coordination with Dev B

PR 8 uses `find_by_model` (implemented in Dev B's PR 6). The Day 0 stub lets Dev A
compile and write unit tests immediately. Integration testing requires Dev B's PR 6
to be merged first.

1. Dev B finishes PR 6, merges to `feat/programming-assistant`
2. Dev B notifies Dev A
3. Dev A syncs: `git fetch origin && git merge origin/feat/programming-assistant`
4. Dev A runs integration tests for PR 8

The `/model` slash command (Dev B's PR 9 extension) integrates with `CrossRunnerSwitcher`.
Dev B can prepare the call site against the public API of PR 8 before PR 8 is merged.

---

## Files touched

### PR 7
- `trogon-runner-tools/src/portable_session.rs` — implement PortableMessage (scaffolded Day 0)
- `trogon-runner-tools/src/lib.rs` — pub mod portable_session (scaffolded Day 0)
- `trogon-acp-runner/src/agent.rs` — session/export and session/import arms
- `trogon-xai-runner/src/agent.rs` — session/export and session/import arms
- `trogon-codex-runner/src/agent.rs` — history accumulation + export/import arms
- `trogon-openrouter-runner/src/agent.rs` — session/export and session/import arms

### PR 8
- `trogon-cli/Cargo.toml` — add acp-nats, async-nats [jetstream], trogon-std, opentelemetry, trogon-registry, agent-client-protocol
- `trogon-cli/src/cross_runner.rs` — new: CrossRunnerSwitcher
- `trogon-cli/src/lib.rs` — pub mod cross_runner; pub use CrossRunnerSwitcher

---

## Tool contract — definitive names

The names are fixed here. **There is no pending coordination.** Dev A implements them,
Dev B registers them in `dispatch_tool()`.

| Tool | String in `dispatch_tool()` |
|---|---|
| Read file | `"read_file"` |
| Write file | `"write_file"` |
| List directory | `"list_dir"` |
| Search by pattern | `"glob"` |
| Edit with replacement | `"str_replace"` |
| Git status | `"git_status"` |
| Git diff | `"git_diff"` |
| Git log | `"git_log"` |
| Fetch URL | `"fetch_url"` |
| Edit notebook | `"notebook_edit"` |
| Create/update task | `"todo_write"` |
| Read tasks | `"todo_read"` |
| Launch sub-agent | `"spawn_agent"` |

### `ToolContext` struct

```rust
pub struct ToolContext {
    pub proxy_url: String,
    pub cwd: String,           // working directory — comes from session.cwd = current_dir()
    pub http_client: reqwest::Client,
}
```

### Shared file with conflict risk

`trogon-agent-core/src/tools/mod.rs` — Dev A and Dev B both touch `dispatch_tool()`.
Coordinated in Day 0: all match arms added at once so no subsequent conflict.
