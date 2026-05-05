# Trogon AI — Definitive implementation plan
# Complete replacement of Claude Code for programming

> Respects the NATS architecture as backend without exception.
> Sources: `claude-code-replacement-plan.md` + `plan.txt`

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
| `.trogon/memory.md` base | ✅ (improved in PR 4) |
| Context compaction | ✅ (improved in PR 4) |
| Multi-model (Claude + Grok + OpenAI) | ✅ |
| Encrypted credentials vault | ✅ |
| Distributed multi-agent | ✅ |

---

## Blocks and timelines

| Blocks | What it delivers | Cumulative effort |
|---|---|---|
| Block 1 | Can read, write, execute, use git — works for programming | 3–4 weeks |
| Block 1 + 2 | Complete feature parity with Claude Code | 5–7 weeks |
| Block 1 + 2 + 3 | 100% replacement in terminal (without IDE) | 8–11 weeks |
| Block 1 + 2 + 3 + 4 | **100% replacement including IDE** | **3–4 months** |

---

## Block 1 — Execution layer (~3–4 weeks)

### PR 1 — Core tools in `trogon-agent-core`

**`crates/trogon-agent-core/Cargo.toml` — new dependencies**
- `globset = "0.4"`
- `ignore = "0.4"`
- `walkdir = "2"`
- `html2text = "0.12"`

**`crates/trogon-agent-core/src/tools/mod.rs` — extend `ToolContext` and `dispatch_tool()`**
- Add field `cwd: String` to `ToolContext` (session working directory)
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
- Atomic write: first writes to `.tmp`, then `rename` (prevents corruption if process dies mid-write)
- Returns `"OK"` or error message

`list_dir`
- Parameters: `path?: String` (default `"."`)
- Uses the `ignore` crate to respect `.gitignore` automatically
- Returns directory tree as text in `tree` style
- Limits to 500 entries to avoid saturating the agent context

`glob`
- Parameters: `pattern: String`, `path?: String`
- Uses `globset` for pattern matching (`src/**/*.rs`, `**/*.test.ts`)
- Filters entries ignored by `.gitignore` via `ignore::Walk`
- Returns list of paths relative to `cwd`

`notebook_edit`
- Parameters: `path: String`, `cell_index: u32`, `content: String`, `cell_type?: String`
- Reads `.ipynb` (pure JSON)
- Edits the cell by index
- Writes the file back

**`crates/trogon-agent-core/src/tools/editor.rs` — new file**

`str_replace`
- Parameters: `path: String`, `old_str: String`, `new_str: String`
- Reads the full file
- Verifies that `old_str` appears **exactly once** — if there are 0 or >1 occurrences, returns descriptive error with exact count
- Replaces and writes the result
- Returns diff of changed lines with ±3 lines of context

**`crates/trogon-agent-core/src/tools/git.rs` — new file**

`git_status`, `git_diff`, `git_log`
- Execute `git status --short`, `git diff`, `git log --oneline -20` respectively
- Use `tokio::process::Command` with `current_dir(&ctx.cwd)`
- Output limited to ~4KB; if exceeded, truncated with notice at the end
- No new dependencies (only tokio)

**`crates/trogon-agent-core/src/tools/web.rs` — new file**

`fetch_url`
- Parameters: `url: String`, `raw?: bool`
- Uses `ctx.http_client` (reuses existing client, does not create one per call)
- If `raw: false` (default): converts HTML to plain text with `html2text`
- Truncates to 8KB with notice if response is larger

---

### PR 2 — Bash with real state

**`crates/trogon-acp-runner/src/wasm_bash_tool.rs`**
- Add field `sandbox_dir: PathBuf` to the `WasmRuntimeBashTool` struct
- Pass it as `cwd` in `CreateTerminalRequest` so bash executes in the real project directory
- `sandbox_dir` is initialized from `session.cwd` in `new_session`

**`crates/trogon-acp-runner/src/session_store.rs`**
- Add `terminal_id: Option<String>` to `SessionState` (stored in NATS KV)
- First bash call: create terminal, save `terminal_id` in `SessionState`
- Subsequent calls: reuse the same terminal with `WriteToTerminalRequest`
- Without this, each bash call starts a new process and state (`cd`, environment variables) does not persist between calls
- Command end demarcation protocol: `<command>; echo "__EXIT_$?__"`
- Read stream until `__EXIT_N__` is found, extract exit code
- Configurable timeout, default 30s; on expiry, returns error with partial output

---

### PR 3 — `trogon-cli` core

