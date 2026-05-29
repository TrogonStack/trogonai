# Trogon Terminal — Definitive Plan (v2)

## Full replacement of Claude Code CLI for programming

**Branch:** `programming-cursor` (from `programming-2`)  
**Scope:** Terminal only — `trogon` CLI + NATS runners. No IDE work.  
**Policy:** Local unsigned commits only; never push unless explicitly requested.  
**Review:** Validated against codebase May 2026; incorporates external review (readline/`/dev/tty` permission design, `/compact` payload bug, Stage 0 exit test scope).

---

## 1. Mission

A developer can:

1. Uninstall Claude Code CLI
2. Run one documented dev stack (or `trogon dev`)
3. Open any project directory and run `trogon`
4. Program with read/write/edit/bash/git/search, approve sensitive actions, see tool output, compact context, resume sessions, configure MCP, and **`/model` switch between Claude, Grok, OpenRouter, and Codex mid-session**

---

## 2. Definition of done

All criteria must pass before claiming “drop Claude Code CLI”:

| # | Criterion | How to verify |
|---|-----------|---------------|
| 1 | **One command starts the stack** | `trogon dev` or `./scripts/trogon-dev.sh` → NATS + runners + wasm + compactor |
| 2 | **File tools work** on default runner | `read Cargo.toml`, edit a file, no silent 60s deny |
| 3 | **Bash works with visible output** | `cargo check` runs; stdout/stderr shown in terminal |
| 4 | **Interactive permissions work** | Write/bash prompts: Allow / Always / Reject via `/dev/tty` |
| 5 | **Sessions resume** | Exit REPL → `trogon --continue` → same conversation |
| 6 | **`/compact` compacts** | Token % drops; compactor receives `trogon.compactor.compact` + `{ messages: [...] }` |
| 7 | **MCP stdio from CLI** | Configured MCP server tools callable by agent |
| 8 | **Cross-runner `/model` works** | Switch Claude ↔ Grok ↔ OpenRouter ↔ Codex mid-session |
| 9 | **Target model set after switch** | `/model o4-mini` on Codex actually uses o4-mini, not runner default |
| 10 | **Docs match code** | No stale stub tables; dev keys documented per runner |
| 11 | **`trogon doctor` all green** | NATS+JS, registry, execution, at least one LLM runner |

**Explicitly out of scope for v1:** VS Code/JetBrains plugins, hooks, full sub-agent tool loops, Sprites, multi-agent production deploy, `git commit`, generic `web_search` (Grok native search documented as alternative).

---

## 3. Architecture

### 3.1 Four runners + shared infrastructure

```text
                         ┌─────────────────────────────────┐
                         │  trogon CLI                      │
                         │  • REPL / --print                │
                         │  • CrossRunnerSwitcher (/model)  │
                         │  • MUST ADD: ACP client (perms)  │
                         └───────────────┬─────────────────┘
                                         │ NATS JetStream
     ┌───────────────┬──────────────┬────┴────┬──────────────┐
     ▼               ▼              ▼         ▼              ▼
 acp.claude      acp.grok    acp.openrouter  acp.codex   acp.wasm
 (Anthropic)     (xAI)       (OpenRouter)    (Codex CLI) (execution)
     │               │              │         │              │
     └──────── trogon-tools (13 tools) ────────┘              │
              except Codex → subprocess tools                  │
                                                               │
     ┌─────────────────────────────────────────────────────────┘
     ▼
 trogon-wasm-runtime     bash terminals (stateful on Claude path)
 trogon-compactor       trogon.compactor.compact (summarize history)
 trogon-secret-proxy    optional (vault mode) vs direct API keys (dev)
 AGENT_REGISTRY (KV)    find_by_model() for /model routing
```

### 3.2 Responsibility split (critical)

| Concern | Owner | NATS pattern |
|---------|-------|--------------|
| Agent loop + trogon-tools | Each LLM runner | `{prefix}.session.{id}.agent.*` |
| **Bash terminals** | `trogon-wasm-runtime` | `acp.wasm.session.{id}.client.terminal.*` |
| **Permission prompts** | **`trogon` CLI** (to build) | `{runner_prefix}.session.{id}.client.session.request_permission` |
| **Elicitation / ask_user** | **`trogon` CLI** (to build) | `{runner_prefix}.session.{id}.client.session.elicitation` |
| Auto-compaction | `trogon-compactor` | `trogon.compactor.compact` |
| Cross-runner history | All runners | ext_method `session/export` + `session/import` |

**Do not reimplement PTY/bash in the CLI** — wire wasm-runtime. **Do implement permission/elicitation client in the CLI.**

**Bash failure modes (both must be fixed):**

1. **No wasm-runtime registered/running** → no terminal responder on `acp.wasm` (**Stage 0**)
2. **No permission client on runner prefix** → tool approval times out ~60s (**Stage 1**)

### 3.3 Mid-session model/runner switching (design)

Already implemented in `cross_runner.rs` + `/model` in `repl.rs`:

```text
User: /model <model_id>
  │
  ├─ registry.find_by_model(model_id) → target acp_prefix
  │
  ├─ same prefix? → session.set_model(model_id)     [works today]
  │
  └─ different prefix?  [requires LocalSet — Stage 1]
       1. session/export on current runner → JSON messages
       2. new_session on target runner (same cwd)
       3. session/import with exported history
       4. REPL replaces prefix + session_id
       5. session.set_model(model_id) on new session   [bug today — fix Stage 1]
       6. Rebind ACP client subscription to new prefix   [Stage 1]
```

**Fidelity today:** `PortableMessage { role, text }` only — tool calls, thinking, images dropped on every cross-runner switch. **Stage 5 fixes this partially with v2 export.**

---

## 4. Four-runner reference matrix

| | **acp-runner** | **xai-runner** | **openrouter-runner** | **codex-runner** |
|--|----------------|----------------|----------------------|------------------|
| **Crate** | `trogon-acp-runner` | `trogon-xai-runner` | `trogon-openrouter-runner` | `trogon-codex-runner` |
| **Prefix** | `acp.claude` | `acp.grok` | `acp.openrouter` | `acp.codex` |
| **Auth env** | `ANTHROPIC_TOKEN`, optional `ANTHROPIC_BASE_URL` | `XAI_API_KEY` | `OPENROUTER_API_KEY` | Codex CLI auth + `CODEX_DEFAULT_MODEL` |
| **Tools** | 13 `trogon-tools` + bash + MCP + ask_user* | 13 tools + bash + native `web_search` | 13 tools + bash (toggleable) | Codex subprocess tools |
| **Bash** | Stateful (`WasmRuntimeBashTool`) | Stateless per call | Stateless per call | Codex-native |
| **TROGON.md** | ✅ `trogon-runner-tools/trogon_md.rs` | ✅ | ✅ | ❌ (gap — Stage 5.6) |
| **Permissions** | Full (`RulesPermissionChecker`) | None → Stage 1.4 | None → Stage 1.4 | None → Stage 1.4 |
| **Compactor** | Auto 85% via `trogon.compactor.compact` | `trim_history` only → Stage 5.1 | `trim_history` only → Stage 5.1 | Codex-managed |
| **export/import** | ✅ lossy text | ✅ | ✅ | ✅ (`pending_history` path) |
| **In dev-runner.sh** | ✅ | ✅ | ❌ → **add Stage 0** | ❌ → **add Stage 0** |
| **In docker-compose** | ✅ `trogon-acp-runner` | ✅ `trogon-xai-runner` | ✅ `trogon-openrouter-runner` | ✅ `trogon-codex-runner` |

