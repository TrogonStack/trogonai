# Trogon CLI — Live & Manual Test Launch Guide

This guide walks any user through launching the Trogon CLI on their own machine and
exercising it by hand — from a cold checkout to a working REPL talking to a real
model, plus a checklist of every feature worth poking at.

Trogon is a terminal coding agent (a Claude Code–style REPL). It talks to model
runners over **NATS JetStream** — there is no direct HTTP between the CLI and the
runners. The local dev stack starts NATS, an execution backend, a compactor, and one
runner per provider you have credentials for.

```
trogon (REPL)  ──NATS──▶  acp-nats bridge  ──▶  runner (claude / grok / …)  ──▶  provider API
```

---

## 1. Prerequisites

You need these on your `PATH`:

| Tool          | Why                                    | Check                      |
|---------------|----------------------------------------|----------------------------|
| `cargo` / Rust | builds the binaries                    | `cargo --version`          |
| `nats-server` | the message bus (must support `-js`)   | `nats-server --version`    |
| `nc` (netcat) | the dev script uses it for port checks | `which nc`                 |
| `bash`        | runs `trogon dev` / `trogon-dev.sh`    | `bash --version`           |

And **at least one model credential**:

- `ANTHROPIC_TOKEN` — Claude runner (`acp.claude`). Recommended; also powers the
  compactor and `/init`.
- `XAI_API_KEY` — Grok runner (`acp.grok`).
- `OPENROUTER_API_KEY` — OpenRouter runner (`acp.openrouter`).
- Codex CLI on PATH + `CODEX_ENABLED=1` — Codex runner (`acp.codex`).

> With **no** credentials you can still build, run `trogon doctor`, and exercise
> `--print` error paths, but you cannot get a model reply. Start with `ANTHROPIC_TOKEN`.

---

## 2. One-time setup

> **Paths in this guide** use `$RSWORKSPACE` for the `rsworkspace` directory of your
> checkout. Set it once per shell so every command below works as-is:
>
> ```bash
> export RSWORKSPACE="$(git rev-parse --show-toplevel)/rsworkspace"   # run from inside the repo
> # …or point it at your checkout explicitly:
> # export RSWORKSPACE=/path/to/trogonai/rsworkspace
> ```

```bash
cd "$RSWORKSPACE"

# 1. Create your local env file and fill in at least one credential
cp .env.local.example .env.local
$EDITOR .env.local        # set ANTHROPIC_TOKEN=... (and/or XAI_API_KEY, etc.)

# 2. Build the release binaries the dev stack expects
cargo build --release \
  -p trogon-cli \
  -p trogon-acp-runner \
  -p trogon-xai-runner \
  -p trogon-openrouter-runner \
  -p trogon-codex-runner \
  -p trogon-wasm-runtime \
  -p trogon-compactor
```

> **Build gotcha:** don't run multiple `cargo` commands against this shared
> `target/` at the same time — concurrent builds corrupt artifacts (linker
> "undefined reference" errors). Let the one build finish.

Key `.env.local` knobs:

```bash
NATS_URL=nats://localhost:4222
ANTHROPIC_TOKEN=...                 # Claude + compactor + /init
ANTHROPIC_BASE_URL=https://api.anthropic.com/v1   # direct mode; omit for proxy
XAI_API_KEY=...                     # optional, enables Grok
OPENROUTER_API_KEY=...              # optional
CODEX_ENABLED=0                     # set 1 (and have Codex CLI) to enable
TROGON_MODE=default                 # default | acceptEdits | bypassPermissions | …
ACP_PREFIX=acp.claude               # which runner the CLI attaches to by default
```

---

## 3. Launch the stack

One command brings up everything you have credentials for:

```bash
./target/release/trogon dev
# (equivalent: ./scripts/trogon-dev.sh  — legacy: ./dev-runner.sh)
```

It is idempotent — it skips anything already running. Expected output names each
service it starts:

```
Starting nats-server -js
Starting trogon-wasm-runtime (prefix=acp.wasm)
Starting trogon-compactor
Starting trogon-acp-runner (prefix=acp.claude)
Starting trogon-xai-runner (prefix=acp.grok)        # only if XAI_API_KEY set
...
Trogon dev stack started. Logs: /tmp/trogon-*.log
```

What starts, and what each needs:

| Service                  | Prefix / subject              | Requires                  |
|--------------------------|-------------------------------|---------------------------|
| NATS + JetStream         | `:4222`                       | always                    |
| `trogon-wasm-runtime`    | `acp.wasm`                    | always (runs `bash`)      |
| `trogon-compactor`       | `trogon.compactor.compact`    | `ANTHROPIC_TOKEN`         |
| `trogon-acp-runner`      | `acp.claude`                  | `ANTHROPIC_TOKEN`         |
| `trogon-xai-runner`      | `acp.grok`                    | `XAI_API_KEY`             |
| `trogon-openrouter-runner` | `acp.openrouter`            | `OPENROUTER_API_KEY`      |
| `trogon-codex-runner`    | `acp.codex`                   | `CODEX_ENABLED=1` + CLI   |