**`crates/trogon-cli/Cargo.toml` — new crate**
- `rustyline = "14"`
- `acp-nats`, `clap`, `axum` — already in the workspace

**`crates/trogon-cli/src/main.rs`**
- Argument parsing with clap
- Reads `TROGON_NATS_URL` (default `nats://localhost:4222`)
- NATS autostart: if connection fails, launches `nats-server -p 4222` as child process
  - Retries up to 3s with 200ms intervals
  - If `nats-server` is not in PATH: prints installation instructions and exits with clear error
  - On exit: kills child process only if this process launched it

**`crates/trogon-cli/src/session.rs`**
- ACP session management via NATS
- Creates session with `cwd = std::env::current_dir()`
- Closes session cleanly on exit

**`crates/trogon-cli/src/repl.rs`**
- REPL loop with rustyline
- Ctrl+C sends cancel to active session and clears state
- Ctrl+D exits cleanly closing the session
- History persisted in `~/.local/share/trogon/history`
- Receives ACP event stream and prints `TextDelta` in real time

**`crates/trogon-cli/src/print.rs`**
- Basic non-interactive mode (completed in PR 10)

**`crates/trogon-agent/src/tools/mod.rs` — wiring the new tools**
- Register in `dispatch_tool()` all tools from PR 1: `read_file`, `write_file`, `list_dir`, `glob`, `str_replace`, `git_status`, `git_diff`, `git_log`, `fetch_url`, `notebook_edit`
- Coordinate with Dev A on day 1 to align tool names and `ToolContext.cwd` contract

---

## Block 2 — Developer tools (~2–3 weeks)

### PR 4 — Context and memory

**`crates/trogon-acp-runner/src/trogon_md.rs` — new file**

Hierarchical TROGON.md:
- Search for `TROGON.md` from `cwd` toward the root of the filesystem (same pattern as Claude Code with CLAUDE.md)
- Also load `~/.config/trogon/TROGON.md` as global user configuration
- Concatenate in order: global → repo root → most specific to current directory
- Inject into `system_prompt` when creating or loading a session

**`crates/trogon-acp-runner/src/agent.rs` — auto-compact with threshold**

Change from unconditional compact to 85% threshold:
```rust
let token_estimate = estimate_token_count(&session.messages); // heuristic len_bytes / 4
if token_estimate > TOKEN_BUDGET * 85 / 100 {
    compact_messages(&mut session).await?;
}
// TOKEN_BUDGET configurable in SessionState, default 200_000
```

---

### PR 5 — Agent extensibility

**`crates/trogon-cli/src/repl.rs` — file `@mentions`**
- Before sending the prompt, scan for `@<path>` tokens
- For each match: resolve path relative to `cwd`, read content, replace `@<path>` with code block containing the file content
- If path does not exist, leave the token unmodified and warn the user
- Path tab-completion in rustyline via a custom `Helper`

**`crates/trogon-agent-core/src/agent_loop.rs` — parallel tools**

Replace sequential execution with parallel using `join_all`:
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
Restriction: if an interactive `PermissionChecker` is active, serialize the calls to avoid asking the user two things simultaneously.

---

### PR 6 — MCP stdio as CLI HTTP proxy

**`crates/trogon-cli/src/stdio_mcp_bridge.rs` — new file**

The NATS backend changes no lines — it only sees normal HTTP MCP servers:
- CLI launches the MCP stdio process as a child (e.g. `npx @modelcontextprotocol/server-filesystem ./`)
- Starts an axum server on a random local port that proxies JSON-RPC ↔ stdin/stdout of the child process
- Registers the server in the session as `StoredMcpServer::Http { url: "http://127.0.0.1:<port>" }`
- On CLI exit: kills child process and releases the port

---

### PR 7 — Additional tools and specialized agents

**`crates/trogon-agent-core/src/tools/mod.rs` — todo management**
- Tool `todo_write`: parameters `id: String`, `content: String`, `status: String` (pending / in_progress / completed) — stored in NATS KV bucket per session
- Tool `todo_read`: returns list of active tasks for the session

**`crates/trogon-agent/src/tools/mod.rs` — `spawn_agent`**
- Create `spawn_agent` ToolDef
- Dispatch: NATS request-reply to `{prefix}.agent.spawn`
- The registry resolves the correct agent and returns the result to the current model turn

**`trogon-registry` — specialized sub-agents**
- Register `Explore` agent with skill: only reads files and answers questions, never edits or executes
- Register `Plan` agent with skill: only plans, does not execute destructive tools

---

## Block 3 — CLI UX (~3–4 weeks)