\* `ask_user` when elicitation channel wired in runner + CLI client.

### trogon-tools surface (all trogon-tools runners)

From `trogon-tools/src/lib.rs` — `all_tool_defs()`:

`read_file`, `write_file`, `list_dir`, `glob`, `str_replace`, `git_status`, `git_diff`, `git_log`, `fetch_url`, `notebook_edit`, `search_files`, `todo_write`, `todo_read`

Plus runner add-ons: `bash` (wasm), MCP tools, `spawn_agent` (not wired in acp production `main.rs`), xAI `web_search` server-side.

### Registry model routing

Each runner registers at startup (`trogon-registry`) with `metadata.models` and `metadata.acp_prefix`.  
`CrossRunnerSwitcher` (`trogon-cli/src/cross_runner.rs`) calls `registry.find_by_model(model_id)`.

OpenRouter models come from `OPENROUTER_MODELS` env (see `trogon-openrouter-runner/src/main.rs`).  
Codex models from `CODEX_MODELS` env (see `trogon-codex-runner/src/main.rs`).

### CLI model aliases (extend in Stage 0)

Current (`repl.rs:437–445`):

```rust
"haiku"      => "claude-haiku-4-5-20251001",
"sonnet"     => "claude-sonnet-4-6",
"opus"       => "claude-opus-4-7",
"grok"       => "grok-3",
"grok-mini"  => "grok-3-mini",
other        => passthrough full model id
```

**Add in Stage 0.5:**

```rust
"openrouter" => OPENROUTER_DEFAULT_MODEL from config/env
"codex"      => CODEX_DEFAULT_MODEL (e.g. "o4-mini")
"o3"         => "o3"
"gpt-4o"     => "gpt-4o"
```

---

## 5. Current state & confirmed bugs

```text
Backend (4 runners + trogon-tools)     ████████████░░  ~88%
Cross-runner /model mechanism          ██████████░░░░  ~75%  (exists, needs LocalSet)
Terminal product (trogon CLI)          ██████░░░░░░░░  ~50%
Dev experience                         ████░░░░░░░░░░  ~30%
Drop-in readiness                      ██████░░░░░░░░  ~55%
```

### Works today

- REPL, `--print`, JSON, NATS autostart, @mentions, 7 slash commands
- 13 programming tools; Claude tools wired (`ac55970fa` — was hardcoded `vec![]` before)
- Cross-runner export/import + `/model` mechanism (`cross_runner.rs`)
- OpenRouter + Codex: export/import in code, compose-ready
- Markdown render, tool-start indicator (`8b04c3394`), error surfacing (`b01b27a63`), token footer
- xAI Grok 4.3 tool dispatch fix (`92e280535`)

### Confirmed bugs (all verified in code)

| # | Bug | Location | Symptom |
|---|-----|----------|---------|
| B1 | **No ACP client** | `trogon-cli` — no `client::run`, no subscriber on `{prefix}.session.*.client.*` | Permission requests timeout ~60s → deny |
| B2 | **NATS autostart without JetStream** | `lib.rs:43` — `Command::new("nats-server").args(["-p", "4222"])` | Registry/KV fails on autostart |
| B3a | **`/compact` wrong subject** | `session.rs:328` — `format!("{}.compactor.compact", self.prefix)` | Publishes to `acp.claude.compactor.compact`; service listens `trogon.compactor.compact` |
| B3b | **`/compact` wrong payload** | `session.rs:332` — `{ "sessionId": "..." }` only | Compactor expects `{ "messages": [...] }` per `trogon-compactor/src/service.rs:46–48` |
| B4 | **Cross-runner no `set_model`** | `repl.rs:197–204` — attach new session but never `set_model` | Target runner uses `AGENT_MODEL` default |
| B5 | **No `LocalSet`** | `main.rs` — plain `#[tokio::main]` | `Bridge` is `!Send` (`CLAUDE.md`); cross-prefix `/model` may panic |
| B6 | **`ToolCallUpdate` ignored** | `session.rs:239` — `_ => {}` | No bash/tool output; runner sends via `prompt_converter.rs:197+` |
| B7 | **`render_diff` wrong names** | `session.rs::render_diff` — `Edit`/`Write`/`MultiEdit` only | Runners use `str_replace`, `write_file`, etc. |
| B8 | **dev-runner incomplete** | `dev-runner.sh` — acp + xai only | No wasm, compactor, openrouter, codex |
| B9 | **MCP bridge unwired** | `stdio_mcp_bridge.rs` exists; not in `main.rs`/`repl.rs` | MCP only in `trogon-acp` IDE path |
| B10 | **No session resume** | CLI always `TrogonSession::new` | No `--continue`, `/sessions`, `/resume` |
| B11 | **Permission modes cosmetic** | `trogon-acp-runner/src/agent.rs:475` — only `bypassPermissions` skips checker | `acceptEdits`/`plan`/`dontAsk` stored but not enforced |
| B12 | **No permissions on xai/OR/codex** | `trogon-xai-runner/src/agent.rs` — no `RulesPermissionChecker` | Ungated tool exec on non-Claude runners |
| B13 | **Lossy export** | `portable_session.rs` — `{ role, text }` only | Tool/thinking blocks dropped on cross-runner switch |

---

## 6. The plan — six stages (+ design gate)

---

### STAGE 0 — Runnable four-runner stack

**Duration:** 2–3 days  
**Goal:** `trogon doctor` green; same-prefix model switch; bash works with wasm in stack

#### 0.1 — Unified dev launcher

**Deliverable:** `rsworkspace/scripts/trogon-dev.sh` and/or `trogon dev` subcommand in `main.rs`

**Full script:**

