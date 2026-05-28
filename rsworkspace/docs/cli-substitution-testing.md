# How to Test: Trogon CLI Substitution

Tests that verify trogon can fully substitute Claude Code CLI.
Branch: `programming-gaps`. Generated: 2026-05-27.

**Legend:** ✅ implemented | ⚠️ stub/partial | ❌ missing

---

## Prerequisites (one-time setup)

```bash
cd rsworkspace

# 1. Fill in API keys
cp .env.local.example .env.local
# Edit .env.local — set ANTHROPIC_TOKEN, XAI_API_KEY, OPENROUTER_API_KEY

# 2. Build everything
cargo build --release \
  -p trogon-cli \
  -p trogon-acp-runner \
  -p trogon-xai-runner \
  -p trogon-openrouter-runner \
  -p trogon-compactor \
  -p trogon-wasm-runtime

# 3. Start the stack
./scripts/trogon-dev.sh

# 4. Verify
./target/release/trogon doctor

# Alias for convenience
alias trogon="$(pwd)/target/release/trogon"
```

---

## A. Unit Tests (no NATS required)

```bash
cargo test -p trogon-cli --lib
# ~143 tests. Covers: slash command parsing, model aliases, markdown rendering,
# @mention expansion, config CRUD, session store, print mode logic
```

---

## B. Integration Tests (NATS must be running)

```bash
cargo test -p trogon-cli --test slash_commands_integration -- --nocapture
cargo test -p trogon-cli --test session_integration -- --nocapture
cargo test -p trogon-cli --test print_integration -- --nocapture
cargo test -p trogon-cli --test stdio_mcp_bridge_integration -- --nocapture
```

---

## C. Startup & Flags ✅

**C1. Basic start**
```bash
trogon
# Expect: "trogon — session <id> (Ctrl+D to quit)"
# Type: hello  →  get a response, token bar appears after
# Ctrl+D → exits cleanly
```

**C2. NATS URL override**
```bash
trogon --nats-url nats://localhost:4222       # works
trogon --nats-url nats://localhost:9999       # fails with error on startup
```

**C3. `--continue-session`**
```bash
trogon           # type "remember the number 42", Ctrl+D
trogon --continue-session
# type "what number did I ask you to remember?"
# Expect: "42"
```

**C4. `--session-id <id>`**
```bash
# Get an ID from /sessions, then:
trogon --session-id <that-id>
# Expect: attaches and prior conversation is visible
```

**C5. `--dangerously-skip-permissions`**
```bash
cd rsworkspace
trogon --dangerously-skip-permissions
# type: "write a file trogon-test.txt with content hello"
# Expect: no permission prompt, file created at rsworkspace/trogon-test.txt
# Note: write_file only works inside the session cwd regardless of mode;
#       use bash for /tmp/ paths: "run: echo hello > /tmp/trogon-test.txt"
```

**C6. `TROGON_MODE` env var**
```bash
cd rsworkspace
TROGON_MODE=acceptEdits trogon
# type: "write a file trogon-mode-test.txt with content test"
# Expect: no permission prompt, file created at rsworkspace/trogon-mode-test.txt
# Note: acceptEdits auto-approves file writes/edits inside cwd; bash still prompts
```

**C7. `--model` alias in print mode**
```bash
trogon --print "say hi" --model haiku
trogon --print "say hi" --model sonnet
# Both should respond (use haiku if sonnet is rate-limited)
```

---

## D. REPL Behavior ✅

**D1. Ctrl+C cancels in-flight response**
```bash
trogon
# type: "write me a 10000 word essay about the universe"
# While generating: press Ctrl+C
# Expect: stops, REPL returns to prompt — no hang, no crash
```

**D2. Ctrl+C on empty input does NOT exit**
```bash
trogon
# At empty prompt: Ctrl+C → stays in REPL
# Ctrl+D → exits
```

**D3. Multiline with backslash**
```bash
trogon
# type: first line\    (backslash then Enter)
# type: second line    (Enter)
# Expect: both lines sent as one prompt
```