> If you prefer to start things by hand: `nats-server -p 4222 -js` in one terminal,
> then run each runner binary with its `ACP_PREFIX` and credentials as env vars (see
> `scripts/trogon-dev.sh` for the exact invocations).

---

## 4. Verify the stack — `doctor`

Before opening the REPL, confirm everything is healthy:

```bash
./target/release/trogon doctor      # or: trogon --doctor
```

`doctor` checks, in order: NATS TCP reachability, JetStream KV, the agent registry,
which runner prefixes are registered, the execution backend, a live session smoke
test, the compactor (publishes to `trogon.compactor.compact`), the Codex CLI, and
warns about missing tokens. Each line is **PASS / FAIL / WARN / SKIP**:

- **FAIL** on "NATS TCP" → NATS isn't up: `nats-server -p 4222 -js`.
- **FAIL** on Registry → NATS started without `-js` (JetStream).
- **WARN** on tokens → that provider's runner just won't be available; fine if intended.
- Exit code is `0` when all *required* checks pass (warnings are OK).

You can re-run these checks any time from inside the REPL with `/doctor`.

---

## 5. Start the CLI

Run it **from the project directory you want the agent to work in** — that's its
working dir for file tools and `TROGON.md` discovery:

```bash
cd /path/to/your/project
"$RSWORKSPACE/target/release/trogon"
```

> Tip: add the binary to your `PATH` (`export PATH="$RSWORKSPACE/target/release:$PATH"`)
> and you can just type `trogon` from anywhere.

Useful startup flags:

| Flag                          | Effect                                                        |
|-------------------------------|---------------------------------------------------------------|
| `--prefix acp.grok`           | attach to a different runner (default `acp.claude`)           |
| `--model sonnet`              | (with `--print`) pick a model; routes to the right runner     |
| `--continue`                  | resume the last session for this project                      |
| `--session-id <id>`           | attach to a specific session (uses `--prefix` runner)         |
| `--stream`                    | stream assistant text live instead of buffering to tool boundaries |
| `--nats-url <url>`            | point at a non-default NATS                                   |
| `--dangerously-skip-permissions` | start in `bypassPermissions` (use with care)              |

Then just type a prompt, e.g. `list the files in this directory and summarize the README`.
The agent will call tools (read/write files, run bash, fetch URLs) and stream a reply.

- **Ctrl+C** cancels the active response.
- **Ctrl+D** quits (cleanly closes the session and kills any auto-started NATS).
- End a line with `\` to continue on the next line (multiline input).

---

## 6. Manual test checklist

Work through these inside the REPL. Type `/help` first to see the live command list.

### Core conversation
- [ ] Send a simple prompt; confirm you get a streamed reply.
- [ ] Ask it to **read a file** (e.g. "show me the top of Cargo.toml") → file tool runs.
- [ ] Ask it to **create/edit a file** → triggers a permission prompt in `default` mode.
- [ ] Ask it to **run a bash command** (e.g. "run `ls -la`") → executes in the wasm runtime.
- [ ] `@`-mention a file in your prompt (e.g. `explain @src/main.rs`) → file contents are inlined.

### Permission modes — `/mode`
- [ ] `/mode` lists modes and marks the current one (`▸`):
  - `default` — auto-allow reads; prompt for edits, bash, MCP
  - `acceptEdits` — also auto-allow file edits
  - `plan` — read-only exploration; deny writes & bash
  - `dontAsk` — auto-allow everything (still audited)
  - `bypassPermissions` — no checks at all
- [ ] `/mode acceptEdits` then ask for an edit → no prompt.
- [ ] `/mode plan` then ask it to write a file → refused.

### Models — `/model`
- [ ] `/model` with no args → lists models from the live registry.
- [ ] `/model haiku` → switches to a cheaper Claude model (same runner).
- [ ] `/model grok` → **cross-runner** switch: exports history, opens a session on
      `acp.grok`, imports it. Requires `XAI_API_KEY`. Confirm the conversation continues.
      Aliases: `sonnet`, `opus`, `haiku`, `grok`, `grok-mini`, `op-claude`, `op-gpt`,
      `op-gemini`, `codex`, `o3`, `gpt-4o`.
- [ ] `/status` → shows prefix, current model, token usage, and registered runners.

### Sessions
- [ ] `/sessions` → lists sessions on the current runner.
- [ ] `/clear` → starts a fresh session (history cleared).
- [ ] Note a session id, `/clear`, then `/resume <id>` → history comes back.
- [ ] Quit, relaunch with `--continue` → last session for this project resumes.

### Context compaction — `/compact`
- [ ] `/compact` → forces compaction now; prints `compacted: N → M tokens`
      (or "no compaction needed"). Requires the compactor + `ANTHROPIC_TOKEN`.
- [ ] `/compact-model` → lists this runner's models and the current compaction model.
- [ ] `/compact-model haiku` → set a cheaper compaction model; `/compact-model default` resets.
- [ ] `/cost` → context tokens used / window size, % and rough $ estimate.

### Project memory — `/memory` & `/init`
- [ ] `/memory list` → shows the `TROGON.md` hierarchy (global → project) and which exist.
- [ ] `/init` → AI-analyzes the project and writes a `TROGON.md` (uses the Claude runner
      via an ephemeral session regardless of your active model). `/init --force` overwrites.
- [ ] `/memory show` → prints the project `TROGON.md`; `/memory edit` → prints the path/editor hint.

### MCP & misc
- [ ] `/mcp list` → shows configured MCP server bridges; `/mcp add` / `/mcp remove` to manage.
- [ ] `/config` → shows config; `/config set <key> <value>` to change.
- [ ] `/cd <path>` (or bare `cd <path>`) → change working directory; `/pwd` to confirm.
- [ ] `/doctor` → re-runs health checks against the URL the CLI is actually connected to.
- [ ] `/help` lists everything above; `/bogus` → "unknown command" hint.

---

## 7. Non-interactive (`--print`) mode

For scripting and quick one-shots — no REPL, prints the result and exits:

```bash
# prompt as an argument
trogon --print "what is 2 + 2?"