```bash
#!/usr/bin/env bash
# rsworkspace/scripts/trogon-dev.sh
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RSWORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
ENV_FILE="$RSWORKSPACE/.env.local"

if [ ! -f "$ENV_FILE" ]; then
  echo "Missing $ENV_FILE — copy from .env.local.example"
  exit 1
fi

set -a; source "$ENV_FILE"; set +a

BIN="$RSWORKSPACE/target/release"
LOG_DIR="${TROGON_LOG_DIR:-/tmp}"

# 1. NATS + JetStream (skip if port already open)
if ! nc -z localhost 4222 2>/dev/null; then
  echo "Starting nats-server -js"
  nats-server -p 4222 -js > "$LOG_DIR/trogon-nats.log" 2>&1 &
fi

# 2. Execution backend — REQUIRED for bash
echo "Starting trogon-wasm-runtime (prefix=acp.wasm)"
WASM_ONLY=0 \
WASM_AUTO_ALLOW_PERMISSIONS=1 \
ACP_PREFIX=acp.wasm \
AGENT_TYPE=wasm \
NATS_URL="${NATS_URL:-nats://localhost:4222}" \
  "$BIN/trogon-wasm-runtime" > "$LOG_DIR/trogon-wasm.log" 2>&1 &

# 3. Compactor — REQUIRED for long sessions + /compact (Stage 2 fixes CLI wiring)
if [ -n "$ANTHROPIC_TOKEN" ]; then
  echo "Starting trogon-compactor"
  PROXY_URL="${PROXY_URL:-http://localhost:8080}" \
  ANTHROPIC_TOKEN="$ANTHROPIC_TOKEN" \
  NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-compactor" > "$LOG_DIR/trogon-compactor.log" 2>&1 &
fi

# 4. LLM runners — start only when credentials present
if [ -n "$ANTHROPIC_TOKEN" ]; then
  echo "Starting trogon-acp-runner (prefix=acp.claude)"
  ACP_PREFIX=acp.claude NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-acp-runner" > "$LOG_DIR/trogon-acp.log" 2>&1 &
fi

if [ -n "$XAI_API_KEY" ]; then
  echo "Starting trogon-xai-runner (prefix=acp.grok)"
  ACP_PREFIX=acp.grok XAI_API_KEY="$XAI_API_KEY" NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-xai-runner" > "$LOG_DIR/trogon-xai.log" 2>&1 &
fi

if [ -n "$OPENROUTER_API_KEY" ]; then
  echo "Starting trogon-openrouter-runner (prefix=acp.openrouter)"
  ACP_PREFIX=acp.openrouter OPENROUTER_API_KEY="$OPENROUTER_API_KEY" \
    NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-openrouter-runner" > "$LOG_DIR/trogon-openrouter.log" 2>&1 &
fi

if [ "${CODEX_ENABLED:-0}" = "1" ]; then
  echo "Starting trogon-codex-runner (prefix=acp.codex)"
  ACP_PREFIX=acp.codex NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-codex-runner" > "$LOG_DIR/trogon-codex.log" 2>&1 &
fi

echo ""
echo "Trogon dev stack started. Logs: $LOG_DIR/trogon-*.log"
echo "Run: $BIN/trogon --doctor"
echo "Then: cd /your/project && $BIN/trogon"
```

**Build line (update error messages):**

```bash
cargo build --release -p trogon-cli -p trogon-acp-runner -p trogon-xai-runner \
  -p trogon-openrouter-runner -p trogon-codex-runner -p trogon-wasm-runtime -p trogon-compactor
```

**Files to change:**

| File | Change |
|------|--------|
| `rsworkspace/scripts/trogon-dev.sh` | Create (above) |
| `rsworkspace/dev-runner.sh` | Replace body with `exec "$(dirname "$0")/scripts/trogon-dev.sh"` or deprecate |
| `rsworkspace/crates/trogon-cli/src/lib.rs:43` | `args(["-p", "4222", "-js"])` |
| `rsworkspace/crates/trogon-cli/src/main.rs` | Optional: `trogon dev` subcommand wrapping script |

#### 0.2 — `.env.local.example`

**File:** `rsworkspace/.env.local.example`

```bash
NATS_URL=nats://localhost:4222

# ── Claude (acp.claude) ──────────────────────────────────────
ANTHROPIC_TOKEN=
ANTHROPIC_BASE_URL=https://api.anthropic.com/v1   # direct dev; omit for proxy mode

# ── Grok (acp.grok) ──────────────────────────────────────────
XAI_API_KEY=

# ── OpenRouter (acp.openrouter) ──────────────────────────────
OPENROUTER_API_KEY=
OPENROUTER_DEFAULT_MODEL=anthropic/claude-sonnet-4
OPENROUTER_MODELS=anthropic/claude-sonnet-4,openai/gpt-4o,google/gemini-2.5-pro

# ── Codex (acp.codex) — requires Codex CLI on PATH ─────────
CODEX_ENABLED=0
CODEX_DEFAULT_MODEL=o4-mini
CODEX_MODELS=o4-mini:o4-mini,o3:o3,gpt-4o:GPT-4o

# ── Proxy / vault mode (optional — docker-compose) ───────────
# PROXY_URL=http://localhost:8080
# ANTHROPIC_TOKEN=tok_anthropic_prod_...

# ── Dev permission bridge until Stage 1 interactive UI ─────
TROGON_MODE=acceptEdits
# TROGON_MODE=bypassPermissions   # fully trusted dev only

# ── Default CLI runner ───────────────────────────────────────
ACP_PREFIX=acp.claude
```

#### 0.3 — `trogon doctor`

**Create:** `rsworkspace/crates/trogon-cli/src/doctor.rs`  
**Wire:** `main.rs` — flag `--doctor` or subcommand `trogon doctor`

| # | Check | Pass condition | Fail message hint |
|---|-------|----------------|-------------------|
| 1 | NATS TCP | Connect to `NATS_URL` | Start `nats-server -p 4222 -js` |
| 2 | JetStream KV | Put/get test key in temp bucket | NATS missing `-js` |
| 3 | Registry `acp.claude` | Entry if `ANTHROPIC_TOKEN` set | Start `trogon-acp-runner` |
| 4 | Registry `acp.grok` | Entry if `XAI_API_KEY` set | Start `trogon-xai-runner` |
| 5 | Registry `acp.openrouter` | Entry if `OPENROUTER_API_KEY` set | Start `trogon-openrouter-runner` |
| 6 | Registry `acp.codex` | Entry if `CODEX_ENABLED=1` | Start `trogon-codex-runner` |
| 7 | Registry `execution` | wasm-runtime with capability `execution` | Start `trogon-wasm-runtime` with `WASM_ONLY=0` |
| 8 | Session smoke | `{ACP_PREFIX}.agent.session.new` → OK within 15s | Runner down or wrong prefix |
| 9 | Compactor | Request-reply `trogon.compactor.compact` with minimal payload | Start `trogon-compactor` |
| 10 | Codex CLI | `codex --version` or equivalent if `CODEX_ENABLED=1` | Install Codex CLI |
| 11 | Token non-empty | Warn if configured runner has empty API key | Fill `.env.local` |

**Files:** `doctor.rs`, `lib.rs` (`pub mod doctor`), `main.rs`

#### 0.4 — Default permission mode (bridge until Stage 1)

**File:** `rsworkspace/crates/trogon-cli/src/session.rs` — in `TrogonSession::new` after session created:

```rust
let mode = std::env::var("TROGON_MODE").unwrap_or_else(|_| "acceptEdits".into());
let _ = session.set_mode(&mode).await; // log warn on error, don't fail new
```

**Also:** document permissive `TROGON.md` example in `docs/terminal-agent-definitive-plan.md` or dev guide:

