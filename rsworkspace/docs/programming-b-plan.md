# Programming B — Work plan
# Runners configuration + CLI surface

> All work is additive. NATS remains as backend without changes.
> The names of all tools and the ToolContext contract are fixed in the "Tool contract" section of Programming A.

---

## Wave summary

| Wave | PRs | Status |
|---|---|---|
| — | PR 3 + PR 9 + PR 10 + PR 13 + PR 14 | ✅ Implemented |
| Wave 1 | PR 4ext + PR 5 + PR 6 | New work — all parallel |
| Wave 2 | PR 9 extension (/model) | New work — after Dev A's PR 8 |
| Optional | PR 15 + PR 16 | Any time after Wave 1 |

---

## Implemented

### PR 3 — `trogon-cli` core ✅

**`crates/trogon-cli/Cargo.toml` — new crate**
```toml
rustyline = "14"
# acp-nats, clap, axum — already in the workspace
```

**`crates/trogon-cli/src/main.rs`**
- Argument parsing with clap
- Reads `TROGON_NATS_URL` (default `nats://localhost:4222`)
- NATS autostart:
  - If connection fails, launches `nats-server -p 4222` as child process
  - Retries up to 3s with 200ms intervals
  - If `nats-server` is not in PATH: prints installation instructions and exits with clear error
  - On exit: kills child process only if this process launched it

**`crates/trogon-cli/src/session.rs`**
- ACP session management via NATS
- Creates session with `cwd = std::env::current_dir()`
- Closes session cleanly on exit (Ctrl+D)

**`crates/trogon-cli/src/repl.rs`**
- REPL loop with rustyline
- Receives ACP event stream and prints `TextDelta` in real time
- Ctrl+C sends cancel to active session and clears state
- Ctrl+D exits cleanly closing the session
- History persisted in `~/.local/share/trogon/history`

**`crates/trogon-cli/src/print.rs`**
- Empty stub for now (completed in PR 14)

**Wiring in `trogon-agent/src/tools/mod.rs`**

Register in `dispatch_tool()` all tools from PR 1:
- `"read_file"`, `"write_file"`, `"list_dir"`, `"glob"`, `"str_replace"`
- `"git_status"`, `"git_diff"`, `"git_log"`, `"fetch_url"`, `"notebook_edit"`

```rust
use trogon_agent_core::tools::{fs, editor, git, web};

"read_file"      => fs::read_file(ctx, input).await,
"write_file"     => fs::write_file(ctx, input).await,
"list_dir"       => fs::list_dir(ctx, input).await,
"glob"           => fs::glob_files(ctx, input).await,
"str_replace"    => editor::str_replace(ctx, input).await,
"git_status"     => git::status(ctx, input).await,
"git_diff"       => git::diff(ctx, input).await,
"git_log"        => git::log(ctx, input).await,
"fetch_url"      => web::fetch_url(ctx, input).await,
"notebook_edit"  => fs::notebook_edit(ctx, input).await,
```

---

### PR 9 — Slash commands ✅

**`crates/trogon-cli/src/repl.rs`**

Commands execute locally, they are not sent to the agent:

| Command | Action |
|---|---|
| `/clear` | Clears session message history via NATS KV |
| `/compact` | Forces context compaction now (`trogon.compactor.compact`) |
| `/cost` | Shows accumulated tokens and $ for the current session |
| `/help` | Lists all available commands |
| `/config` | Reads/writes local CLI config |
| `/init` | Analyzes the project with LLM via ACP and generates `TROGON.md` |

Note: `/model` is NOT yet implemented — it depends on PR 8 (Dev A Wave 2). See PR 9 extension below.

---

### PR 10 — Agent extensibility ✅

**`crates/trogon-cli/src/repl.rs` — file `@mentions`**
- Before sending the prompt, scan for `@<path>` tokens
- For each match: resolve relative to `cwd`, read content, replace with code block
- If path does not exist: leave token unmodified and warn
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
Restriction: if an interactive `PermissionChecker` is active, serialize the calls.

---

### PR 13 — TUI ✅

**`crates/trogon-cli/src/repl.rs`**
- Show colored diffs (before/after) on each `str_replace` and `write_file` operation
- Ctrl+C cancels the active NATS operation and clears session state
- Consume `UsageSummary` events and show accumulated tokens and $ per session
- Multiline input

---

### PR 14 — Non-interactive mode + granular permissions ✅

**`crates/trogon-cli/src/print.rs`**
- Activated with `trogon --print "do X"` or `trogon -p "do X"`
- Reads prompt from argument or from stdin: `trogon --print "explain" < error.log`
- Prints only `TextDelta` to stdout (no colors or interactive UI)
- Exit code 0 on success, 1 if agent returns error
- Useful for pipes and CI/CD