**D4. History persists across restarts**
```bash
trogon         # type a few messages, Ctrl+D
trogon
# Up arrow → see previous commands
ls ~/.local/share/trogon/history   # file exists
```

**D5. Token bar appears after each turn**
```bash
# After any response: expect line like "1,245/200,000 tokens (0%)"
```

---

## E. @-Mention File Expansion ✅

**E1. Basic file expansion**
```bash
cd rsworkspace   # paths are relative to session cwd
trogon
# type: summarize @Cargo.toml
# Expect: Claude receives the file content and summarizes it
```

**E2. Tab completion**
```bash
# (run trogon from rsworkspace/ so relative paths work cleanly)
# type: @crates/trogon-cl   then Tab
# Expect: completes to @crates/trogon-cli/
```

**E3. Directory rejected**
```bash
# type: @crates
# Expect: "is a directory" error, not a crash
```

**E4. Path escape rejected**
```bash
# type: @../../etc/passwd
# Expect: rejected with path-escape error
```

**E5. Large file truncation**
```bash
dd if=/dev/zero bs=201K count=1 > /tmp/big.txt
# type: @/tmp/big.txt
# Expect: truncation warning, still used (truncated to 200KB)
```

---

## F. Slash Commands ✅

### Info & Navigation

**F1. `/help`** — shows all commands, no crash

**F2. `/status`**
```
# Expect: prefix (acp.claude), current model, token counts, runner list
```

**F3. `/cost`**
```
# After some messages:
# Expect: "context X/Y (Z%), ~$0.00N"
```

**F4. `/pwd` and `/cd`**
```bash
/pwd              # shows current cwd
/cd /tmp          # changes to /tmp
/pwd              # shows /tmp
/cd ~             # tilde expands
```
Then ask Claude to "list files in current directory" — should see /tmp contents.

**F5. `cd` without slash**
```bash
cd /tmp
/pwd   # shows /tmp
```

**F6. `/cd` affects tool execution**
```bash
/cd /tmp
# ask: "write a file test.txt with hello"
# Expect: /tmp/test.txt created (not original cwd)
ls /tmp/test.txt
```

### Session Control

**F7. `/clear`**
```bash
# send 3 messages then:
/clear
# Expect: new session ID shown
# ask: "what did we talk about before?"
# Expect: Claude has no memory of prior conversation
```

**F8. `/sessions` and `/resume`**
```bash
/sessions
# Expect: list with timestamps and IDs
/resume <id-from-list>
# Expect: prior conversation restored
```

**F9. `/resume` with bad ID**
```bash
/resume fake-id-00000
# Expect: graceful error, REPL stays alive
```

**F10. `/compact`**
```bash
# Have a long conversation (10+ messages)
/compact
# Expect: "compacted X → Y tokens" with Y < X
# Token bar shows lower number
# Continue chatting — context still coherent
```

### Model & Mode

**F11. `/model` — show**
```bash
/model
# Expect: current model + all available models from registry
```

**F12. `/model` — same-runner alias**
```bash
/model haiku    # switches within acp.claude runner
/model sonnet   # switches back
```

**F13. `/model` — cross-runner switch**
```bash
# On acp.claude:
# type: "remember the number 7"
/model grok
# Expect: "switching runner..." brief pause
# type: "what number did I ask you to remember?"
# Expect: Grok answers "7" — history exported + imported
/status   # shows acp.grok prefix + grok model
```
> ⚠️ **Known bug:** grok may respond as "Claude" if imported history has identity statements. Tracked for fix.

**F14. `/model <unknown>`**
```bash
/model fake-model-xyz
# Expect: "not found" error, session unchanged
```

**F15. `/mode`**
```bash
/mode               # shows current mode
/mode acceptEdits   # auto-approves file writes
/mode bypassPermissions
/mode default
/mode invalid_mode  # error listing valid modes
```

### Memory, MCP, Config

**F16. `/memory list`** — shows TROGON.md layer stack (global → project)

**F17. `/memory show`** — prints merged TROGON.md content