```markdown
## Permissions
allow_paths: **
allow_commands: *
```

#### 0.5 — Extend model aliases

**File:** `rsworkspace/crates/trogon-cli/src/repl.rs` — `resolve_model_alias()`

Add aliases from section 4. When `/model` with no args: read `~/.config/trogon/config.json` **and** show note that live model may differ (full fix in Stage 2/5).

#### 0.6 — Documentation truth

| File | Action |
|------|--------|
| `rsworkspace/CLAUDE.md` | Commit; add four-runner dev flow, `trogon-dev.sh`, doctor |
| `rsworkspace/.env.local.example` | Commit (section 0.2) |
| `docs/terminal-agent-definitive-plan.md` | This document |
| `rsworkspace/docs/cli-status.md` | Rewrite — mark PRs 6–9 done, remove stub table |
| `TROGON.md` (root) | Fix placeholder or point to `rsworkspace/CLAUDE.md` |
| `docs/programming.md` | Commit if still accurate; fix xai/OR "chat-only" gap table |

#### Stage 0 exit test (NO cross-runner switch)

```bash
cd rsworkspace && cargo build --release -p trogon-cli -p trogon-acp-runner \
  -p trogon-wasm-runtime -p trogon-compactor
cp .env.local.example .env.local   # fill ANTHROPIC_TOKEN
./scripts/trogon-dev.sh
./target/release/trogon --doctor   # green for configured services

cd /your/rust/project
./target/release/trogon --prefix acp.claude
> read Cargo.toml and list dependencies     # file tools (B1 mitigated by TROGON_MODE)
> run `echo hello` in bash                  # wasm-runtime (fixes B8 partial)
/model sonnet                               # same prefix — set_model
/model opus                                 # same prefix — set_model
# DO NOT /model grok here — requires LocalSet (Stage 1, B5)
```

---

### STAGE 1 — ACP client layer (permissions + elicitation)

**Duration:** 8–10 days (includes readline/permission design + integration tests)  
**Goal:** Interactive trust on all trogon-tools runners; cross-runner `/model` safe

#### 1.0 — Design gate (REQUIRED BEFORE PR 4)

**Create:** `docs/permission-ui-design.md`

**Problem:** REPL uses sync `rl.readline("> ")` (`repl.rs:162`) between turns. `acp_nats::client::run` runs in `spawn_local` and calls `TuiClient::request_permission` asynchronously. The shared `rustyline::Editor` cannot be used from the permission handler — readline owns the terminal.

**When permissions fire:** During **active turns** (inner `tokio::select!` at `repl.rs:322+`), while tools execute. Not while idle at `> `. Handler still runs in a **separate task** from REPL.

**Rejected options:**

| Option | Why not v1 |
|--------|------------|
| Use rustyline for permission prompt | Editor blocked or re-entrant unsafe |
| Channel to REPL inner loop only | Still need raw tty read; doesn't help idle edge case |
| Auto-deny and rely on TROGON.md only | Not Claude Code parity |

**Chosen: Option A — `/dev/tty` in `TuiClient`**

```text
acp_nats::client::run (spawn_local)
  └─ TuiClient::request_permission
       ├─ eprintln! to stderr: tool name, path/command snippet
       ├─ eprintln! "[a]llow  [A]lways  [r]eject"
       └─ read single key from /dev/tty (crossterm or raw termios)
       └─ return RequestPermissionResponse to NATS reply subject
```

**Elicitation:** same — short prompts on `/dev/tty`, line-based for multi-field forms.

**Non-interactive `--print`:** `--dangerously-skip-permissions` auto-allows (Stage 4); until then use `TROGON_MODE=bypassPermissions`.

**Future Stage 6:** Option C — `tokio-rustyline` or cancelable `spawn_blocking` for whole REPL.

**Reference implementations:**

- Permission outcome parsing: `trogon-acp/src/main.rs:305–400`
- NATS client loop: `acp-nats-stdio/src/main.rs:96–111`
- Runner permission bridge: `trogon-acp-runner/src/permission_bridge.rs`

#### 1.1 — `LocalSet` runtime bootstrap

**Create:** `rsworkspace/crates/trogon-cli/src/runtime.rs`

```rust
pub async fn run_interactive<SF, F, SW>(...) -> anyhow::Result<()>
where
    SF: SessionFactory,
    F: Fs,
    SW: RunnerSwitcher,
{
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        let client_handle = tokio::task::spawn_local(run_acp_client(...));
        let repl_result = repl::run(factory, prefix, cwd, fs, switcher).await;
        client_handle.abort();
        repl_result
    }).await
}
```

**Change:** `main.rs` — interactive branch calls `runtime::run_interactive` instead of `repl::run` directly.

**Reference:** `Bridge` is `!Send` — `cross_runner.rs:40`, `CLAUDE.md`.

#### 1.2 — `TuiClient`

**Create:** `rsworkspace/crates/trogon-cli/src/tui_client.rs`

| ACP method | Implement? | Notes |
|------------|--------------|-------|
| `request_permission` | ✅ | `/dev/tty` (1.0) |
| `request_elicitation` / `ElicitationClient` | ✅ | `/dev/tty` forms |
| `ExitPlanMode` special case | ✅ | Port `trogon-acp/src/main.rs:329–377` |
| `terminal/create`, `terminal/output`, etc. | ❌ | wasm-runtime owns bash |
| `read_text_file`, `write_text_file` | ❌ v1 | Runner uses trogon-tools in-process |

**Wire in `run_acp_client`:**

```rust
use acp_nats::client;
let bridge = Rc::new(Bridge::new(nats.clone(), js_client, SystemClock, &meter, config.clone(), notif_tx));
let client = Rc::new(TuiClient::new(active_session.clone(), active_prefix.clone()));
client::run(nats, client, bridge, StdJsonSerialize).await;
```

**State tracked in `Arc<Mutex<ActiveClientState>>`:**

```rust
struct ActiveClientState {
    session_id: Option<String>,
    prefix: String,
    allowed_tools: Vec<String>,  // for allow_always persistence hint
}
```

Updated from `repl.rs` on: session create, `/clear`, cross-runner `/model`, exit.

**Files:** `tui_client.rs`, `runtime.rs`, `lib.rs`, `main.rs`

#### 1.3 — Cross-runner fixes

| # | File | Fix |
|---|------|-----|
| 1 | `main.rs`, `runtime.rs` | All `CrossRunnerSwitcher` use inside `LocalSet` |
| 2 | `repl.rs:197–204` | After cross-prefix switch: `session.set_model(&model_id).await?` |
| 3 | `runtime.rs` | On prefix change, update `ActiveClientState.prefix` and ensure client subscription covers new prefix (may require restart client task or wildcard — document choice in permission-ui-design.md) |
| 4 | `repl.rs:189` | Use `cwd.canonicalize()` for cross-runner migration path |

#### 1.4 — Permission mode semantics (all trogon-tools runners)

**Reference:** `trogon-acp-runner/src/agent.rs:475` — today only `bypassPermissions` skips checker.

