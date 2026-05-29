# Trogon CLI — Complete Substitution Test Checklist

Legend: **[IMPL]** = implemented and should work | **[STUB/PARTIAL]** = code exists but incomplete | **[MISSING]** = not implemented in trogon

Branch: `programming-gaps`. Generated: 2026-05-27.

---

## 1. Startup & Invocation

- **[IMPL]** `trogon` starts interactive REPL, prints session ID and `(Ctrl+D to quit)`
- **[IMPL]** `trogon --nats-url nats://other:4222` uses alternate NATS server
- **[IMPL]** `trogon --prefix acp.claude` uses custom ACP prefix
- **[IMPL]** `trogon --dangerously-skip-permissions` skips all tool gates
- **[IMPL]** `trogon --continue-session` resumes last session for current project
- **[IMPL]** `trogon --session-id <id>` attaches to specific session
- **[IMPL]** `trogon --model sonnet` resolves alias and starts with that model
- **[IMPL]** `TROGON_NATS_URL` env var overrides `--nats-url`
- **[IMPL]** `ACP_PREFIX` env var overrides `--prefix`
- **[IMPL]** `TROGON_MODE=acceptEdits trogon` starts in acceptEdits mode
- **[MISSING]** `--system-prompt "custom"` — no system prompt override from CLI
- **[MISSING]** `--append-system-prompt "extra"` — no system prompt append
- **[MISSING]** `--add-dir /other/path` — no additional working directories
- **[MISSING]** `--verbose` / `-v` flag
- **[MISSING]** `--plan` to start immediately in plan mode
- **[MISSING]** `--name "session-name"` for named sessions

---

## 2. REPL Basic Behavior