**`crates/trogon-acp-runner`**
- Allowlist/denylist by path (e.g. `allow: src/**, deny: .env`)
- Allowlist/denylist by bash command (e.g. `allow: cargo test, deny: rm -rf`)
- Same approval gates mechanism from the platform branch (NATS request-reply)
- Configurable in `TROGON.md` and via `/config`

---

## Day 0 — scaffolding (shared with Dev A, half-day)

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
"session/export" => { todo!("PR 7 — Dev A") }
"session/import" => { todo!("PR 7 — Dev A") }
```

For openrouter-runner, add the full `ext_method` scaffold if missing:
```rust
async fn ext_method(&self, args: ExtRequest) -> Result<ExtResponse, Error> {
    match args.method.as_str() {
        "session/list_children" => { /* stub */ Ok(ExtResponse::new("[]".into())) }
        "session/export"        => { todo!("PR 7 — Dev A") }
        "session/import"        => { todo!("PR 7 — Dev A") }
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

## Wave 1 — all three PRs are independent and can be done in parallel

### PR 4 extension — TROGON.md injection to xai-runner, codex-runner, openrouter-runner

acp-runner already has TROGON.md injection (PR 4, implemented). This PR extends it
to the remaining three runners.

#### Step 1 — Move `trogon_md` to `trogon-runner-tools`

Move `trogon-acp-runner/src/trogon_md.rs` → `trogon-runner-tools/src/trogon_md.rs`.

In `trogon-runner-tools/src/lib.rs`:
```rust
pub mod trogon_md;
```

In `trogon-acp-runner`, replace the module with a re-export:
```rust
pub use trogon_runner_tools::trogon_md;
```

#### Step 2 — Add dependency to xai, codex, openrouter runners

```toml
trogon-runner-tools = { path = "../trogon-runner-tools" }
```

#### Step 3 — xai-runner: inject at prompt time

`XaiSession` already has `cwd: String`. In the prompt handler:
```rust
let trogon_md = trogon_runner_tools::trogon_md::load_trogon_md(&cwd).await;
let effective_system_prompt = match (trogon_md, session_system_prompt) {
    (Some(md), Some(sp)) => Some(format!("{md}\n\n{sp}")),
    (Some(md), None)     => Some(md),
    (None, sp)           => sp,
};
```

#### Step 4 — openrouter-runner: inject at prompt time

`OpenRouterSession` already has `cwd: String`. Apply the same pattern.

#### Step 5 — codex-runner: inject on first turn only

Codex has no system prompt API — TROGON.md must be prepended to the first user message.

Add `first_turn: bool` to `CodexSession` (if not already added in Day 0):
```rust
struct CodexSession {
    thread_id: String,
    cwd: String,
    model: Option<String>,
    parent_session_id: Option<String>,
    first_turn: bool,
    history: Vec<PortableMessage>,
    pending_history: Option<Vec<PortableMessage>>,
}
```

The `first_turn` injection is wired in the prompt handler by Dev A (PR 7). Dev B only
needs to ensure the field is initialized to `true` in `new_session`.

**Branch:** `feat/trogon-md-all-runners`

---

### PR 5 — `ext_method` scaffold in xai-runner, codex-runner, openrouter-runner

xai-runner and codex-runner handle `"session/list_children"` but not
`"session/get_state"`. openrouter-runner has no `ext_method` at all (added as stub
in Day 0). This PR adds `"session/get_state"` to all three.

Add the `"session/get_state"` arm in xai-runner, codex-runner, and openrouter-runner:

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

Note: the `"session/export"` and `"session/import"` arms were stubbed in Day 0 and
will be implemented by Dev A in PR 7. Do not touch those arms in this PR.

**Branch:** `feat/ext-method-scaffold`

---

### PR 6 — Registry capabilities and model list

All non-Claude runners currently register with only `"chat"` as their capability,
and no model list. `CrossRunnerSwitcher` (Dev A's PR 8) needs `find_by_model` which
reads `metadata.models` from the registry.

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

**`trogon-registry/src/registry.rs`** — implement `find_by_model` (replace the Day 0 stub):
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

After this PR, notify Dev A so they can run integration tests for PR 8.

**Branch:** `feat/registry-capabilities`

---

## Wave 2

### PR 9 extension — `/model` slash command

Depends on: Dev A's PR 8 (CrossRunnerSwitcher merged to `feat/programming-assistant`)

Add `/model <id>` to the existing slash command handler in `trogon-cli/src/repl.rs`:

```rust
"/model" => {
    let model_id = args.trim();
    if model_id.is_empty() {
        println!("Usage: /model <model-id>");
        println!("Example: /model anthropic/claude-opus-4-7");
        continue;
    }
    match switcher.switch_model(&current_prefix, &current_session_id, model_id, &cwd).await {
        Ok((new_prefix, new_session_id)) => {
            if new_prefix == current_prefix {
                println!("Already on a runner that supports {model_id}");
            } else {
                current_prefix = new_prefix;
                current_session_id = new_session_id;
                println!("Switched to {model_id}");
            }
        }
        Err(e) => eprintln!("Error switching model: {e}"),
    }
}
```

The REPL must hold a `CrossRunnerSwitcher` instance and track `current_prefix`,
`current_session_id`, and `cwd` as mutable state. Update `repl.rs` accordingly.

**Branch:** `feat/slash-commands-model`

---

## Optional PRs

### PR 15 — `_meta.systemPrompt` in xai-runner and openrouter-runner

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

**Branch:** `feat/system-prompt`

---

### PR 16 — Spawn handler in xai-runner and openrouter-runner

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

**Branch:** `feat/spawn-handler`

---

## Working branches

```
feat/programming-assistant      ← shared base branch with Dev A
  feat/trogon-md-all-runners    ← PR 4 extension
  feat/ext-method-scaffold      ← PR 5
  feat/registry-capabilities    ← PR 6
  feat/slash-commands-model     ← PR 9 extension
  feat/system-prompt            ← PR 15
  feat/spawn-handler            ← PR 16
```

Each feature branch makes a PR to `feat/programming-assistant`, not directly to `platform`.

### Coordination with Dev A

PR 4ext, PR 5, and PR 6 are all independent of Dev A's work — start them on Day 1.

PR 9 extension (`/model`) depends on Dev A's PR 8 (CrossRunnerSwitcher). Dev B can
prepare the call site in `repl.rs` against the published API signature, and wire
the actual call once PR 8 lands.

Notify Dev A when PR 6 is merged — they need `find_by_model` for PR 8 integration tests.

### Flow when Dev B depends on something from Dev A

1. Dev A finishes PR 8, merges to `feat/programming-assistant`
2. Dev A notifies Dev B
3. Dev B syncs: `git fetch origin && git merge origin/feat/programming-assistant`
4. Dev B wires `/model` in `repl.rs` and merges PR 9 extension

---

## Files touched

### PR 4 extension
- `trogon-runner-tools/src/trogon_md.rs` — new (moved from acp-runner)
- `trogon-runner-tools/src/lib.rs` — add pub mod trogon_md
- `trogon-acp-runner/src/lib.rs` — replace module with re-export
- `trogon-xai-runner/Cargo.toml` — add trogon-runner-tools dep
- `trogon-xai-runner/src/agent.rs` — call load_trogon_md at prompt time
- `trogon-codex-runner/Cargo.toml` — add trogon-runner-tools dep
- `trogon-codex-runner/src/agent.rs` — add first_turn field, initialize to true
- `trogon-openrouter-runner/Cargo.toml` — add trogon-runner-tools dep
- `trogon-openrouter-runner/src/agent.rs` — call load_trogon_md at prompt time

### PR 5
- `trogon-xai-runner/src/agent.rs` — add session/get_state arm, derive Serialize on XaiSession
- `trogon-codex-runner/src/agent.rs` — add session/get_state arm, derive Serialize on CodexSession
- `trogon-openrouter-runner/src/agent.rs` — add session/get_state arm, derive Serialize on OpenRouterSession

### PR 6
- `trogon-xai-runner/src/main.rs` — expand capabilities, add model_ids to registry metadata
- `trogon-codex-runner/src/main.rs` — expand capabilities, add model_ids to registry metadata
- `trogon-openrouter-runner/src/main.rs` — expand capabilities, add model_ids to registry metadata
- `trogon-acp-runner/src/main.rs` — add model_ids to registry metadata
- `trogon-registry/src/registry.rs` — implement find_by_model (replace Day 0 stub)

### PR 9 extension
- `trogon-cli/src/repl.rs` — add /model arm, CrossRunnerSwitcher instance, mutable prefix/session tracking

### PR 15
- `trogon-xai-runner/src/agent.rs` — read req.meta["systemPrompt"] in new_session
- `trogon-openrouter-runner/src/agent.rs` — read req.meta["systemPrompt"] in new_session

### PR 16
- `trogon-xai-runner/src/main.rs` — clone nats, launch spawn subscriber
- `trogon-xai-runner/src/spawn_handler.rs` — new: oneshot_call
- `trogon-openrouter-runner/src/main.rs` — clone nats, launch spawn subscriber
- `trogon-openrouter-runner/src/spawn_handler.rs` — new: oneshot_call