**Implement mode → policy in each runner:**

| Runner | File to change |
|--------|------------------|
| acp | `trogon-acp-runner/src/agent.rs` |
| xai | `trogon-xai-runner/src/agent.rs` |
| openrouter | `trogon-openrouter-runner/src/agent.rs` |
| codex | `trogon-codex-runner/src/agent.rs` |

| Mode | Checker behavior |
|------|------------------|
| `default` | `RulesPermissionChecker` → TROGON.md rules → interactive channel |
| `acceptEdits` | Auto-allow `write_file`, `str_replace`, `notebook_edit`; prompt `bash`, MCP |
| `dontAsk` | Auto-allow all (log audit) |
| `plan` | Deny write/bash tools until `ExitPlanMode` permission flow |
| `bypassPermissions` | No checker installed |

**Also:** expose `bypassPermissions` in `session_mode_state` (`trogon-acp-runner/src/agent.rs:357–366` currently omits it).

**Wire permission channel on xai/openrouter/codex** same as acp-runner `main.rs:170–197`:

```rust
let (perm_tx, mut perm_rx) = mpsc::channel::<PermissionReq>(32);
// spawn_local drain → handle_permission_request_nats
```

#### 1.5 — CLI `/mode` slash command

**File:** `repl.rs` — add to slash handler:

```
/mode [default|acceptEdits|plan|dontAsk|bypassPermissions]
```

Calls `session.set_mode()`. Show current mode when no arg (after Stage 2, also show runner-reported mode via ext_method).

#### Stage 1 exit test (includes cross-runner)

```bash
./scripts/trogon-dev.sh   # all four runners if keys set
trogon
TROGON_MODE=default
> create test.txt with content hello
# → /dev/tty: [a]llow [A]lways [r]eject — no 60s hang (fixes B1)

/model grok
# → export/import + set_model (fixes B4, B5)
> edit test.txt to say goodbye
# → permission on xai (fixes B12 partial)

/model anthropic/claude-sonnet-4    # OpenRouter
/model o4-mini                        # Codex
/model sonnet                         # back to Claude
```

---

### STAGE 2 — Tool feedback, streaming, `/compact` fix

**Duration:** 3–4 days  
**Goal:** Terminal shows agent work; manual compact works

#### 2.1 — `ToolCallUpdate` → `StreamEvent`

**File:** `rsworkspace/crates/trogon-cli/src/session.rs`

Extend enum:

```rust
pub enum StreamEvent {
    Text(String),
    Thinking,
    ToolCall(String),
    Diff(String),
    ToolFinished {
        name: String,
        output: String,
        exit_code: Option<i32>,
        status: ToolCallStatus,  // from agent_client_protocol
    },
    ModeUpdate { mode: String },
    ConfigUpdate { /* ... */ },
    PlanUpdate { entries: Vec<String> },
    Usage { used_tokens: u64, context_size: u64 },
    Error(String),
    Done(String),
}
```

In prompt notification match (`session.rs:212–239`), replace `_ => {}` with:

```rust
SessionUpdate::ToolCallUpdate(update) => {
    if let Some(ev) = map_tool_call_update(&update) {
        // emit StreamEvent::ToolFinished (or skip terminal_output-only updates)
    }
}
SessionUpdate::CurrentModeUpdate(m) => { ... }
SessionUpdate::ConfigOptionUpdate(c) => { ... }
SessionUpdate::Plan(p) => { ... }
```

**File:** `repl.rs` — on `ToolFinished`: print truncated output (2048 chars), exit code when known, `[failed]` badge when `status == Failed`.

**Reference:** `trogon-acp-runner/src/prompt_converter.rs:169–213`

##### ⚠ PR 7 implementation note — no `exit_code` on the wire

`StreamEvent::ToolFinished` carries `exit_code: Option<i32>`, but **`ToolCallUpdateFields` in `agent-client-protocol` has no `exit_code` field** — only `status: ToolCallStatus`, `content: Vec<ToolCallContent>`, `raw_output`, `locations`, etc. Runners encode success/failure in `status`; exit code must be **derived** in the CLI mapper.

Add a helper (e.g. `map_tool_call_update` in `session.rs` or `tool_update.rs`):

| Field on `StreamEvent::ToolFinished` | Source (priority order) |
|--------------------------------------|-------------------------|
| `status` | `update.fields.status` directly |
| `output` | 1) `raw_output` if JSON string; 2) else join `Text` blocks from `content`; 3) for `Diff` blocks use one-line path summary (diff body already handled by `StreamEvent::Diff` on `ToolCall` start) |
| `name` | `update.meta` tool name if present; else match `tool_call_id` against pending `ToolCall` cache from `SessionUpdate::ToolCall` |
| `exit_code` | See extraction table below |

**Exit code extraction** (first match wins):

| Priority | Source | Notes |
|----------|--------|-------|
| 1 | `update.meta["terminal_exit"]["exit_code"]` | IDE path: `trogon-acp/src/agent.rs:387–408` sends a second update with `terminal_exit` meta |
| 2 | Parse `__EXIT_(\d+)__` from `raw_output` | Wasm bash demarcation: `trogon-runner-tools/src/wasm_bash_tool.rs:17–18`. Use **last** numeric marker (same rule as `extract_before_marker`). Strip marker from displayed `output`. |
| 3 | Infer from `status` | `Completed` → `Some(0)`; `Failed` → `None` (badge from status, not code) |

**Skip non-terminal updates:** `trogon-acp` may emit two `ToolCallUpdate`s per bash call — first with `meta["terminal_output"]` only (no final status). Ignore until `status` is `Completed` or `Failed`, or until `terminal_exit` meta arrives.

**Runner variance (expected today):**

| Runner | What arrives on completed bash |
|--------|--------------------------------|
| `acp-runner` | `status` from internal `exit_code`; `raw_output` = stdout/stderr string (marker usually stripped by `WasmRuntimeBashTool` before LLM sees it) |
| xai / openrouter / codex | `status(Completed)` + `raw_output(String(result))` only — no exit code on wire; infer `Some(0)` when Completed |
| IDE replay (`trogon-acp`) | `terminal_exit` meta with explicit `exit_code` |

**PR 7 acceptance:** add unit tests for the mapper covering (a) `terminal_exit` meta, (b) `__EXIT_1__` in raw_output, (c) Completed with no code → `Some(0)`, (d) Failed → `[failed]` badge with `exit_code: None`.

#### 2.2 — Fix diff rendering (B7)

**File:** `session.rs::render_diff`

Add match arms:

| Tool name | Input fields | Render |
|-----------|--------------|--------|
| `str_replace` | `path`, `old_str`, `new_str` | unified diff ±3 lines |
| `write_file` | `path`, `content` | new file preview |
| `read_file` | `path` | one-line summary |
| `bash` | `command` | one-line summary |
| `Edit`/`Write`/`MultiEdit` | (keep existing) | ACP compat |

#### 2.3 — Display modes

**Files:** `main.rs`, `repl.rs`

