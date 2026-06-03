# TrogonAI — Programming Assistant Implementation Plan

TrogonAI is a terminal-based programming assistant with multi-runner support.

> Respects the NATS architecture as backend without exception.

---

## What is already ready (do not touch)

| Capability | Status |
|---|---|
| Real-time token streaming | ✅ platform branch |
| Vision / images | ✅ |
| Extended thinking | ✅ |
| Prompt caching | ✅ |
| Approval gates (sensitive tools) | ✅ platform branch |
| Token tracking per session | ✅ platform branch |
| `trogon_md.rs` in `trogon-acp-runner` | ✅ (extended in PR 4) |
| Context compaction (`trogon-compactor`) | ✅ (threshold added in PR 4) |
| Multi-model (Claude + Grok + OpenAI + OpenRouter) | ✅ |
| Encrypted credentials vault | ✅ |
| Distributed multi-agent | ✅ |

---

## Runners

| Runner | Model provider | Crate |
|--------|---------------|-------|
| `trogon-acp-runner` | Anthropic Claude | reference implementation |
| `trogon-xai-runner` | xAI Grok | `feat/xai-runner` |
| `trogon-codex-runner` | OpenAI Codex CLI | `feat/codex-runner` |
| `trogon-openrouter-runner` | OpenRouter (any model) | `openrouter-integration` |

---

## Gap summary by runner (current state)

| Feature | acp-runner | xai-runner | codex-runner | openrouter-runner |
|---------|-----------|-----------|-------------|------------------|
| TROGON.md injection | ✅ | ❌ | ❌ | ❌ |
| `_meta.systemPrompt` in `new_session` | ✅ | ❌ | n/a | ❌ |
| `ext_method` / `session/get_state` | ✅ | ❌ | ❌ | ❌ (no ext_method at all) |
| `{prefix}.agent.spawn` handler | ❌ | ❌ | ❌ | ❌ |
| Registry capabilities beyond `chat` | ✅ | ❌ | ❌ | ❌ |
| Full agentic tool execution | ✅ | ❌ | ✅ (via Codex CLI) | ❌ |

---

## Required PRs

These PRs are strictly necessary for the core use case: **programming, model
selection, and mid-session model switching**.

---

### PR 1 — Core tools in `trogon-agent-core` [Required]

**`crates/trogon-agent-core/Cargo.toml` — new dependencies**
- `globset = "0.4"`
- `ignore = "0.4"`
- `walkdir = "2"`
- `html2text = "0.12"`

**`crates/trogon-agent-core/src/tools/mod.rs`**
- Add field `cwd: String` to `ToolContext` (session working directory)
- Complete `dispatch_tool()` with routing:
  - `"read_file"` → `fs::read_file`
  - `"write_file"` → `fs::write_file`
  - `"list_dir"` → `fs::list_dir`
  - `"glob"` → `fs::glob_files`
  - `"str_replace"` → `editor::str_replace`
  - `"git_status"` → `git::status`
  - `"git_diff"` → `git::diff`
  - `"git_log"` → `git::log`
  - `"fetch_url"` → `web::fetch_url`

```rust
pub struct ToolContext {
    pub proxy_url: String,
    pub cwd: String,           // working directory — from session.cwd = current_dir()
    pub http_client: reqwest::Client,
}
```

**`crates/trogon-agent-core/src/tools/fs.rs` — new file**

`read_file`
- Parameters: `path: String`, `offset?: u32`, `limit?: u32`
- Resolves path relative to `ctx.cwd`
- Rejects paths outside `cwd` (path traversal protection)
- Returns content with line numbers in `cat -n` style
- If `offset`/`limit` present, returns only that line range

`write_file`
- Parameters: `path: String`, `content: String`
- Creates intermediate directories with `tokio::fs::create_dir_all`
- Atomic write: writes to `.tmp` then `rename`

`list_dir`
- Parameters: `path?: String` (default `"."`)
- Uses `ignore` crate to respect `.gitignore`
- Returns directory tree in `tree` style, limited to 500 entries

`glob`
- Parameters: `pattern: String`, `path?: String`
- Uses `globset` for pattern matching (`src/**/*.rs`, `**/*.test.ts`)
- Filters entries ignored by `.gitignore` via `ignore::Walk`