**F18. `/config`**
```bash
/config                       # shows ~/.config/trogon/config.json
/config set test_key hello    # stores value
/config get test_key          # returns "hello"
# Restart trogon:
/config get test_key          # still "hello" — persisted
```

**F19. `/mcp list|add|remove`**
```bash
/mcp add myfs npx -y @modelcontextprotocol/server-filesystem /tmp
/mcp list          # shows myfs active
# ask: "list files in /tmp via the MCP server"
# Expect: Claude uses MCP tool
/mcp remove myfs
/mcp list          # empty
pgrep -f "server-filesystem"   # process gone
```

**F20. `/init`**
```bash
mkdir /tmp/test-project && cd /tmp/test-project
trogon
/init
# Expect: analyzes project, generates TROGON.md
cat TROGON.md   # has project content
```

**F21. `/doctor`** — same as `trogon doctor`, green checkmarks

---

## G. Permission System ✅

**G1. Default mode — interactive prompt**
```bash
trogon   # (TROGON_MODE not set or = default)
# ask: "create a file /tmp/perm-test.txt"
# Expect: permission prompt appears
# press 'a' → file created
```

**G2. 'a' = allow once** (re-prompts on next write)
**G3. 'w' = allow always** (no prompt for same tool after)
**G4. 'r' = reject** (tool rejected, file NOT created, verify with `ls`)

**G5. Timeout (55s)**
```bash
# ask: "create /tmp/perm-timeout.txt"
# Wait 55+ seconds without pressing anything
# Expect: auto-cancel, file NOT created
```

**G6. `acceptEdits` mode**
```bash
cd rsworkspace
TROGON_MODE=acceptEdits trogon
# ask: "create accept-test.txt with content hello"
# Expect: no prompt, file created at rsworkspace/accept-test.txt
# Note: write_file is restricted to session cwd; /tmp/ paths will be rejected by the tool
```

**G7. `plan` mode**
```bash
/mode plan
# ask: "create /tmp/plan-test.txt"
# Expect: Claude describes what it WOULD do, tool NOT executed
ls /tmp/plan-test.txt   # should NOT exist
```

---

## H. Non-Interactive `--print` Mode ✅

**H1. Basic**
```bash
trogon --print "what is 2+2"
echo $?   # 0
```

**H2. Stdin pipe**
```bash
echo "what is the capital of France?" | trogon --print
```

**H3. File as stdin**
```bash
trogon --print < /tmp/prompt.txt
```

**H4. JSON output**
```bash
trogon --print "say hi" --output-format json | python3 -m json.tool
# Expect: {"text":"...","stop_reason":"end_turn"}
```

**H5. Exit code 1 on error**
```bash
trogon --print "hello" --prefix acp.nonexistent
echo $?   # 1
```

**H6. `--print-tools` (NDJSON tool lines)**
```bash
trogon --print "read the file /tmp/trogon-test.txt" \
       --print-tools --dangerously-skip-permissions
# Expect: NDJSON line {"type":"tool","name":"read_file",...} then text response
```

**H7. `--stream`**
```bash
trogon --print "count to 10 slowly" --stream
# Expect: tokens appear live (streaming)
```

---

## I. Session Persistence ✅

**I1. Resume survives restart**
```bash
trogon
# type "my secret word is BANANA", Ctrl+D
trogon --continue-session
# type "what was my secret word?"
# Expect: "BANANA"
```

**I2. Per-project isolation**
```bash
mkdir -p /tmp/proj-a /tmp/proj-b
cd /tmp/proj-a && trogon   # type "project A context", Ctrl+D
cd /tmp/proj-b && trogon
# type "what project am I in?"
# Expect: no memory of project A (fresh session)
```

**I3. sessions.json is valid JSON**
```bash
python3 -m json.tool ~/.local/share/trogon/sessions.json
```

---

## J. TROGON.md Injection ✅

**J1. Model obeys instructions**
```bash
mkdir /tmp/trogon-md-test && cd /tmp/trogon-md-test
cat > TROGON.md << 'EOF'
# Test
Always respond in ALL CAPS.
EOF
trogon
# type: "hello"
# Expect: response in ALL CAPS
```