| Mode | Flag | Behavior |
|------|------|----------|
| Rendered | default | Buffer text in `response_buf`; markdown at tool boundary / Done (`7a5201620`) |
| Stream | `--stream` | `print!` + `stdout.flush()` on each `StreamEvent::Text` |

#### 2.4 — Thinking toggle

**File:** `repl.rs`

`/config show_thinking true` or env `TROGON_SHOW_THINKING=1` → on `StreamEvent::Thinking`, print dim `*thinking...*` on stderr (extend to show content when runner sends it).

#### 2.5 — Fix `/compact` (B3a + B3b)

**Today (`session.rs:327–336`):**

```rust
let subject = format!("{}.compactor.compact", self.prefix);  // WRONG (B3a)
let payload = json!({ "sessionId": session_id });             // WRONG (B3b)
nats.publish_bytes(subject, ...);  // fire-and-forget — also wrong pattern
```

**Correct compactor API (`trogon-compactor/src/service.rs`):**

- Subject: `trogon.compactor.compact` (constant `COMPACT_SUBJECT`)
- Request: `{ "messages": Vec<Message> }`
- Response: `{ "messages", "compacted", "tokens_before", "tokens_after" }`

**Fix procedure — new `TrogonSession::compact`:**

1. `ext_method("session/export", { sessionId })` → messages JSON
2. Parse to `CompactRequest { messages }`
3. `nats.request("trogon.compactor.compact", payload).await` (request-reply, 120s timeout)
4. Parse `CompactResponse`
5. If `compacted`: `ext_method("session/import", { sessionId, messages: compacted })` or runner-specific replace API
6. Return stats to REPL for `/compact` user feedback

**Also fix:** `set_model` / `set_mode` should parse error JSON like prompt path (`session.rs:188–197` pattern).

#### Stage 2 exit test

```bash
trogon --stream
> run cargo test -p trogon-cli 2>&1 | head -20
# → ToolFinished shows bash output (fixes B6)

> edit a file with str_replace
# → colored diff (fixes B7)

/compact
# → "compacted: N tokens before → M after" (fixes B3a, B3b)
```

---

### STAGE 3 — Session lifecycle

**Duration:** 3–4 days  
**Goal:** Resume sessions on any runner

#### 3.1 — Session index

**Create:** `rsworkspace/crates/trogon-cli/src/session_store.rs`

**Path:** `~/.local/share/trogon/sessions.json`

```json
{
  "/abs/path/to/project": {
    "prefix": "acp.claude",
    "session_id": "550e8400-e29b-41d4-a716-446655440000",
    "model": "claude-sonnet-4-6",
    "updated_at": "2026-05-22T18:30:00Z"
  }
}
```

**Optional extended index** (for resume after cross-runner switch):

```json
{
  "/abs/path/to/project": {
    "last": { "prefix": "acp.grok", "session_id": "...", "model": "grok-3" },
    "by_prefix": {
      "acp.claude": { "session_id": "...", "model": "claude-sonnet-4-6" }
    }
  }
}
```

**Update index:** after each successful turn; on `/model` switch; on `/clear` (new session).

#### 3.2 — Extend `Session` trait

**File:** `session.rs`

```rust
async fn load_session(&self, session_id: &str, cwd: &Path) -> Result<()>;
async fn list_sessions(&self) -> Result<Vec<SessionSummary>>;
```

Implement via NATS agent methods (match runner APIs in `trogon-acp-runner/src/agent.rs:869`, `:1005`).

#### 3.3 — CLI surface

| Surface | Files | Behavior |
|---------|-------|----------|
| `trogon --continue` | `main.rs` | Load index for `cwd.canonicalize()`; `load_session`; attach |
| `trogon --session-id <id> --prefix <p>` | `main.rs` | Skip index; direct attach |
| `/sessions` | `repl.rs` | Table: id, prefix, model, updated |
| `/resume <id>` | `repl.rs` | `load_session` + replace REPL session |
| `/clear` | `repl.rs` (existing) | Close + new session; update index |

#### 3.4 — Cross-runner resume semantics (document in CLAUDE.md)

- `--continue` → last session for project (any prefix)
- `/model` → always creates new session on target runner (by design)
- To resume Claude after switching to Grok: `/model sonnet` then `/resume <claude-session-id>` or extended index `by_prefix`

#### Stage 3 exit test

```bash
trogon
> implement feature X across 3 turns
# Ctrl+D
trogon --continue
> what were we doing?

/model grok
> continue in Grok
# Ctrl+D
trogon --continue   # resumes Grok session
```

---

### STAGE 4 — MCP + print mode hardening

**Duration:** 3–4 days

#### 4.1 — MCP config + lifecycle

**Create:** `rsworkspace/crates/trogon-cli/src/mcp.rs`

**Config:** `~/.config/trogon/mcp.json`

```json
{
  "servers": [
    {
      "name": "github",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_..." }
    },
    {
      "name": "my-tool",
      "command": "node",
      "args": ["/path/to/server.js"],
      "env": {}
    }
  ]
}
```

**Lifecycle:**

1. On REPL start: load `mcp.json`
2. On `session.new`: for each server, `StdioMcpBridge::spawn()` → local HTTP URL
3. Pass URLs in `NewSessionRequest` as `StoredMcpServer::Http { url }` (mirror `trogon-acp/src/agent.rs:537`, `build_session_mcp`)
4. On `/clear`, session close, REPL exit: `bridge.shutdown()` kill child + release port
5. Maintain `HashMap<session_id, Vec<StdioMcpBridge>>` if multiple sessions per process

**Slash commands (`repl.rs`):**

| Command | Action |
|---------|--------|
| `/mcp list` | Show configured + active bridges |
| `/mcp add <name> <command...>` | Append to config + hot-start if session active |
| `/mcp remove <name>` | Stop bridge + remove from config |

**Files:** `mcp.rs`, `main.rs`, `session.rs`, `repl.rs`, existing `stdio_mcp_bridge.rs`

#### 4.2 — `--print` for agentic CI

**Files:** `print.rs`, `main.rs`

| Flag | Behavior |
|------|----------|
| `--print-tools` | Emit NDJSON lines: `{"type":"tool","name":"...","output":"..."}` |
| `--dangerously-skip-permissions` | Set `bypassPermissions` on session; document security warning |
| `--model <id>` | Resolve via registry; support cross-runner for single-shot |
| `--prefix <acp_prefix>` | Skip default `ACP_PREFIX` |
| Exit `0` | Success `end_turn` |
| Exit `1` | Error / cancelled |
| Exit `2` | `maxTurnRequests` |

**Also:** parse `set_model`/`set_mode` errors in print path; include tool events when `--print-tools`.

#### Stage 4 exit test

```bash
trogon
/mcp add github "npx -y @modelcontextprotocol/server-github"
> use github MCP to list my open PRs

trogon --print --dangerously-skip-permissions --model grok-3 \
  "run cargo clippy -p trogon-cli and summarize warnings"
echo $?
```

---