- **[IMPL]** Type a message, press Enter → gets a response
- **[IMPL]** Multi-turn conversation — second message has context of first
- **[IMPL]** Line ending with `\` + Enter → continues input on next line (multiline)
- **[IMPL]** Ctrl+C during streaming → cancels the in-flight response cleanly
- **[IMPL]** Ctrl+D → exits the REPL
- **[IMPL]** Ctrl+C on empty input (no active response) → does NOT exit
- **[IMPL]** Up/Down arrows → navigate command history
- **[IMPL]** History persists across sessions (`~/.local/share/trogon/history`)
- **[IMPL]** User input styled as `┃ {text}` after submit
- **[IMPL]** Token bar shows after each turn (e.g., `1,245/200,000 tokens (0%)`)
- **[MISSING]** Shift+Tab to cycle permission modes (default → acceptEdits → plan)
- **[MISSING]** `! <command>` shell mode — run shell commands directly in REPL
- **[MISSING]** Ctrl+R reverse history search
- **[MISSING]** Ctrl+L to redraw screen
- **[MISSING]** Ctrl+G / Ctrl+X Ctrl+E to open prompt in `$EDITOR`
- **[MISSING]** Escape key to stop streaming mid-response (keeping work done)
- **[MISSING]** Prompt suggestions (Tab to accept autocomplete suggestion)

---

## 3. @-Mention File Expansion

- **[IMPL]** `@src/main.rs` in prompt expands file content as markdown code block
- **[IMPL]** Tab completion on `@` prefix to pick files
- **[IMPL]** Files over 200 KB truncated with a warning
- **[IMPL]** `@some/directory` rejected with "is a directory" message
- **[IMPL]** `@../../etc/passwd` (path escaping cwd) is rejected
- **[IMPL]** `@nonexistent.rs` gives a readable error, not a panic
- **[IMPL]** Multiple `@mentions` in one prompt all expand
- **[IMPL]** File content actually reaches the runner and Claude uses it

---

## 4. Slash Commands

### Navigation & Info

- **[IMPL]** `/help` — shows all available commands
- **[IMPL]** `/status` — shows prefix, model, token counts, registered runners
- **[IMPL]** `/cost` — shows token usage with context%, estimated cost
- **[IMPL]** `/pwd` — prints current working directory (synced from session)
- **[IMPL]** `/cd <path>` — changes cwd, tilde-aware
- **[IMPL]** `cd <path>` (without slash) — alias for `/cd`
- **[IMPL]** `/cd` with relative path works from current session cwd
- **[IMPL]** `/cd` after previous `/cd` uses new cwd as base (not stale)
- **[IMPL]** `cd` from bash terminal tool also updates REPL's cwd

### Session Control

- **[IMPL]** `/clear` — starts new session, clears history; reports failure if runner is down
- **[IMPL]** `/sessions` — lists all sessions on current runner with timestamps
- **[IMPL]** `/resume <session-id>` — attaches to an existing session by ID
- **[IMPL]** `/resume` with invalid ID — graceful error, doesn't crash
- **[IMPL]** `/clear` then `/resume <old-id>` — restores old conversation

### Context

- **[IMPL]** `/compact` — forces context compaction, shows before/after token counts
- **[IMPL]** `/compact` timeout (120s) — doesn't hang REPL indefinitely
- **[IMPL]** Auto-compaction fires at ~85% context usage (runner-side)

### Model & Mode

- **[IMPL]** `/model` — shows current model and available models from registry
- **[IMPL]** `/model sonnet` — same-runner model switch via alias
- **[IMPL]** `/model grok` — cross-runner switch (exports history, creates session on xai-runner, imports)
- **[IMPL]** `/model <unknown>` — graceful "not found" error, session unchanged
- **[IMPL]** `/mode` — shows current permission mode
- **[IMPL]** `/mode acceptEdits` — switches permission mode live
- **[IMPL]** `/mode bypassPermissions` — skips all gates
- **[IMPL]** `/mode invalid` — graceful error listing valid modes

### Memory

- **[IMPL]** `/memory list` — shows TROGON.md layers (global → project)
- **[IMPL]** `/memory show` — prints merged TROGON.md content
- **[IMPL]** `/memory show <path>` — prints specific file
- **[IMPL]** `/memory edit` — prints path to open in `$EDITOR`

### MCP

- **[IMPL]** `/mcp list` — shows configured servers and active bridges
- **[IMPL]** `/mcp add <name> <command> [args...]` — registers stdio MCP server
- **[IMPL]** `/mcp remove <name>` — unregisters and shuts down server
- **[IMPL]** MCP server spawns as subprocess; its tools appear in session
- **[IMPL]** MCP bridges shut down on `/clear` (no leaked child processes)
- **[IMPL]** MCP bridges shut down on `/model` cross-runner switch
- **[MISSING]** `/mcp add --transport http <name> <url>` — HTTP transport MCP
- **[MISSING]** MCP OAuth authentication
- **[MISSING]** `/mcp__<server>__<prompt_name>` as callable slash command

### Config

- **[IMPL]** `/config` — shows all config from `~/.config/trogon/config.json`
- **[IMPL]** `/config get <key>` — retrieves single value
- **[IMPL]** `/config set <key> <value>` — stores value persistently across restarts

### Init & Doctor

- **[IMPL]** `/init` — analyzes project with AI, generates TROGON.md
- **[IMPL]** `/init --force` — overwrites existing TROGON.md
- **[IMPL]** `/init` runs with `bypassPermissions` mode (no tool gates)
- **[IMPL]** `/doctor` — runs same health checks as `trogon doctor`
- **[MISSING]** `/review` — PR review
- **[MISSING]** `/pr-comments` — view and respond to PR comments
- **[MISSING]** `/release-notes`
- **[MISSING]** `/allowed-tools` — show tools available in current mode
- **[MISSING]** `/vim` — toggle vim mode for prompt input
- **[MISSING]** `/ide` — manage IDE integration
- **[MISSING]** `/login` / `/logout`
- **[MISSING]** `/bug` — report a bug
- **[MISSING]** `/rewind` / `/checkpoint` — restore prior conversation state
- **[MISSING]** `/plan` as an explicit command
- **[MISSING]** `/agents` — manage subagents
- **[MISSING]** `/tasks` — list background tasks

---

## 5. Non-Interactive / `--print` Mode

- **[IMPL]** `trogon --print "prompt"` — sends prompt, prints response, exits 0
- **[IMPL]** `echo "hello" | trogon --print` — reads from stdin
- **[IMPL]** `trogon --print < file.txt` — reads file as prompt
- **[IMPL]** `trogon --print --output-format json` — outputs `{"text":"...","stop_reason":"..."}`
- **[IMPL]** `trogon --print --print-tools` — emits NDJSON lines for tool calls
- **[IMPL]** Exit code 0 on `end_turn`
- **[IMPL]** Exit code 1 on error or cancelled
- **[IMPL]** Exit code 2 on max turns reached
- **[IMPL]** `trogon --print --model grok "prompt"` — uses specified model
- **[IMPL]** `trogon --print --stream "prompt"` — streams text live
- **[MISSING]** `--output-format stream-json` (NDJSON event stream)

---

## 6. Session Management

- **[IMPL]** New session created on startup; ID shown in header
- **[IMPL]** Session index written to `~/.local/share/trogon/sessions.json`
- **[IMPL]** `--continue-session` resumes last session for the current project (keyed by git root/cwd)
- **[IMPL]** `--continue-session` falls back to new session if last not found, with warning
- **[IMPL]** `--session-id <id>` attaches directly (conflicts with `--continue-session`)
- **[IMPL]** `/sessions` lists existing sessions; timestamps shown
- **[IMPL]** `/resume <id>` binds existing session and restores conversation
- **[IMPL]** Session index writes are atomic (no corruption under concurrent access)
- **[IMPL]** `/clear` creates new session; old session closed
- **[MISSING]** Session naming (`--name`, `/rename`)
- **[MISSING]** `trogon sessions list` as a CLI subcommand (trogon only has `/sessions` in REPL)

---

## 7. Permission System

- **[IMPL]** In `default` mode, tools trigger an interactive permission prompt
- **[IMPL]** Permission prompt reads from `/dev/tty` (works even when stdin is piped)
- **[IMPL]** `a` → allow once; `w` → allow always (session); `r` → reject; `c`/Esc → cancel
- **[IMPL]** Prompt times out after 55 seconds
- **[IMPL]** Only one permission prompt active at a time (`PermissionCoordinator`)
- **[IMPL]** `acceptEdits` mode auto-approves file edits without prompt
- **[IMPL]** `plan` mode shows tools but doesn't execute
- **[IMPL]** `dontAsk` mode auto-approves everything
- **[IMPL]** `bypassPermissions` mode skips all gates
- **[IMPL]** `/mode <name>` changes mode live mid-session
- **[IMPL]** `TROGON_MODE` env var sets initial mode
- **[IMPL]** `--dangerously-skip-permissions` flag activates bypass
- **[IMPL]** `w` (allow always) persists for the same tool within the session
- **[IMPL]** Rejecting a tool call doesn't crash or hang the session
- **[MISSING]** `permissions.allow` / `permissions.deny` rules in config (pre-approve specific tool patterns)
- **[MISSING]** `auto` mode (LLM-based safety classifier)
- **[MISSING]** Protected paths never auto-approved (`.git/`, `.env`, etc.)
- **[MISSING]** `permissions.additionalDirectories` — allow reads outside cwd

---

## 8. Tool Output & Rendering in REPL

- **[IMPL]** `┆ <tool-name>` shown while tool is active
- **[IMPL]** Tool output truncated at 2048 chars with `… [truncated]`
- **[IMPL]** CRLF in tool output normalized (`\r\n` → `\n`)
- **[IMPL]** Trailing whitespace stripped from tool output
- **[IMPL]** Bash stdout shown in REPL (ToolCallUpdate)
- **[IMPL]** Diff display for editor tool
- **[IMPL]** Non-zero exit code shown for bash failures
- **[IMPL]** Tool errors (e.g., file not found) surfaced to user, don't crash session
- **[IMPL]** Long-running bash commands stream output progressively
- **[IMPL]** Tool writes a file — file actually exists on disk after response

---

## 9. Markdown Rendering

- **[IMPL]** `**bold**` → ANSI bold
- **[IMPL]** `*italic*` → ANSI italic
- **[IMPL]** `` `code` `` → highlighted inline code
- **[IMPL]** ` ```rust ... ``` ` → code block with language label
- **[IMPL]** `# Heading` → formatted header
- **[IMPL]** `---` → horizontal rule
- **[IMPL]** Inline tool status `┆ tool-name` during active tool call
- **[IMPL]** Nested markdown (code inside list) renders without garbling
- **[IMPL]** Raw ANSI in model output doesn't double-render or break terminal