**J2. Changes picked up on restart**
```bash
# Edit TROGON.md: "Always respond in Spanish"
# Ctrl+D, restart trogon
# type: "hello"
# Expect: response in Spanish
```

---

## K. Cross-Runner Switching ✅ (with known bug)

**K1. History survives switch**
```bash
trogon
# type: "remember: apple, banana, cherry"
/model grok
# type: "what were the three words?"
# Expect: "apple, banana, cherry"
```
> ⚠️ Known: if exported history has Claude identity statements, grok-4 may respond as "Claude".
> Workaround: start a fresh Grok session with `/clear` after switching.

**K2. Tool calls work after switch**
```bash
/model grok
# ask: "list files in current directory"
# Expect: tool executes on the grok runner
```

**K3. Switch back**
```bash
/model grok
/model sonnet     # or haiku
/status           # shows acp.claude prefix
```

---

## L. Compaction ✅

**L1. Manual compaction**
```bash
trogon
# Have a long conversation (paste a large document, ask about it)
/cost    # note token count
/compact
# Expect: "X → Y tokens" where Y < X
/cost    # lower
# Continue chatting — coherence preserved
```

**L2. Compaction doesn't lose tool context**
```bash
# Ask Claude to read a file (uses read_file tool)
/compact
# Ask about the file contents
# Expect: Claude still knows what was in the file
```

---

## M. `trogon doctor` ✅

**M1. All-green**
```bash
trogon doctor
echo $?   # 0
```

**M2. Failure detection**
```bash
pkill nats-server
trogon doctor
echo $?   # 1 — NATS check fails
# Restart: nats-server -p 4222 -js &
```

**M3. Runner down**
```bash
pkill trogon-acp-runner
trogon doctor
echo $?   # 1 — runner check fails
```

---

## N. Durability / Recovery ✅

**N1. Runner restart — clean error (no hang)**
```bash
trogon
# send a message, get response
pkill trogon-acp-runner
# send another message
# Expect: clear error message, REPL stays alive (does NOT hang)
# Restart: ./scripts/trogon-dev.sh
/clear   # new session on restarted runner
```

**N2. xai-runner session restoration**
```bash
/model grok
# type: "remember the number 99"
pkill trogon-xai-runner
# Restart xai-runner
# type: "what number did I ask you to remember?"
# Expect: "99" — session restored from KV
```

---

## O. `.env.local` Loading ✅

**O1. Picks up values**
```bash
echo 'MY_TEST_VAR=hello123' >> rsworkspace/.env.local
trogon
# run: echo $MY_TEST_VAR
# Expect: hello123
```

**O2. Does NOT override existing env**
```bash
export MY_TEST_VAR=original
trogon
# run: echo $MY_TEST_VAR
# Expect: "original" (pre-set env vars not overridden)
```

---

## P. Git & GitHub Tools ✅

Trogon exposes native git tools (`git_status`, `git_diff`, `git_log`, `git_commit`,
`git_create_branch`, `git_push`). GitHub operations use `bash` + `gh` CLI — the same
path Claude Code takes.

**Prerequisites**
```bash
# gh CLI must be installed and authenticated
gh auth status   # should show "Logged in to github.com"
```

**P1. git_status**
```bash
cd rsworkspace
trogon
# ask: "what files have uncommitted changes?"
# Expect: Claude calls git_status and lists modified files
```

**P2. git_diff**
```bash
# ask: "show me the diff of my current changes"
# Expect: Claude calls git_diff and shows the diff (truncated at 4KB)
# For staged changes:
# ask: "show me what is staged"
# Expect: Claude passes --staged to git_diff
```

**P3. git_log**
```bash
# ask: "show me the last 10 commits"
# Expect: Claude calls git_log and shows recent commits (oneline format)
```