`notebook_edit`
- Parameters: `path: String`, `cell_index: u32`, `content: String`, `cell_type?: String`
- Reads `.ipynb` (pure JSON), edits the cell by index, writes back

**`crates/trogon-agent-core/src/tools/editor.rs` — new file**

`str_replace`
- Parameters: `path: String`, `old_str: String`, `new_str: String`
- Verifies `old_str` appears **exactly once** — returns error with count if not
- Returns diff of changed lines with ±3 lines of context

**`crates/trogon-agent-core/src/tools/git.rs` — new file**

`git_status`, `git_diff`, `git_log`
- Execute `git status --short`, `git diff`, `git log --oneline -20`
- Use `tokio::process::Command` with `current_dir(&ctx.cwd)`
- Output limited to ~4KB; truncated with notice if exceeded

**`crates/trogon-agent-core/src/tools/web.rs` — new file**

`fetch_url`
- Parameters: `url: String`, `raw?: bool`
- Reuses `ctx.http_client`
- If `raw: false` (default): converts HTML to plain text with `html2text`
- Truncates to 8KB with notice if response is larger

#### Tool contract — definitive names

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

---

### PR 2 — Stateful bash [Required]

**`crates/trogon-acp-runner/src/wasm_bash_tool.rs`**
- Add field `sandbox_dir: PathBuf` to `WasmRuntimeBashTool`
- Pass as `cwd` in `CreateTerminalRequest` so bash executes in the real project directory
- `sandbox_dir` initialized from `session.cwd` in `new_session`
- First bash call: create terminal, save `terminal_id` in `SessionState`
- Subsequent calls: reuse terminal with `WriteToTerminalRequest`
- Command end demarcation: `<command>; echo "__EXIT_$?__"`
- Read stream until `__EXIT_N__`, extract exit code
- Configurable timeout, default 30s

**`crates/trogon-acp-runner/src/session_store.rs`**
- Add `terminal_id: Option<String>` to `SessionState`

---

### PR 3 — `trogon-cli` REPL [Required]

**`crates/trogon-cli/Cargo.toml` — new crate**
- `rustyline = "14"`
- `acp-nats`, `clap`, `axum` — already in the workspace

**`crates/trogon-cli/src/main.rs`**
- Argument parsing with clap
- Reads `TROGON_NATS_URL` (default `nats://localhost:4222`)
- NATS autostart: if connection fails, launches `nats-server -p 4222` as child
  - Retries up to 3s with 200ms intervals
  - If `nats-server` not in PATH: prints instructions and exits with clear error
  - On exit: kills child process only if this process launched it

**`crates/trogon-cli/src/session.rs`**
- Creates ACP session with `cwd = std::env::current_dir()`
- Closes session cleanly on exit

**`crates/trogon-cli/src/repl.rs`**
- REPL loop with rustyline
- Ctrl+C cancels active session, Ctrl+D exits cleanly
- History persisted in `~/.local/share/trogon/history`
- Receives ACP event stream, prints `TextDelta` in real time

**`crates/trogon-agent/src/tools/mod.rs`**
- Register all PR 1 tools in `dispatch_tool()`

---

### PR 4 — Auto-compact + TROGON.md injection to all runners [Required]

This PR does two things: (1) adds auto-compact threshold to acp-runner, and (2)
moves the `trogon_md` module to `trogon-runner-tools` and extends it to all runners
so every runner has workspace-aware system prompts.

Auto-compact is required — without it, long programming sessions eventually exceed
the context window limit, causing a hard API failure that loses the conversation.

#### Auto-compact in acp-runner

**`crates/trogon-acp-runner/src/agent.rs`**

Change from unconditional compact to 85% threshold:
```rust
let token_estimate = estimate_token_count(&session.messages); // heuristic len_bytes / 4
if token_estimate > TOKEN_BUDGET * 85 / 100 {
    compact_messages(&mut session).await?;
}
// TOKEN_BUDGET configurable in SessionState, default 200_000
```

#### TROGON.md to all runners

**Step 1 — Move `trogon_md` to `trogon-runner-tools`**

Move `trogon-acp-runner/src/trogon_md.rs` → `trogon-runner-tools/src/trogon_md.rs`.

In `trogon-runner-tools/src/lib.rs`:
```rust
pub mod trogon_md;
```

In `trogon-acp-runner`, replace with re-export:
```rust
pub use trogon_runner_tools::trogon_md;
```