---

## 10. TROGON.md Memory (≡ CLAUDE.md)

- **[IMPL]** `TROGON.md` at git root loaded and injected into every session
- **[IMPL]** `/init` generates TROGON.md from codebase analysis
- **[IMPL]** `/init --force` overwrites existing file
- **[IMPL]** `/memory list` shows the hierarchy (global → project)
- **[IMPL]** `/memory show` prints merged content
- **[IMPL]** `/memory edit` prints path for `$EDITOR`
- **[IMPL]** Edit TROGON.md between sessions — new session picks up changes
- **[IMPL]** Model obeys instructions in TROGON.md
- **[STUB/PARTIAL]** Global TROGON.md at `~/.config/trogon/TROGON.md` — referenced but verify actually loaded
- **[MISSING]** Path-scoped rules (`.trogon/rules/*.md` with glob frontmatter)
- **[MISSING]** `@path/to/file` imports inside TROGON.md
- **[MISSING]** Subdirectory TROGON.md files loaded on-demand when Claude reads files in that dir

---

## 11. Configuration & Environment

- **[IMPL]** `~/.config/trogon/config.json` stores config persistently
- **[IMPL]** `/config set <key> <value>` survives restart
- **[IMPL]** `/config get <key>` returns stored value
- **[IMPL]** `.env.local` loaded from cwd ancestors (no override of existing non-empty vars)
- **[IMPL]** `TROGON_ENV_FILE=<path>` to specify custom env file
- **[IMPL]** Tilde expansion in paths (`~/...` → `$HOME/...`)
- **[MISSING]** `settings.json` schema (trogon uses simpler key-value, not full Claude Code settings structure)
- **[MISSING]** Project `.claude/settings.json` / `.claude/settings.local.json`
- **[MISSING]** `permissions.*` fields in config
- **[MISSING]** `cleanupPeriodDays` (session auto-cleanup)