# prompt from stdin (note: -p with no value reads stdin)
echo "explain this error" | trogon --print
trogon --print < notes.txt

# JSON output: one line {"text":"...","stop_reason":"..."}
trogon --print "summarize the README" --output-format json

# emit NDJSON tool lines as tools run
trogon --print "list files" --print-tools

# pick a model (routed to the right runner automatically)
trogon --print "hi" --model grok

# fully unattended (skip permission gates)
trogon --print "refactor foo.rs" --dangerously-skip-permissions
```

Exit code reflects the stop reason (non-zero on an error stop), so `--print` is safe
to use in shell pipelines and CI.

---

## 8. Troubleshooting

**Logs** — every service logs to `/tmp/trogon-*.log` (override with `TROGON_LOG_DIR`):

```bash
tail -f /tmp/trogon-nats.log /tmp/trogon-acp.log /tmp/trogon-compactor.log
```

| Symptom                                   | Likely cause / fix                                            |
|-------------------------------------------|---------------------------------------------------------------|
| `doctor` FAIL on NATS TCP                 | NATS not running → `nats-server -p 4222 -js`                  |
| `doctor` FAIL on Registry                 | NATS started without `-js`; restart with JetStream            |
| `/model grok` says unknown model          | `XAI_API_KEY` unset, or xai runner not started — check doctor |
| Replies hang / no model output            | runner for your `--prefix` isn't up; check its `/tmp/*.log`   |
| `/compact` errors                         | compactor not running (needs `ANTHROPIC_TOKEN`)               |
| "trogon dev script not found"             | run from `rsworkspace`, or set `TROGON_DEV_SCRIPT=<path>`      |
| Edits/bash silently auto-run              | you're in `acceptEdits`/`bypass` mode — check `/mode`         |
| Build "undefined reference" errors        | concurrent cargo builds against shared `target/`; rebuild serially |

---

## 9. Shutdown

- In the REPL: **Ctrl+D** quits and closes the session.
- The background stack keeps running. To stop everything:

```bash
pkill -f 'release/trogon-'    # runners, compactor, wasm-runtime
pkill -f 'nats-server'        # only if the dev script started it for you
```

> Re-running `trogon dev` is safe at any time — it detects what's already up and
> only starts what's missing.

---

### Quick reference — fastest happy path

```bash
export RSWORKSPACE="$(git rev-parse --show-toplevel)/rsworkspace"   # or /path/to/trogonai/rsworkspace
cd "$RSWORKSPACE"
cp .env.local.example .env.local && $EDITOR .env.local   # set ANTHROPIC_TOKEN
cargo build --release -p trogon-cli -p trogon-acp-runner \
  -p trogon-wasm-runtime -p trogon-compactor
./target/release/trogon dev
./target/release/trogon doctor
cd /path/to/your/project
"$RSWORKSPACE/target/release/trogon"
```