**Step 2 — Add dependency to xai, codex, openrouter runners**

```toml
trogon-runner-tools = { path = "../trogon-runner-tools" }
```

**Step 3 — xai-runner: inject at prompt time**

`XaiSession` already has `cwd: String` (line 66). In the prompt handler:
```rust
let trogon_md = trogon_runner_tools::trogon_md::load_trogon_md(&cwd).await;
let effective_system_prompt = match (trogon_md, session_system_prompt) {
    (Some(md), Some(sp)) => Some(format!("{md}\n\n{sp}")),
    (Some(md), None)     => Some(md),
    (None, sp)           => sp,
};
```

**Step 4 — openrouter-runner: inject at prompt time**

`OpenRouterSession` already has `cwd: String` (line 46). Apply the same pattern.

**Step 5 — codex-runner: inject on first turn only**

Codex has no system prompt API — TROGON.md must be prepended to the first user message.

Add `first_turn: bool` to `CodexSession`:
```rust
struct CodexSession {
    thread_id: String,
    cwd: String,
    model: Option<String>,
    parent_session_id: Option<String>,
    first_turn: bool,   // true until the first prompt is processed
}
```

Initialize to `true` in `new_session`. In the prompt handler:
```rust
let (thread_id, model, cwd, first_turn) = {
    let mut sessions = self.sessions.lock().await;
    let s = sessions.get_mut(&session_id)
        .ok_or_else(|| internal_error(format!("session {session_id} not found")))?;
    let ft = s.first_turn;
    s.first_turn = false;
    (s.thread_id.clone(), s.model.clone(), s.cwd.clone(), ft)
};

let user_input = if first_turn {
    match trogon_runner_tools::trogon_md::load_trogon_md(&cwd).await {
        Some(md) => format!("{md}\n\n{user_input}"),
        None     => user_input,
    }
} else {
    user_input
};
```

---

### PR 5 — `ext_method` scaffold in xai-runner, codex-runner, openrouter-runner [Required]

`trogon-acp-runner` handles `"session/list_children"` and `"session/get_state"` in
its `ext_method` implementation. xai-runner and codex-runner handle
`"session/list_children"` but not `"session/get_state"`. openrouter-runner has no
`ext_method` at all.

Add (or extend) the `ext_method` handler with a `"session/get_state"` arm:

```rust
"session/get_state" => {
    let params: serde_json::Value =
        serde_json::from_str(args.params.get())
            .map_err(|e| Error::new(ErrorCode::InvalidParams.into(), e.to_string()))?;
    let session_id = params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::new(ErrorCode::InvalidParams.into(), "missing sessionId"))?;
    let sessions = self.sessions.lock().await;
    let state = sessions.get(session_id).ok_or_else(|| {
        Error::new(ErrorCode::InvalidParams.into(), "session not found")
    })?;
    let raw = serde_json::to_string(state)
        .map_err(|e| Error::new(ErrorCode::InternalError.into(), e.to_string()))?;
    Ok(ExtResponse::new(raw.into()))
}
```

Add `#[derive(Serialize)]` to `XaiSession`, `CodexSession`, and `OpenRouterSession`.

For openrouter-runner: add the full `ext_method` scaffolding including
`"session/list_children"` since the method does not exist yet.

---

### PR 6 — Registry capabilities and model list [Required]

All non-Claude runners currently register with only `"chat"` as their capability,
and no model list. `CrossRunnerSwitcher` (PR 7) needs `find_by_model` which reads
`metadata.models` from the registry.

**xai-runner** (`main.rs`):
```rust
let model_ids: Vec<String> = models
    .split(',')
    .filter_map(|entry| entry.split(':').next().map(|id| id.trim().to_string()))
    .collect();

let cap = trogon_registry::AgentCapability {
    agent_type: agent_type.clone(),
    capabilities: vec!["chat".to_string(), "explore".to_string(), "plan".to_string()],
    nats_subject: format!("{}.agent.>", prefix),
    current_load: 0,
    metadata: serde_json::json!({
        "acp_prefix": &prefix,
        "models": model_ids,
    }),
};
```

**openrouter-runner** — same pattern, parse `OPENROUTER_MODELS`.

**codex-runner** — same pattern, parse `CODEX_MODELS`:
```rust
capabilities: vec!["chat".to_string(), "code_edit".to_string()],
```