### STAGE 5 — Four-runner parity + cross-switch fidelity

**Duration:** 5–7 days

#### 5.1 — Compactor on xAI + OpenRouter

**Create:** `rsworkspace/crates/trogon-runner-tools/src/compaction.rs`

Extract from `trogon-acp-runner/src/agent.rs` (auto-compact at 85%):

```rust
pub async fn maybe_compact(
    nats: &Client,
    messages: &[Message],
    token_budget: usize,
    threshold_pct: u8,  // default 85
) -> Result<Option<Vec<Message>>, CompactError>
```

**Wire in before each prompt:**

| Runner | File |
|--------|------|
| xai | `trogon-xai-runner/src/agent.rs` — replace primary `trim_history` trigger |
| openrouter | `trogon-openrouter-runner/src/agent.rs` |

Keep `trim_history` as fallback if compactor unreachable.

**Env:** `TOKEN_BUDGET=200000`, `COMPACT_THRESHOLD_PCT=85`

#### 5.2 — Verify permissions (Stage 1.4) end-to-end

Run Stage 1 exit test with `TROGON_MODE=default` on each runner after `/model` switch.

#### 5.3 — `PortableMessageV2`

**File:** `rsworkspace/crates/trogon-runner-tools/src/portable_session.rs`

```rust
#[derive(Serialize, Deserialize)]
pub struct PortableMessageV2 {
    pub version: u32,  // 2
    pub role: String,
    pub blocks: Vec<PortableBlock>,
}

#[derive(Serialize, Deserialize)]
pub enum PortableBlock {
    Text(String),
    ToolUse { id: String, name: String, input_summary: String },
    ToolResult { id: String, output_summary: String },
    Thinking { text: String },
}
```

**Update export/import:**

| Runner | File | Notes |
|--------|------|-------|
| acp | `trogon-acp-runner/src/agent.rs` | Include tool blocks in export |
| xai | `trogon-xai-runner/src/agent.rs` | |
| openrouter | `trogon-openrouter-runner/src/agent.rs` | |
| codex | `trogon-codex-runner/src/agent.rs` | Validate `pending_history` + `first_turn` absorption |

**CLI:** `cross_runner.rs` — detect `version: 2` in export JSON; fallback to v1 `{role,text}[]`.

#### 5.4 — Cross-switch metadata

**File:** `cross_runner.rs` — after import:

```rust
session.set_model(model_id).await?;
session.set_mode(prior_mode).await?;  // if stored in index
// ext_method to restore enabled_tools on openrouter/xai if supported
```

#### 5.5 — Bash semantics (document + optional unify)

| Runner | Bash impl | Stateful? | After `/model` switch |
|--------|-----------|-----------|------------------------|
| Claude | `WasmRuntimeBashTool` | Yes (`terminal_id` in KV) | Lost on prefix change |
| xAI | `execute_bash_via_nats` | No | N/A |
| OpenRouter | same as xAI | No | N/A |
| Codex | Codex CLI | Codex-managed | N/A |

Document in `/help` and CLAUDE.md: **`cd` / env do not survive cross-runner switch.**

**Optional Stage 6:** route xai/OR through `WasmRuntimeBashTool` for stateful parity.

#### 5.6 — TROGON.md on Codex

**File:** `trogon-codex-runner/src/agent.rs`

On `first_turn` or session create:

```rust
let trogon_md = trogon_runner_tools::trogon_md::load_trogon_md(&cwd).await;
// prepend to system prompt / pending_history
```

Mirror `trogon-xai-runner/src/agent.rs:1026–1027`.

#### 5.7 — `/model` without args — registry listing

**File:** `repl.rs`

Query registry for all agents; group output:

```text
Claude (acp.claude):
  sonnet → claude-sonnet-4-6
  opus   → claude-opus-4-7
  haiku  → claude-haiku-4-5-20251001
Grok (acp.grok):
  grok       → grok-3
  grok-mini  → grok-3-mini
OpenRouter (acp.openrouter):
  anthropic/claude-sonnet-4
  openai/gpt-4o
  ...
Codex (acp.codex):
  o4-mini
  o3
  gpt-4o

Current session: claude-sonnet-4-6 on acp.claude (abc-123)
```

#### Stage 5 exit test

Full cross-runner torture test from Stage 1 plus:

- Long Grok session → verify compactor fires (not blind trim at 20 messages)
- Export v2: tool summaries survive Claude → OpenRouter switch
- Codex round-trip: Claude → Codex → Claude with intelligible context

---

### STAGE 6 — Polish (ongoing, 1–2 weeks)

| Item | Files | Priority |
|------|-------|----------|
| `/doctor` in `/help` | `repl.rs` | High |
| `/status` — prefix, model, tokens, runners up | `repl.rs` + doctor checks | High |
| `/memory` — edit TROGON.md hierarchy | `repl.rs`, `fs.rs` | Medium |
| Wire `spawn_agent` in acp production | `trogon-acp-runner/src/main.rs:151`, `agent.rs` | Low |
| `git commit` tool | `trogon-tools/src/git.rs` | Medium |
| `web_search` in trogon-tools OR doc Grok-only | `trogon-tools` or docs | Low |
| Unified stateful bash on xai/OR | runner agents | Low |
| cross_runner tests: `#[ignore]` without Docker or in-process NATS mock | `cross_runner.rs` tests | Medium |
| Async readline (Option C) | `repl.rs` | Low |
| Hooks (`PreToolUse`, etc.) | design doc only | Defer |

---

## 7. PR sequence on `programming-cursor`

| PR | Stage | Title | Key files |
|----|-------|-------|-----------|
| 0 | 1.0 | **`docs/permission-ui-design.md`** | new doc — gate before TuiClient |
| 1 | 0 | Four-runner dev stack + NATS `-js` | `scripts/trogon-dev.sh`, `lib.rs:43`, `dev-runner.sh` |
| 2 | 0 | `trogon doctor` + model aliases + default mode | `doctor.rs`, `main.rs`, `repl.rs`, `session.rs` |
| 3 | 0 | Docs truth | `CLAUDE.md`, `.env.local.example`, `cli-status.md`, `TROGON.md` |
| 4 | 1 | LocalSet + TuiClient + `client::run` | `runtime.rs`, `tui_client.rs`, `main.rs` |
| 5 | 1 | Permission modes all 4 runners + `/mode` | `agent.rs` ×4, `repl.rs` |
| 6 | 1 | Cross-runner set_model + prefix rebind | `repl.rs`, `cross_runner.rs`, `runtime.rs` |
| 7 | 2 | ToolCallUpdate + diff names **(+ exit_code mapper — see §2.1)** | `session.rs`, `repl.rs`, optional `tool_update.rs` |
| 8 | 2 | `/compact` fix (B3a+B3b) + `--stream` | `session.rs`, `repl.rs`, `main.rs` |
| 9 | 3 | Session resume + `--continue` | `session_store.rs`, `session.rs`, `main.rs`, `repl.rs` |
| 10 | 4 | MCP wiring | `mcp.rs`, `main.rs`, `session.rs`, `repl.rs` |
| 11 | 4 | Print hardening | `print.rs`, `main.rs` |
| 12 | 5 | Compactor xai + openrouter | `compaction.rs`, xai/openrouter `agent.rs` |
| 13 | 5 | PortableMessageV2 + codex validation | `portable_session.rs`, all runners, `cross_runner.rs` |
| 14 | 5 | Codex TROGON.md + registry `/model` listing | codex `agent.rs`, `repl.rs` |
| 15 | 6 | Polish slash commands | `repl.rs`, `git.rs`, etc. |

