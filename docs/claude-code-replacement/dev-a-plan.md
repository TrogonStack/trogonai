# Dev A — Work plan
# Services track (NATS backend)

> All work is additive. NATS remains as backend without changes.
> The names of all tools and the ToolContext contract are fixed in the "Tool contract" section at the end of this document.

---

## Wave summary

| Wave | PRs | Effort |
|---|---|---|
| Wave 1 | PR 1 + PR 2 | 1–2 weeks |
| Wave 2 | PR 4 + PR 6 | 1 week |
| Wave 3 | PR 7 + PR 10 (permissions) | 1–2 weeks |
| Wave 4 | PR 11 | 4–6 weeks |
| **Total** | | **7–11 weeks** |

---

## Wave 1

### PR 1 — Core tools in `trogon-agent-core`

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

**Branch:** `feat/core-tools`

---

### PR 2 — Bash with real state

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

**Branch:** `feat/bash-stateful`

---

## Wave 2

### PR 4 — Context and memory

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

**Branch:** `feat/trogon-md`

---

### PR 6 — MCP stdio as CLI HTTP proxy

**`crates/trogon-cli/src/stdio_mcp_bridge.rs` — new file**
- Launch MCP stdio process as child (e.g. `npx @modelcontextprotocol/server-filesystem ./`)
- Start axum server on a random local port
- Proxy JSON-RPC ↔ stdin/stdout of the child process
- Register in the session as `StoredMcpServer::Http { url: "http://127.0.0.1:<port>" }`
- On CLI exit: kill child process and release port
- The NATS backend changes no lines

**Branch:** `feat/mcp-stdio-bridge`

---

## Wave 3

### PR 7 — Additional tools and specialized agents

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
- Register `Explore` agent with skill: only reads files and answers questions, never edits or executes
- Register `Plan` agent with skill: only plans, does not execute destructive tools

**Branch:** `feat/extra-tools`

---

### PR 10 — Granular permissions

**`crates/trogon-acp-runner`**
- Allowlist/denylist by path (e.g. `allow: src/**, deny: .env`)
- Allowlist/denylist by bash command (e.g. `allow: cargo test, deny: rm -rf`)
- Same approval gates mechanism from the platform branch (NATS request-reply)
- Configurable in `TROGON.md` and via `/config`

**Branch:** `feat/permissions`

---

## Wave 4

### PR 11 — VS Code extension (`trogon-vscode`)

- Chat panel inside the editor
- Inline diffs with accept/reject changes per file
- Slash commands from the editor
- Communicates with `trogon-cli` or directly via NATS/ACP

**Branch:** `feat/vscode`

---

## Working branches

```
feat/claude-code-replacement    ← shared base branch with Dev B
  feat/core-tools               ← PR 1
  feat/bash-stateful            ← PR 2
  feat/trogon-md                ← PR 4
  feat/mcp-stdio-bridge         ← PR 6
  feat/extra-tools              ← PR 7
  feat/permissions              ← PR 10
  feat/vscode                   ← PR 11
```

Each feature branch makes a PR to `feat/claude-code-replacement`, not directly to `platform`.

### Flow when Dev B depends on something from Dev A

1. Dev A finishes their PR, merges it to `feat/claude-code-replacement`
2. Dev A notifies Dev B
3. Dev B syncs their local branch:
   ```bash
   git fetch origin
   git merge origin/feat/claude-code-replacement
   ```
4. Dev B now has Dev A's modules available and can compile

`feat/claude-code-replacement` is the shared source of truth — each developer syncs from there when they need what the other built.

---

## Tool contract — definitive names

The names are fixed here. **There is no pending coordination.** Dev A implements them exactly like this, Dev B registers them exactly like this.

### Tool names

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

### Change in `ToolContext`

Dev A adds the `cwd` field in `crates/trogon-agent-core/src/tools/mod.rs`:

```rust
pub struct ToolContext {
    pub proxy_url: String,
    pub cwd: String,           // working directory — comes from session.cwd = current_dir()
    pub http_client: reqwest::Client,
}
```

### Shared file with conflict risk

`trogon-agent-core/src/tools/mod.rs` — Dev A and Dev B both add entries to `dispatch_tool()`. Coordinate briefly if they coincide on that file at the same time.