**acp-runner** — parse `session_model_state`, add to registry metadata.

**`trogon-registry/src/registry.rs` — add `find_by_model`**:
```rust
pub async fn find_by_model(&self, model_id: &str) -> Result<Option<AgentCapability>, String> {
    let all = self.store.list().await?;
    Ok(all.into_iter().find(|cap| {
        cap.metadata
            .get("models")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|m| m.as_str() == Some(model_id)))
            .unwrap_or(false)
    }))
}
```

---

### PR 7 — Cross-runner session export/import [Required]

Depends on: PR 4 (codex `first_turn` field), PR 5 (openrouter ext_method scaffold)

This PR enables mid-session model switching by giving each runner the ability to
export its conversation history as portable messages and import history from another runner.

#### Step 1 — `PortableMessage` in `trogon-runner-tools`

New file `trogon-runner-tools/src/portable_session.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortableMessage {
    pub role: String,  // "user" | "assistant"
    pub text: String,
}
```

In `trogon-runner-tools/src/lib.rs`:
```rust
pub mod portable_session;
```

#### Step 2 — `session/export` in each runner

**xai-runner**:
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

**acp-runner** — reads from KV:
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

Note: `ContentBlock::ToolUse` and `ContentBlock::Thinking` are intentionally
dropped. The receiving runner only speaks text. This lossy conversion is unavoidable
when migrating away from acp-runner.

**codex-runner** — reads from the accumulated `history` field added below:
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

#### Step 3 — History accumulation in codex-runner

`CodexSession` currently stores no history. Add two fields:
```rust
struct CodexSession {
    thread_id: String,
    cwd: String,
    model: Option<String>,
    parent_session_id: Option<String>,
    first_turn: bool,                              // for TROGON.md injection (PR 4)
    history: Vec<PortableMessage>,                 // shadow copy for export
    pending_history: Option<Vec<PortableMessage>>, // imported, prepended on first turn
}
```

In the prompt handler, use `get_mut` to update `first_turn` and capture
`pending_history` atomically:
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

#### Step 4 — `session/import` in each runner

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
    s.last_response_id = None; // stale ID cleared so next prompt sends full history
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