---

## 12. MCP Integration

- **[IMPL]** `~/.config/trogon/mcp.json` stores MCP server configurations
- **[IMPL]** MCP server spawned as subprocess, JSON-RPC over stdio
- **[IMPL]** Bridge HTTP URL generated and passed to ACP session
- **[IMPL]** All configured servers spawn on REPL start
- **[IMPL]** Bridges committed to session after creation
- **[IMPL]** Bridges shut down on `/clear`, `/resume`, `/model` cross-runner switch
- **[IMPL]** `/mcp add <name> <cmd> [args...]` with optional env vars
- **[IMPL]** `/mcp remove <name>` removes and shuts down bridge
- **[IMPL]** Add an MCP server with a custom tool → tool appears in Claude's tool list
- **[IMPL]** MCP tool call works end-to-end (Claude calls it, result returned)
- **[IMPL]** Broken MCP server (crashes on start) doesn't crash trogon
- **[IMPL]** `/mcp remove` stops the subprocess (no zombie process)
- **[MISSING]** HTTP transport MCP servers
- **[MISSING]** SSE transport MCP servers
- **[MISSING]** MCP OAuth (bearer tokens, client credentials)
- **[MISSING]** `alwaysLoad` / deferred tool search
- **[MISSING]** MCP resources (`@server:protocol://resource/path`)
- **[MISSING]** Per-server timeouts
- **[MISSING]** Import from Claude Desktop config

---

## 13. Cross-Runner Model Switching

- **[IMPL]** `/model <id>` on same runner → calls `set_model`, no session restart
- **[IMPL]** `/model <id>` on different runner → exports history, creates session on target, imports history
- **[IMPL]** MCP bridges shut down before switch, respawned after
- **[IMPL]** Permission client rebound to new runner's prefix
- **[IMPL]** Model alias `haiku/sonnet/opus` resolves to acp.claude
- **[IMPL]** Model alias `grok/grok-mini` resolves to acp.grok (xai-runner)
- **[IMPL]** Model alias `openrouter` resolves to acp.openrouter
- **[IMPL]** `/model <unknown>` → "not found" error, session unchanged
- **[IMPL]** Conversation history visible in new runner after cross-runner switch
- **[IMPL]** Tool calls work on the new runner after switch
- **[STUB/PARTIAL]** Cross-runner switch while a response is in progress — needs verification
- **[STUB/PARTIAL]** **Known bug:** exported Claude history includes identity statements ("I'm Claude") → grok-4 continues that persona. Fix: inject neutral system prompt on import.

---

## 14. Context Compaction

- **[IMPL]** `/compact` sends request to compactor, shows before/after token counts
- **[IMPL]** Timeout 120s — REPL not blocked indefinitely
- **[IMPL]** Auto-compaction triggers at 85% context usage (runner-side)
- **[IMPL]** V2 export preserves tool_use/tool_result pairs
- **[IMPL]** Tracked token count refreshed after compaction
- **[IMPL]** Conversation continues coherently after compaction

---

## 15. Subagents (`spawn_agent` tool)

- **[IMPL]** `spawn_agent` tool available (Dev A PR 7)
- **[IMPL]** `Explore` and `Plan` subagent types
- **[IMPL]** Subagent completes and result flows back to parent
- **[IMPL]** Subagent has independent context window
- **[MISSING]** Custom subagent definitions (`.claude/agents/` directory)
- **[MISSING]** Worktree isolation for subagents (`isolation: worktree`)
- **[MISSING]** `/agents` REPL command to manage subagents
- **[MISSING]** `/tasks` to list background tasks