**Commit policy:** unsigned commits only. **Never push** unless explicitly requested.

---

## 8. Timeline (v2)

```text
Stage 0   Four-runner dev stack + doctor        2–3 days
Stage 1   ACP client + /dev/tty permissions     8–10 days  ← trust hinge + design risk
Stage 2   Tool output + /compact (B3a+B3b)      3–4 days
Stage 3   Session resume                          3–4 days
Stage 4   MCP + print                             3–4 days
Stage 5   Four-runner parity + export v2          5–7 days
Stage 6   Polish                                  ongoing
──────────────────────────────────────────────────────────
Full "drop Claude Code CLI" (4 runners)         ~5–7 weeks
MVP (Claude-only daily driver)                  ~3–4 weeks  (Stages 0+1+2+3)
```

**Critical path:** 0 → 1 → 2 → 3 → 5

| Milestone | Stages | What you get |
|-----------|--------|--------------|
| **Dev stack works** | 0 | doctor green, bash, same-prefix `/model` |
| **Trustworthy terminal** | 0+1 | permissions, cross-runner `/model` |
| **Claude Code feel** | 0+1+2 | tool output, compact, streaming option |
| **Daily driver** | 0+1+2+3 | resume, MCP, print CI |
| **Full 4-runner** | through 5 | compactor, export v2, codex TROGON.md |

---

## 9. Mid-session switching — before vs after

| Action | Before plan | After plan |
|--------|-------------|------------|
| `/model sonnet` (same runner) | ✅ `set_model` | ✅ |
| `/model grok` (cross-prefix) | ⚠️ export/import; no set_model; no LocalSet; no perms on xai | ✅ |
| `/model <openrouter-id>` | ⚠️ only if runner manually started | ✅ in `trogon-dev.sh` |
| `/model o4-mini` (→ Codex) | ⚠️ lossy import | ✅ validated + model set |
| Tool context after switch | ❌ lost | ⚠️ v2 summaries (Stage 5) |
| Bash `cd` after switch | ❌ lost | ❌ documented v1 limit |
| Permissions on target runner | ❌ xai/OR/codex | ✅ all runners (Stage 1) |
| Long Grok/OR sessions | ❌ `trim_history` at 20 msgs | ✅ compactor (Stage 5) |
| Manual `/compact` | ❌ wrong subject + payload | ✅ request-reply + import (Stage 2) |

---

## 10. Credential modes

### Dev (recommended for substitution testing)

```bash
# .env.local — direct API keys, no vault
ANTHROPIC_TOKEN=sk-ant-...
ANTHROPIC_BASE_URL=https://api.anthropic.com/v1
XAI_API_KEY=xai-...
OPENROUTER_API_KEY=sk-or-...
CODEX_ENABLED=1
# Codex: authenticate via Codex CLI separately

./scripts/trogon-dev.sh
cd ~/project && trogon
```

### Production / team (docker-compose)

**File:** `rsworkspace/docker-compose.yml` — services already defined:

- `nats`, `proxy`, `worker` (vault tokens `VAULT_TOKEN_*`)
- `trogon-acp-runner`, `trogon-xai-runner`, `trogon-openrouter-runner`, `trogon-codex-runner`
- `trogon-wasm-runtime`, `trogon-compactor`

```bash
cd rsworkspace
# .env with ANTHROPIC_TOKEN=tok_* and VAULT_TOKEN_* in worker
docker compose up nats proxy worker trogon-acp-runner trogon-wasm-runtime trogon-compactor
# optional: trogon-xai-runner, trogon-openrouter-runner, trogon-codex-runner
trogon --nats-url nats://localhost:4222 --prefix acp.claude
```

**Note:** compose may need `WASM_ONLY=0` on wasm-runtime service (verify in Dockerfile/env).

---

## 11. Out of scope (v1)

- IDE extensions (`trogon-vscode`, JetBrains) — ACP covers editors separately
- `trogon-local-runner` (host bash) — wasm-runtime + `WASM_ONLY=0` suffices
- Full `spawn_agent` tool loops — one-shot spawn enough for v1
- Claude Code hooks (`PreToolUse`, `PostToolUse`, etc.)
- Sprites / safe external tool sandbox
- Multi-agent router/actor production deploy
- Bit-perfect cross-runner export (images, full tool I/O)
- `git commit`, generic `web_search` (document Grok native)

---

## 12. Success statement

When this plan is complete on `programming-cursor`:

> A developer runs `./scripts/trogon-dev.sh`, then `trogon` in any repo. They program with the same habits as Claude Code — files, bash, approvals on `/dev/tty`, compaction, resume, MCP — and switch models mid-session with `/model sonnet`, `/model grok`, `/model openai/gpt-4o`, or `/model o4-mini` without restarting or losing the thread. Claude Code CLI is no longer needed for terminal programming.

---

## 13. Changelog v1 → v2

| Change | Reason |
|--------|--------|
| Added **Stage 1.0** + `docs/permission-ui-design.md` | Readline can't serve async permission handler; use `/dev/tty` |
| **Stage 0 exit test** scoped — no cross-runner `/model` | Cross-runner needs LocalSet (Stage 1) |
| **`/compact` B3a + B3b** listed separately | Subject AND payload both wrong |
| **Stage 1 duration** 8–10 days | Permission design + integration risk |
| **MVP timeline** 3–4 weeks; full plan 5–7 weeks | Realistic buffers |
| **PR 0** added | Design doc before TuiClient code |
| **Bash failure modes** split | wasm-runtime (Stage 0) + permissions (Stage 1) |
| **Four runners** explicit throughout | OpenRouter + Codex in dev stack, matrix, Stage 5 |
| **§2.1 exit_code mapper** | `ToolCallUpdateFields` has no `exit_code`; PR 7 must parse meta / `__EXIT_N__` / infer from `status` |

---

## 14. Related documents

| Document | Status |
|----------|--------|
| `docs/claude-code-replacement/claude-code-replacement-definitive-plan.md` | Original block plan — partially superseded |
| `docs/programming.md` | PR checklist — gap table stale on xai/OR tools |
| `rsworkspace/docs/cli-status.md` | Stale (2026-05-06) — rewrite Stage 0 |
| `rsworkspace/CLAUDE.md` | Dev guide — update Stage 0 |
| `docs/permission-ui-design.md` | **To create** — Stage 1.0 gate |

**Canonical plan for terminal Claude Code replacement:** this document.