**codex-runner** — sets both `s.history` and `pending_history`:
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
    // Write to both: s.history makes export consistent before the first prompt;
    // s.pending_history injects the context into the subprocess on the first turn.
    s.history = messages.clone();
    s.pending_history = Some(messages);
    s.first_turn = true;  // reset so prepend fires
    let raw = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    Ok(ExtResponse::new(raw.into()))
}
```

### Known limitations of PR 7

- **Lossy export from acp-runner**: `ContentBlock::ToolUse` and `ContentBlock::Thinking`
  are dropped. The receiving runner sees tool interactions as plain text. This is
  inherent to cross-API portability and acceptable in v1 — code state is on disk
  and the new model can re-read files it needs.
- **Codex history is text-only**: tool calls from Codex are accumulated as raw
  text output, not structured tool-use blocks.
- **Codex pending_history validated (2026-05-19)**: the prepend mechanism was tested
  against the mock Codex CLI end-to-end. It works correctly. Two additional fixes were
  applied during validation: (1) `session/import` now writes to both `s.history` and
  `s.pending_history` so that `session/export` before the first prompt returns the
  imported messages immediately (consistent with all other runners); (2) xai-runner
  `session/import` now clears `last_response_id` so the next prompt sends the full
  history instead of using the stale stateful API thread. Both fixes are covered by
  unit tests, in-process integration tests, NATS e2e tests, and live smoke tests.
- **Within-runner model switching is unaffected**: `set_session_model` continues to
  work as-is for models within the same runner.

---

### PR 8 — `CrossRunnerSwitcher` in `trogon-cli` [Required]

Depends on: PR 6 (models in registry), PR 7 (export/import ext_methods)

Cross-runner switching must be orchestrated by the ACP client that holds the active
session reference.

`trogon-acp` (IDE client) stays Claude-only in v1: its `new_session` writes to
`ACP_SESSIONS` KV while each runner's `new_session` writes to its own in-memory
`HashMap`. There is no shared session namespace, so building multi-runner logic into
`trogon-acp` is not needed in v1.

`trogon-cli` is the terminal client. This PR adds `CrossRunnerSwitcher`.

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
- `async-nats` with `features = ["jetstream"]` is needed for `async_nats::Client`
  and `async_nats::jetstream::new()`. It is not re-exported by `acp-nats`.
- `trogon-std` is needed for `SystemClock`. Not re-exported by `acp-nats`.
- `opentelemetry` is needed for `&opentelemetry::metrics::Meter` in `Bridge::new`.
  `opentelemetry::global::meter("trogon-cli")` returns a no-op meter if OTel is not
  initialized — acceptable for a CLI tool.

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
    /// Returns the original pair unchanged if the model is already on the current runner.
    ///
    /// `cwd` must be the absolute workspace path used when the original session was created.
    /// Runners store it verbatim and use it for TROGON.md injection on every turn.
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
        // Receiver dropped immediately: notification_sender is only used in prompt.rs,
        // never called by new_session or ext_method.
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
- **Raw JSON piping**: export result is passed directly to import without deserializing
  into `PortableMessage`. CrossRunnerSwitcher is agnostic to the message format.
- **`base_config.for_prefix()`**: derives per-runner configs from the base config.

#### Step 3 — Expose from `trogon-cli`

In `trogon-cli/src/lib.rs`:
```rust
pub mod cross_runner;
pub use cross_runner::CrossRunnerSwitcher;
```

#### Step 4 — Integration with REPL

The terminal REPL's `/model <id>` slash command (PR 9) calls:
```
CrossRunnerSwitcher::switch_model(current_prefix, current_session_id, model_id, workspace_cwd)
```
where `workspace_cwd` is the absolute path used when creating the original session.
The REPL replaces its local `current_prefix` and `current_session_id` with the
returned values and continues sending prompts to the new runner.

### Known limitations of PR 8

- **LocalSet required**: `Bridge` is `!Send`. The terminal REPL runs in a `LocalSet`,
  same as `acp-nats-stdio`. This is the expected execution model for ACP clients.
- **Session ID changes**: after migration, the REPL holds a new session ID and prefix.
- **trogon-acp stays Claude-only in v1**: IDE sessions use the embedded acp-runner
  only. Multi-runner for IDE is deferred to a future PR.

---

### PR 9 — Slash commands including `/model` [Required]

Depends on: PR 8 (CrossRunnerSwitcher)

**`crates/trogon-cli/src/repl.rs`**

Commands execute locally, not sent to the agent:

| Command | Action |
|---|---|
| `/model <id>` | Switches model mid-session via `CrossRunnerSwitcher::switch_model` |
| `/clear` | Clears session message history via NATS KV |
| `/compact` | Forces context compaction now (`trogon.compactor.compact`) |
| `/cost` | Shows accumulated tokens and $ for the current session |
| `/help` | Lists all available commands |
| `/config` | Reads/writes local CLI config |
| `/init` | Analyzes the project with LLM via ACP, generates `TROGON.md` |

---

## Optional PRs

These PRs improve the experience but are not required for the core use case.

---

### PR 10 — Agent extensibility: @mentions and parallel tools [Optional]

**`crates/trogon-cli/src/repl.rs` — file `@mentions`**
- Before sending the prompt, scan for `@<path>` tokens
- For each match: resolve relative to `cwd`, read content, replace with code block
- If path does not exist, leave unmodified and warn
- Path tab-completion in rustyline via a custom `Helper`

**`crates/trogon-agent-core/src/agent_loop.rs` — parallel tools**
```rust
// before (sequential)
for call in &tool_calls {
    let result = dispatch_tool(&ctx, &call.name, &call.input).await;
}

// after (parallel)
let futures: Vec<_> = tool_calls.iter()
    .map(|call| dispatch_tool(&ctx, &call.name, &call.input))
    .collect();