### PR 8 — TUI

**`crates/trogon-cli/src/repl.rs`**
- Show colored diffs (before/after) on each `str_replace` and `write_file` operation
- Ctrl+C cancels the active NATS operation and clears session state
- Show token and $ cost per session by consuming the `UsageSummary` events the platform branch already emits
- Navigable history with up/down arrow keys (rustyline already handles this with the PR 3 history)
- Multiline input

---

### PR 9 — Complete slash commands

**`crates/trogon-cli/src/repl.rs`**

Commands execute locally, they are not sent to the agent:

| Command | Action |
|---|---|
| `/clear` | Clears session message history via NATS KV |
| `/compact` | Forces context compaction now (`trogon.compactor.compact`) |
| `/cost` | Shows accumulated tokens and $ for the current session |
| `/help` | Lists all available commands |
| `/config` | Reads/writes local CLI config |
| `/model <id>` | Changes the model used in the session without restarting it |
| `/init` | Analyzes the project with LLM via ACP and generates `TROGON.md` in the current directory |

---

### PR 10 — Non-interactive mode and granular permissions

**`crates/trogon-cli/src/print.rs` — non-interactive mode**
- Activated with `trogon --print "do X"` or `trogon -p "do X"`
- Reads prompt from argument or from stdin: `trogon --print "explain" < error.log`
- Prints only `TextDelta` to stdout (no colors or interactive UI)
- Exit code 0 on success, 1 if agent returns error
- Useful for pipes and CI/CD

**`crates/trogon-acp-runner` — granular permissions**
- Allowlist/denylist by path (e.g. `allow: src/**, deny: .env`)
- Allowlist/denylist by bash command (e.g. `allow: cargo test, deny: rm -rf`)
- Same approval gates mechanism from the platform branch (NATS request-reply)
- Configurable in `TROGON.md` and via `/config`

---

## Block 4 — IDE integration (~2–3 months)

### PR 11 — VS Code extension (`trogon-vscode`)

- Chat panel inside the editor
- Inline diffs with accept/reject changes per file
- Slash commands from the editor
- Communicates with `trogon-cli` or directly via NATS/ACP

**Effort: 4–6 weeks**

### PR 12 — JetBrains plugin (`trogon-jetbrains`)

- Same concept on the JetBrains plugin API (IntelliJ, GoLand, RustRover, etc.)

**Effort: 4–6 weeks**

---

## Complete task list

### Block 1 — Execution layer

**PR 1 — `trogon-agent-core/src/tools/`**
- `mod.rs`: add `cwd: String` to `ToolContext`
- `mod.rs`: complete `dispatch_tool()` with routing for 9 new tools
- `fs.rs` (new): `read_file` with offset/limit, path traversal protection, cat-n
- `fs.rs`: atomic `write_file` (.tmp → rename) with create_dir_all
- `fs.rs`: `list_dir` respecting .gitignore, tree, 500 entry limit
- `fs.rs`: `glob` with globset + ignore::Walk
- `fs.rs`: `notebook_edit` for .ipynb files
- `editor.rs` (new): `str_replace` with exactly-once validation + diff ±3 lines
- `git.rs` (new): `git_status`, `git_diff`, `git_log` with 4KB limit
- `web.rs` (new): `fetch_url` with html2text and 8KB limit
- `Cargo.toml`: add globset, ignore, walkdir, html2text

**PR 2 — Bash stateful**
- `wasm_bash_tool.rs`: add `sandbox_dir: PathBuf`, pass `cwd` to `CreateTerminalRequest`
- `session_store.rs`: add `terminal_id: Option<String>` to `SessionState`
- `wasm_bash_tool.rs`: reuse terminal with `WriteToTerminalRequest` on subsequent calls
- `wasm_bash_tool.rs`: demarcation protocol `echo "__EXIT_$?__"` + exit code extraction
- `wasm_bash_tool.rs`: configurable timeout default 30s

**PR 3 — `trogon-cli` core**
- `Cargo.toml` (new): rustyline, acp-nats, clap, axum
- `main.rs`: clap + TROGON_NATS_URL + NATS autostart with retry 3s/200ms
- `main.rs`: print instructions if nats-server is not in PATH
- `main.rs`: kill child process only if it was launched by this process
- `session.rs`: create ACP session with cwd = current_dir(), close cleanly
- `repl.rs`: rustyline loop + print TextDelta in real time
- `repl.rs`: Ctrl+C cancels session, Ctrl+D exits cleanly
- `repl.rs`: history in ~/.local/share/trogon/history
- `trogon-agent/src/tools/mod.rs`: register all PR 1 tools in dispatch_tool()

