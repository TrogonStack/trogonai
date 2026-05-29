# Dev B — Work plan
# CLI track (developer surface)

> All work is additive. NATS remains as backend without changes.
> The names of all tools and the ToolContext contract are fixed in the "Tool contract" section at the end of this document.

---

## Wave summary

| Wave | PRs | Effort |
|---|---|---|
| Wave 1 | PR 3 + wiring dispatch | 1–2 weeks |
| Wave 2 | PR 5 | 1 week |
| Wave 3 | PR 8 + PR 9 | 2–3 weeks |
| Wave 4 | PR 10 (non-interactive) + PR 12 | 2–3 weeks |
| **Total** | | **6–9 weeks** |

---

## Wave 1

### PR 3 — `trogon-cli` core

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
- Empty stub for now (completed in PR 10)

**Branch:** `feat/cli-core`

---

### Wiring in `trogon-agent` — Wave 1

**`crates/trogon-agent/src/tools/mod.rs`**

Register in `dispatch_tool()` all tools that Dev A implements in PR 1:
- `"read_file"`
- `"write_file"`
- `"list_dir"`
- `"glob"`
- `"str_replace"`
- `"git_status"`
- `"git_diff"`
- `"git_log"`
- `"fetch_url"`
- `"notebook_edit"`

> Wait for confirmation from Dev A on day 1 with the exact names and `ToolContext` signature before making this PR.

**Branch:** part of `feat/cli-core` or own branch `feat/dispatch-wiring`

---

## Wave 2

### PR 5 — Agent extensibility

**`crates/trogon-cli/src/repl.rs` — file `@mentions`**
- Before sending the prompt to the agent, scan the text for `@<path>` tokens
- For each match:
  - Resolve the path relative to `cwd`
  - Read the file content
  - Replace `@<path>` with code block containing the content
- If path does not exist: leave the token unmodified and warn the user
- Path tab-completion in rustyline via a custom `Helper`

**`crates/trogon-agent-core/src/agent_loop.rs` — parallel tools**

Replace sequential execution with parallel:
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

**Branch:** `feat/mentions-parallel`

---

## Wave 3

### PR 8 — TUI

**`crates/trogon-cli/src/repl.rs`**
- Show colored diffs (before/after) on each `str_replace` and `write_file` operation
- Ctrl+C cancels the active NATS operation and clears session state
- Consume `UsageSummary` events (already emitted by the platform branch) and show accumulated tokens and $ per session
- Multiline input

**Branch:** `feat/cli-tui`

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

**Branch:** `feat/slash-commands`

---

## Wave 4

### PR 10 — Non-interactive mode

**`crates/trogon-cli/src/print.rs`**
- Activated with `trogon --print "do X"` or `trogon -p "do X"`
- Reads prompt from argument or from stdin: `trogon --print "explain" < error.log`
- Prints only `TextDelta` to stdout (no colors or interactive UI)
- Exit code 0 on success, 1 if agent returns error
- Useful for pipes and CI/CD

**Branch:** `feat/cli-noninteractive`

---

### PR 12 — JetBrains plugin (`trogon-jetbrains`)

- Same concept as the VS Code extension (done by Dev A)
- JetBrains plugin API (IntelliJ, GoLand, RustRover, etc.)
- Chat panel inside the editor
- Inline diffs with accept/reject changes
- Slash commands from the editor
- Communicates with `trogon-cli` or directly via NATS/ACP

**Branch:** `feat/jetbrains`

---

## Working branches

```
feat/claude-code-replacement    ← shared base branch with Dev A
  feat/cli-core                 ← PR 3 + wiring dispatch
  feat/mentions-parallel        ← PR 5
  feat/cli-tui                  ← PR 8
  feat/slash-commands           ← PR 9
  feat/cli-noninteractive       ← PR 10
  feat/jetbrains                ← PR 12
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

### Wiring that Dev B does — exact code

Open `crates/trogon-agent/src/tools/mod.rs`, add the imports and match arms in `dispatch_tool()`:

```rust
use trogon_agent_core::tools::{fs, editor, git, web};

// in dispatch_tool() — add alongside the existing GitHub/Linear/Slack tools:
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
"todo_write"     => todo::write(ctx, input).await,
"todo_read"      => todo::read(ctx, input).await,
"spawn_agent"    => agent::spawn(ctx, input).await,
```

Dev B can make this PR even if Dev A's implementations are not ready — as long as the modules exist (even if empty), it compiles.

### Shared file with conflict risk

`trogon-agent-core/src/tools/mod.rs` — Dev A and Dev B both add entries to `dispatch_tool()`. Coordinate briefly if they coincide on that file at the same time.