**P4. git_commit**
```bash
# Make a throwaway change:
echo "test" > /tmp/git-tool-test.txt && cd rsworkspace && echo "# test" >> README.md
# ask: "commit README.md with message 'test: git tool'"
# Expect: Claude stages README.md and commits — no manual git commands
git log --oneline -1   # verify commit exists
git reset HEAD~1 --mixed   # undo
```

**P5. git_create_branch**
```bash
cd rsworkspace
trogon
# ask: "create a branch called test/git-tools and switch to it"
# Expect: Claude calls git_create_branch with checkout=true
git branch   # shows * test/git-tools
git checkout programming-gaps   # switch back
git branch -d test/git-tools
```

**P6. git_push (dry run)**
```bash
# ask: "what branch am I on and push it to origin with upstream tracking"
# Expect: Claude calls git_push with set_upstream=true
# Note: this will actually push — only run on a safe test branch
```

**P7. GitHub via bash — PR creation**
```bash
# (requires gh authenticated, on a branch with commits ahead of main)
# ask: "open a draft PR against main with title 'test PR' and body 'testing gh integration'"
# Expect: Claude runs `gh pr create --draft --title ... --body ...`
```

**P8. GitHub via bash — PR status**
```bash
# ask: "list open PRs in this repo"
# Expect: Claude runs `gh pr list` and shows results
```

**P9. GitHub via bash — issue list**
```bash
# ask: "show me the last 5 open issues"
# Expect: Claude runs `gh issue list --limit 5`
```

---

## Known Gaps (❌ Not Implemented)

These are Claude Code features **not built** in trogon. Not test failures — known scope gaps.

| Feature | Claude Code | Trogon |
|---------|-------------|--------|
| Hooks system | PreToolUse, PostToolUse, Stop, etc. | ❌ |
| Checkpointing | `/rewind`, file snapshots | ❌ |
| Auth | `/login`, `/logout`, OAuth | ❌ |
| GitHub integration | `/pr-comments`, `/review` | via `bash` + `gh` CLI ⚠️ |
| Vim mode | `/vim`, vi keybindings | ❌ |
| IDE management | `/ide` | ❌ |
| Plugin/skill marketplace | `/plugin install`, SKILL.md | ❌ (planned) |
| Background sessions | `/background`, remote control | ❌ |
| Batch mode | `/batch` parallel decomposition | ❌ |
| Extended thinking toggle | Alt+T | ❌ |
| Fast mode toggle | Alt+O | ❌ |
| Sandboxing | `sandbox.*` config | ❌ |
| `stream-json` output | `--output-format stream-json` | ❌ |
| System prompt override | `--system-prompt`, `--append-system-prompt` | ❌ |
| Named sessions | `--name`, `/rename` | ❌ |
| Permission rules in config | `permissions.allow/deny` patterns | ❌ |
| Auto permission mode | LLM classifier | ❌ |
| Shift+Tab mode cycling | cycle modes with keyboard | ❌ |
| `!` shell mode | run shell commands inline | ❌ |
| MCP HTTP/SSE transports | `--transport http/sse` | ❌ |
| MCP OAuth | bearer tokens, client credentials | ❌ |
| Path-scoped rules | `.trogon/rules/*.md` | ❌ |
| Auto-updater | background updates | ❌ |
| Voice dictation | hold Space to dictate | ❌ |
| Worktree management | `--worktree`, git worktrees | ❌ |
| Custom keybinding config | `~/.claude/keybindings.json` | ❌ |
| Prompt suggestions | tab to autocomplete | ❌ |

---

## Known Bugs (to fix)

1. **Rate limit not surfaced** — Sonnet/Opus rate limit causes 180s silent hang. Runner should send `StreamEvent::Error` on API failure. Workaround: use haiku or grok.
2. **Cross-runner identity contamination** — exported Claude history includes "I'm Claude" messages; grok-4 continues that persona. Fix: inject neutral system prompt on import.
3. **Nil-body cancel deserialization** — cancel with empty body causes `deserialize notification: EOF` warning in runner. Harmless but noisy.
4. **Missing X-Req-Id on `agent.load`** — causes "no reply subject" error for session load. Investigate CLI's MCP session bind path.