---

### Block 2 — Developer tools

**PR 4 — Context and memory**
- `trogon_md.rs` (new): search for TROGON.md from cwd toward root of filesystem
- `trogon_md.rs`: load ~/.config/trogon/TROGON.md as global user config
- `trogon_md.rs`: concatenate in order global → root → current directory
- `trogon_md.rs`: inject into system_prompt when creating/loading session
- `agent.rs`: change unconditional compact to 85% threshold with len_bytes/4 heuristic
- `agent.rs`: TOKEN_BUDGET configurable in SessionState, default 200_000

**PR 5 — Extensibility**
- `repl.rs`: scan for `@<path>` in the prompt before sending to agent
- `repl.rs`: resolve path relative to cwd, read file, inject content
- `repl.rs`: warn if path does not exist, do not fail
- `repl.rs`: path tab-completion for @mentions via rustyline Helper
- `agent_loop.rs`: replace sequential for with parallel join_all
- `agent_loop.rs`: serialize if interactive PermissionChecker is active

**PR 6 — MCP stdio**
- `stdio_mcp_bridge.rs` (new): spawn MCP stdio process as child
- `stdio_mcp_bridge.rs`: start axum server on random local port
- `stdio_mcp_bridge.rs`: proxy JSON-RPC ↔ stdin/stdout of child process
- `stdio_mcp_bridge.rs`: register as StoredMcpServer::Http in the session
- `stdio_mcp_bridge.rs`: kill child process and release port on CLI exit

**PR 7 — Additional tools and agents**
- `mod.rs`: tool `todo_write` (NATS KV bucket per session, states: pending/in_progress/completed)
- `mod.rs`: tool `todo_read` (list active tasks for the session)
- `mod.rs`: `spawn_agent` ToolDef → NATS request-reply to {prefix}.agent.spawn
- `trogon-registry`: register Explore agent (only reads, never edits or executes)
- `trogon-registry`: register Plan agent (only plans, does not execute destructive tools)

---

### Block 3 — CLI UX

**PR 8 — TUI**
- `repl.rs`: render colored before/after diffs in str_replace and write_file
- `repl.rs`: Ctrl+C cancels active NATS operation and clears state
- `repl.rs`: consume UsageSummary events → show accumulated tokens and $ per session
- `repl.rs`: multiline input

**PR 9 — Slash commands**
- `repl.rs`: `/clear` — clears history via NATS KV
- `repl.rs`: `/compact` — forces compaction now
- `repl.rs`: `/cost` — shows accumulated tokens/$ for the session
- `repl.rs`: `/help` — lists available commands
- `repl.rs`: `/config` — reads/writes local config
- `repl.rs`: `/model <id>` — changes model without restarting session
- `repl.rs`: `/init` — analyzes project with LLM via ACP, generates TROGON.md

**PR 10 — Non-interactive and permissions**
- `print.rs`: `--print`/`-p` flag, read from argument
- `print.rs`: read prompt from stdin if no text is passed (`trogon --print < error.log`)
- `print.rs`: print only TextDelta to stdout, no colors
- `print.rs`: exit code 0 on success, 1 on agent error
- `trogon-acp-runner`: allowlist/denylist by path
- `trogon-acp-runner`: allowlist/denylist by bash command
- `trogon-acp-runner`: configurable in TROGON.md and via /config

---

### Block 4 — IDE integration

**PR 11 — VS Code (`trogon-vscode`)**
- Chat panel inside the editor
- Inline diffs with accept/reject changes per file
- Slash commands from the editor
- Communication with trogon-cli or directly via NATS/ACP

**PR 12 — JetBrains (`trogon-jetbrains`)**
- Same concept on the JetBrains plugin API

---

## Division of work

### Dev A — Services track

**Wave 1 (~1–2 weeks)**
- PR 1 complete: all tools in `trogon-agent-core/src/tools/` (fs, editor, git, web)
- PR 2 complete: stateful bash with terminal_id in SessionState

**Wave 2 (~1 week)**
- PR 4 complete: hierarchical TROGON.md + auto-compact 85%
- PR 6 complete: MCP stdio HTTP proxy in `stdio_mcp_bridge.rs`

**Wave 3 (~1–2 weeks)**
- PR 7 complete: todo_write/read, spawn_agent, Explore and Plan sub-agents
- PR 10 (permissions): allowlist/denylist by path and command in `trogon-acp-runner`

**Wave 4 (~4–6 weeks)**
- PR 11: VS Code extension

---