let results = futures::future::join_all(futures).await;
```
Restriction: if an interactive `PermissionChecker` is active, serialize calls.

---

### PR 11 — MCP stdio as CLI HTTP proxy [Optional]

**`crates/trogon-cli/src/stdio_mcp_bridge.rs` — new file**
- CLI launches MCP stdio process as child
- Starts axum server on random local port, proxies JSON-RPC ↔ stdin/stdout
- Registers as `StoredMcpServer::Http` in session
- On exit: kills child process and releases port

---

### PR 12 — Todo tools and specialized sub-agents [Optional]

**`crates/trogon-agent-core/src/tools/mod.rs`**
- `todo_write`: `id: String`, `content: String`, `status: String` (pending/in_progress/completed) — NATS KV per session
- `todo_read`: returns list of active tasks for the session

**`crates/trogon-agent/src/tools/mod.rs`**
- `spawn_agent` ToolDef — NATS request-reply to `{prefix}.agent.spawn`

**`trogon-registry`**
- Register `Explore` agent: read-only, never edits or executes
- Register `Plan` agent: only plans, no destructive tools

---

### PR 13 — TUI colored diffs [Optional]

**`crates/trogon-cli/src/repl.rs`**
- Show colored diffs (before/after) on each `str_replace` and `write_file`
- Show token and $ cost per session via `UsageSummary` events
- Multiline input support

---

### PR 14 — Non-interactive mode and granular permissions [Optional]

**`crates/trogon-cli/src/print.rs`**
- `trogon --print "do X"` or `trogon -p "do X"`
- Reads from stdin: `trogon --print < error.log`
- Prints only `TextDelta`, exit code 0/1
- Useful for pipes and CI/CD

**`crates/trogon-acp-runner` — granular permissions**
- Allowlist/denylist by path (e.g. `allow: src/**, deny: .env`)
- Allowlist/denylist by bash command
- Configurable in `TROGON.md` and via `/config`

---

### PR 15 — `_meta.systemPrompt` in xai-runner and openrouter-runner [Optional]

Both runners already have `system_prompt: Option<String>` in their session struct.

In `new_session`, after reading the agent/env system prompt:
```rust
let meta_system_prompt = req.meta
    .as_ref()
    .and_then(|m| m.get("systemPrompt"))
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());

let system_prompt = meta_system_prompt.or(session_system_prompt);
```

Codex does not apply: it has no system prompt concept.

---

### PR 16 — Spawn handler in xai-runner and openrouter-runner [Optional]

Enables xai-runner and openrouter-runner to handle `{prefix}.agent.spawn` requests
for one-shot sub-agent calls (no tool-use loop).

In `trogon-xai-runner/src/main.rs`, clone `nats` before it moves into
`AgentSideNatsConnection::with_jetstream`, then:

```rust
let spawn_nats  = nats.clone();
let spawn_key   = api_key.clone();
let spawn_model = default_model.clone();
let spawn_pfx   = prefix.clone();

tokio::spawn(async move {
    use futures_util::StreamExt as _;
    let mut sub = spawn_nats
        .subscribe(format!("{spawn_pfx}.agent.spawn"))
        .await
        .expect("failed to subscribe to agent.spawn");

    while let Some(msg) = sub.next().await {
        let Some(reply) = msg.reply else { continue };
        let Ok(req) = serde_json::from_slice::<serde_json::Value>(&msg.payload) else { continue };
        let prompt = req["prompt"].as_str().unwrap_or("").to_string();
        let nats2 = spawn_nats.clone();
        let key2  = spawn_key.clone();
        let model2 = spawn_model.clone();
        tokio::spawn(async move {
            let result = oneshot_call(&key2, &model2, &prompt).await;
            nats2.publish(reply, result.into()).await.ok();
        });
    }
});
```

Add `oneshot_call` in `trogon-xai-runner/src/spawn_handler.rs`: single API call,
no session, no history, no tools. Apply same pattern in openrouter-runner.

Use NATS queue group (`queue_subscribe`) if both runners subscribe to avoid
duplicate handling.

---

## Architecture notes

### Tool execution by runner

| Runner | Tool execution | Tools available |
|--------|---------------|-----------------|
| `trogon-acp-runner` | Server-side, full agentic loop | read_file, write_file, bash, search, git, web, MCP |
| `trogon-codex-runner` | Managed by Codex CLI subprocess | file ops, bash — whatever Codex CLI supports |
| `trogon-xai-runner` | xAI server-side only | web_search, x_search (xAI native) |
| `trogon-openrouter-runner` | None currently | — |

PRs 1–9 make all four runners feature-parity for chat and model switching.
For full agentic programming (file reads, bash, iterative edits), only
`trogon-acp-runner` and `trogon-codex-runner` are capable. This is a deliberate
design reflected in the existing code.

---

## Known gaps

**Gap A — No custom tool execution in xai-runner and openrouter-runner**

When the user switches to Grok or an OpenRouter model mid-session, the history
migrates correctly but the agent loses all programming tools. It can converse but
not edit files or execute commands.

For openrouter-runner: the path is defined — the chat completions format supports
function calling (OpenAI spec). Requires: adding tool definitions to the API request,
parsing `tool_calls` deltas in SSE, adding an execution loop using `trogon-tools`,
feeding results back as a `tool` role message.

For xai-runner: **validated 2026-05-14** — grok-4 supports custom function definitions
via the Responses API. A test call with a `tools` array returned a `function_call`
output block with `parallel_tool_calls: true`. The `previous_response_id` field is
present in the response, so the stateful conversation optimization is preserved — no
need to switch to the OpenAI-compatible endpoint and no SSE parser rewrite required.
The implementation loop: send request with tools → parse `function_call` blocks in
output → execute with `dispatch_tool` → send follow-up with `previous_response_id`
and `function_call_output` input blocks → repeat until no `function_call` in output.
This is not a design gap — it is straightforward implementation work.

**Gap B — Context compaction in xai-runner and openrouter-runner**

These runners trim history by message count only (`max_history`, default 20),
dropping oldest messages blindly. The fix is to call `trogon-compactor`
(`trogon.compactor.compact`) the same way acp-runner does. Requires adding the NATS
call before the trim and converting `Vec<Message>` → `Vec<compactor::Message>`.

**Gap C — spawn_agent sub-agents cannot execute tools**

PR 12 and PR 16 implement `agent.spawn` as a one-shot API call — no tool-use loop.
For programming sub-agents that need to read files or run commands, this is
insufficient. A full tool-using session requires a different spawn architecture.
Deferred — the one-shot implementation covers summarisation, planning, and text
generation sub-agents.

---

## Dependency order

| PR | Depends on | Can parallelize with |
|----|------------|----------------------|
| PR 1 — Core tools | nothing | PR 2, 3, 4, 5, 6 |
| PR 2 — Stateful bash | nothing | PR 1, 3, 4, 5, 6 |
| PR 3 — trogon-cli REPL | nothing | PR 1, 2, 4, 5, 6 |
| PR 4 — Auto-compact + TROGON.md all runners | nothing | PR 1, 2, 3, 5, 6 |
| PR 5 — ext_method scaffold | nothing | PR 1, 2, 3, 4, 6 |
| PR 6 — Registry capabilities + model list | nothing | PR 1, 2, 3, 4, 5 |
| PR 7 — Session export/import | PR 4 (codex first_turn), PR 5 (openrouter ext_method) | PR 1, 2, 3, 6 |
| PR 8 — CrossRunnerSwitcher | PR 6 (find_by_model), PR 7 (export/import) | PR 1, 2, 3, 4, 5 |
| PR 9 — Slash commands (/model) | PR 8 (CrossRunnerSwitcher) | PR 1, 2, 3, 4, 5, 6, 7 |

Optional PRs (10–16) have no dependencies on each other and can be parallelized
freely after the required PRs are in place.

---

## Files touched per PR

### PR 1
- `trogon-agent-core/Cargo.toml` — add globset, ignore, walkdir, html2text
- `trogon-agent-core/src/tools/mod.rs` — add `cwd: String` to ToolContext, complete dispatch_tool()
- `trogon-agent-core/src/tools/fs.rs` — new: read_file, write_file, list_dir, glob, notebook_edit
- `trogon-agent-core/src/tools/editor.rs` — new: str_replace
- `trogon-agent-core/src/tools/git.rs` — new: git_status, git_diff, git_log
- `trogon-agent-core/src/tools/web.rs` — new: fetch_url
- `trogon-agent/src/tools/mod.rs` — register all PR 1 tools in dispatch_tool()

### PR 2
- `trogon-acp-runner/src/wasm_bash_tool.rs` — add sandbox_dir, terminal reuse, demarcation protocol
- `trogon-acp-runner/src/session_store.rs` — add terminal_id to SessionState

### PR 3
- `trogon-cli/Cargo.toml` — new crate: rustyline, acp-nats, clap, axum
- `trogon-cli/src/main.rs` — clap, NATS autostart, child process management
- `trogon-cli/src/session.rs` — ACP session with cwd = current_dir()
- `trogon-cli/src/repl.rs` — rustyline loop, TextDelta streaming, Ctrl+C/D, history

### PR 4
- `trogon-runner-tools/src/trogon_md.rs` — new (moved from acp-runner)
- `trogon-runner-tools/src/lib.rs` — add pub mod trogon_md
- `trogon-acp-runner/src/lib.rs` — replace module with re-export
- `trogon-acp-runner/src/agent.rs` — change to 85% threshold auto-compact
- `trogon-xai-runner/Cargo.toml` — add trogon-runner-tools dep
- `trogon-xai-runner/src/agent.rs` — call load_trogon_md at prompt time
- `trogon-codex-runner/Cargo.toml` — add trogon-runner-tools dep
- `trogon-codex-runner/src/agent.rs` — add first_turn field, prepend on first turn
- `trogon-openrouter-runner/Cargo.toml` — add trogon-runner-tools dep
- `trogon-openrouter-runner/src/agent.rs` — call load_trogon_md at prompt time

### PR 5
- `trogon-xai-runner/src/agent.rs` — add session/get_state arm, derive Serialize on XaiSession
- `trogon-codex-runner/src/agent.rs` — add session/get_state arm, derive Serialize on CodexSession
- `trogon-openrouter-runner/src/agent.rs` — add full ext_method with both arms, derive Serialize

### PR 6
- `trogon-xai-runner/src/main.rs` — expand capabilities list, add model_ids to registry metadata
- `trogon-codex-runner/src/main.rs` — expand capabilities list, add model_ids to registry metadata
- `trogon-openrouter-runner/src/main.rs` — expand capabilities list, add model_ids to registry metadata
- `trogon-acp-runner/src/main.rs` — add model_ids to registry metadata
- `trogon-registry/src/registry.rs` — add find_by_model method

### PR 7
- `trogon-runner-tools/src/portable_session.rs` — new: PortableMessage type
- `trogon-runner-tools/src/lib.rs` — add pub mod portable_session
- `trogon-acp-runner/src/agent.rs` — add session/export and session/import arms
- `trogon-xai-runner/src/agent.rs` — add session/export and session/import arms
- `trogon-codex-runner/src/agent.rs` — add history + pending_history fields, accumulation, export/import
- `trogon-openrouter-runner/src/agent.rs` — add session/export and session/import arms

### PR 8
- `trogon-cli/Cargo.toml` — add acp-nats, async-nats [jetstream], trogon-std, opentelemetry, trogon-registry, agent-client-protocol
- `trogon-cli/src/cross_runner.rs` — new: CrossRunnerSwitcher with switch_model and ensure_bridge
- `trogon-cli/src/lib.rs` — add pub mod cross_runner; pub use cross_runner::CrossRunnerSwitcher

### PR 9
- `trogon-cli/src/repl.rs` — add /model (calls CrossRunnerSwitcher), /clear, /compact, /cost, /help, /config, /init

### PR 10 (optional)
- `trogon-cli/src/repl.rs` — @mentions scan, tab-completion Helper
- `trogon-agent-core/src/agent_loop.rs` — replace sequential for with parallel join_all

### PR 11 (optional)
- `trogon-cli/src/stdio_mcp_bridge.rs` — new: MCP stdio HTTP proxy

### PR 12 (optional)
- `trogon-agent-core/src/tools/mod.rs` — add todo_write, todo_read
- `trogon-agent/src/tools/mod.rs` — add spawn_agent ToolDef
- `trogon-registry` — register Explore and Plan agents

### PR 13 (optional)
- `trogon-cli/src/repl.rs` — colored diffs, UsageSummary consumption, multiline input

### PR 14 (optional)
- `trogon-cli/src/print.rs` — --print/-p flag, stdin, exit codes
- `trogon-acp-runner` — allowlist/denylist by path and command

### PR 15 (optional)
- `trogon-xai-runner/src/agent.rs` — read req.meta["systemPrompt"] in new_session
- `trogon-openrouter-runner/src/agent.rs` — read req.meta["systemPrompt"] in new_session

### PR 16 (optional)
- `trogon-xai-runner/src/main.rs` — clone nats, launch spawn subscriber
- `trogon-xai-runner/src/spawn_handler.rs` — new: oneshot_call
- `trogon-openrouter-runner/src/main.rs` — clone nats, launch spawn subscriber
- `trogon-openrouter-runner/src/spawn_handler.rs` — new: oneshot_call