---

## 16. `trogon doctor`

- **[IMPL]** `trogon doctor` subcommand runs all health checks
- **[IMPL]** Checks: NATS TCP, JetStream, agent registry, compactor, runner availability, session creation, tool dispatch
- **[IMPL]** Exit code 0 if all required checks pass, 1 if any fail
- **[IMPL]** Green/red/yellow/dim colored output
- **[IMPL]** With NATS down → exits 1, shows which check failed
- **[IMPL]** With NATS up but no runners → shows appropriate warning
- **[IMPL]** `/doctor` in REPL runs same checks

---

## 17. `trogon dev`

- **[IMPL]** `trogon dev` starts local dev stack (NATS, wasm, compactor, runners)
- **[IMPL]** `trogon dev` then `trogon doctor` → all green
- **[IMPL]** Stack is healthy after `trogon dev`

---

## 18. Durability & Recovery

- **[IMPL]** Runner restart → `/clear` reports failure cleanly (no silent hang)
- **[IMPL]** xai-runner restores sessions from KV on prompt after restart
- **[IMPL]** `no_ack` on ClientOps stream prevents PubAck/reply races
- **[IMPL]** Kill and restart runner while REPL is open → clear error, not silent hang
- **[STUB/PARTIAL]** **Known bug:** Sonnet/Opus API rate limit → runner calls API, gets blocked, never sends error back → CLI hangs for up to 180s. Workaround: use haiku or grok.

---

## 19. Hooks System — MISSING (Major Gap)

- **[MISSING]** `PreToolUse` hook — intercept/block/modify tools before execution
- **[MISSING]** `PostToolUse` hook — react to tool results
- **[MISSING]** `Stop` hook — post-turn processing
- **[MISSING]** `Notification` hook
- **[MISSING]** `UserPromptSubmit` hook
- **[MISSING]** Any `hooks` key in config/settings
- **[MISSING]** Shell command hooks, HTTP hooks, MCP tool hooks

---

## 20. Missing Features — Full List

These Claude Code capabilities are **not implemented** in trogon. Known scope gaps, not bugs.

| Feature | Notes |
|---------|-------|
| Hooks system (all events) | PreToolUse, PostToolUse, Stop, Notification, etc. |
| Checkpointing / `/rewind` | Conversation and file snapshots |
| `/login` / `/logout` | Trogon uses NATS/API key directly |
| Auto-updater | No equivalent |
| Sandboxing (`sandbox.*` config) | |
| Vim mode | Editor-style keybindings for prompt input |
| Voice dictation | Hold Space to dictate |
| Background sessions (`/background`) | Detach session |
| Remote control | Control Claude from phone/remote |
| Batch mode (`/batch`) | Parallel task decomposition |
| Extended thinking toggle (Alt+T) | |
| Fast mode toggle (Alt+O) | |
| `/pr-comments`, `/review` | GitHub integration |
| `/ide` | IDE extension management |
| `/bug` | In-CLI bug reporting |
| Plugin / skill marketplace | `/plugin install`, SKILL.md runtime |
| `stream-json` output format | For `--print` scripting |
| Named sessions (`--name`, `/rename`) | |
| `--system-prompt` / `--append-system-prompt` | |
| Channels (Telegram, Discord, iMessage) | Push events |
| `auto` permission mode | LLM classifier |
| Protected paths never auto-approved | `.git/`, `.env`, etc. |
| Per-project `settings.json` schema | Full Claude Code settings structure |
| Custom keybinding config file | `~/.claude/keybindings.json` equivalent |
| Prompt suggestions (tab autocomplete) | |
| Shift+Tab mode cycling | |
| `!` shell mode in REPL | |
| Ctrl+R reverse history search | |
| MCP HTTP/SSE transports | |
| MCP OAuth authentication | |
| MCP resources (`@server:protocol://resource`) | |
| Path-scoped rules (`.trogon/rules/`) | |
| Worktree management from CLI | |
| Subagent custom definitions | `.claude/agents/` |
| `/agents`, `/tasks` REPL commands | |
| `trogon sessions list` as CLI subcommand | |
| Session auto-cleanup (`cleanupPeriodDays`) | |
| `permissions.allow/deny` patterns in config | |