### Dev B — CLI track

**Wave 1 (~1–2 weeks)**
- PR 3 complete: trogon-cli core — REPL, ACP session, streaming, NATS autostart, Ctrl+C/D, history
- Wiring in `trogon-agent/src/tools/mod.rs`: register in dispatch_tool() the tools from PR 1 (coordinate with Dev A on day 1 to align names and ToolContext.cwd contract)

**Wave 2 (~1 week)**
- PR 5 complete: @mentions with tab-completion + parallel tools join_all

**Wave 3 (~2–3 weeks)**
- PR 8 complete: TUI — colored diffs, Ctrl+C, cost per session, multiline
- PR 9 complete: slash commands /clear, /compact, /cost, /help, /config, /model, /init

**Wave 4 (~2–3 weeks)**
- PR 10 (non-interactive): --print, stdin, exit codes
- PR 12: JetBrains plugin

---

### Effort per developer

| | Dev A | Dev B |
|---|---|---|
| Wave 1 | 1–2 weeks | 1–2 weeks |
| Wave 2 | 1 week | 1 week |
| Wave 3 | 1–2 weeks | 2–3 weeks |
| Wave 4 | 4–6 weeks | 2–3 weeks |
| **Total** | **7–11 weeks** | **6–9 weeks** |

---

### Tool contract — definitive names

The names are fixed here. There is no pending coordination between devs — each one consults this document and uses them exactly as shown.

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

**Change in `ToolContext`** — Dev A adds the `cwd` field in `crates/trogon-agent-core/src/tools/mod.rs`:

```rust
pub struct ToolContext {
    pub proxy_url: String,
    pub cwd: String,           // working directory — comes from session.cwd = current_dir()
    pub http_client: reqwest::Client,
}
```

### Flow and dependencies

The waves are priority order, not time synchronization. Each developer advances at their own pace — when they finish their wave they start the next without waiting for the other.

Dev B can do the wiring in `dispatch_tool()` as soon as Wave 1 starts, without waiting for Dev A to finish the implementations — as long as the modules exist (even if empty), it compiles. When Dev A merges a PR to `feat/claude-code-replacement`, Dev B syncs with `git fetch origin && git merge origin/feat/claude-code-replacement`.

**Only file with conflict risk:** `trogon-agent-core/src/tools/mod.rs` — both can add entries to `dispatch_tool()`. Coordinate briefly if they coincide on that file at the same time.

---

### Branch organization

```
platform
└── feat/claude-code-replacement      ← shared base branch (already exists in origin)
    ├── feat/core-tools               ← Dev A — PR 1
    ├── feat/bash-stateful            ← Dev A — PR 2
    ├── feat/trogon-md                ← Dev A — PR 4
    ├── feat/mcp-stdio-bridge         ← Dev A — PR 6
    ├── feat/extra-tools              ← Dev A — PR 7
    ├── feat/permissions              ← Dev A — PR 10 (permissions)
    ├── feat/vscode                   ← Dev A — PR 11
    ├── feat/cli-core                 ← Dev B — PR 3
    ├── feat/mentions-parallel        ← Dev B — PR 5
    ├── feat/cli-tui                  ← Dev B — PR 8
    ├── feat/slash-commands           ← Dev B — PR 9
    ├── feat/cli-noninteractive       ← Dev B — PR 10 (non-interactive)
    └── feat/jetbrains                ← Dev B — PR 12
```

Short PRs per feature → `feat/claude-code-replacement`. A single final PR `feat/claude-code-replacement` → `platform` when everything is ready.

### Flow when a developer depends on the other's work

1. Dev A (or B) finishes their PR and merges it to `feat/claude-code-replacement`
2. Notifies the other developer
3. The other syncs their local branch:
   ```bash
   git fetch origin
   git merge origin/feat/claude-code-replacement
   ```
4. They now have the other's work available and can compile

`feat/claude-code-replacement` is the shared source of truth — each developer syncs from there when they need what the other built.

---

## Why everything respects the NATS architecture

| Pattern | PRs |
|---|---|
| Extension of existing crates without new NATS service | PR 1, PR 2, PR 4, PR 5, PR 7 |
| New NATS client (does not touch the backend) | PR 3, PR 8, PR 9, PR 10, PR 11, PR 12 |
| Bridge that hides complexity from the backend | PR 6 — the runner only sees a normal HTTP MCP server |

The filesystem and git tools live in `trogon-agent-core` and access the filesystem directly via `ctx.cwd`. NATS communication happens at the session and agent level, not at the level of each file read — more efficient and simpler.
