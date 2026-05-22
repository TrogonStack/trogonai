# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test

```bash
# Build everything
cargo build

# Build a specific crate
cargo build -p trogon-cli
cargo build -p trogon-acp-runner

# Build release binaries (used for dev-runner.sh)
cargo build --release -p trogon-acp-runner -p trogon-xai-runner -p trogon-cli

# Run all tests (unit only, no NATS required)
cargo test

# Run tests for a specific crate
cargo test -p trogon-cli
cargo test -p trogon-acp-runner

# Run integration tests (requires a NATS server with JetStream)
NATS_TEST_URL=nats://localhost:4222 cargo test -p trogon-cron -- --include-ignored --test-threads=1

# Lint (workspace enforces warnings-as-errors and clippy::all-as-errors)
cargo clippy
cargo fmt --check
```

Workspace enforces `warnings = "deny"` and `clippy::all = "deny"` globally. All clippy warnings must be fixed before committing.

## Running Locally

### One command (recommended)

```bash
cd rsworkspace
cp .env.local.example .env.local   # fill ANTHROPIC_TOKEN, XAI_API_KEY, etc.
cargo build --release -p trogon-cli -p trogon-acp-runner -p trogon-xai-runner \
  -p trogon-openrouter-runner -p trogon-codex-runner -p trogon-wasm-runtime -p trogon-compactor
./scripts/trogon-dev.sh            # or: ./target/release/trogon dev
./target/release/trogon doctor       # or: trogon --doctor
```

`trogon-dev.sh` starts (when credentials are set):

| Service | Prefix / subject | Requires |
|---------|------------------|----------|
| NATS + JetStream | `:4222` | always |
| `trogon-wasm-runtime` | `acp.wasm` | always (bash) |
| `trogon-compactor` | `trogon.compactor.compact` | `ANTHROPIC_TOKEN` |
| `trogon-acp-runner` | `acp.claude` | `ANTHROPIC_TOKEN` |
| `trogon-xai-runner` | `acp.grok` | `XAI_API_KEY` |
| `trogon-openrouter-runner` | `acp.openrouter` | `OPENROUTER_API_KEY` |
| `trogon-codex-runner` | `acp.codex` | `CODEX_ENABLED=1` + Codex CLI |

Legacy wrapper: `./dev-runner.sh` → `scripts/trogon-dev.sh`.

### Manual start

Start NATS with JetStream:
```bash
nats-server -p 4222 -js
```

Run the CLI from a project directory:
```bash
cd /your/project
/path/to/rsworkspace/target/release/trogon
```

Default permission mode for new sessions: `TROGON_MODE=acceptEdits` (see `.env.local.example`). Interactive permission UI is Stage 1 (`docs/permission-ui-design.md`).

## Architecture

The system is built on **NATS JetStream as the only transport**. All components communicate through NATS subjects — no direct HTTP between internal services.

### Protocol layers

```
trogon-cli (terminal REPL)
    │  NATS request-reply / pub-sub
    ▼
acp-nats (Bridge) — implements Agent Client Protocol over NATS
    │  NATS subject: {prefix}.session.{id}.agent.*
    ▼
trogon-acp-runner / trogon-xai-runner / trogon-codex-runner / trogon-openrouter-runner
    │  HTTP (via trogon-secret-proxy) or subprocess
    ▼
Anthropic / xAI / OpenAI API
```

### Key crates

**`acp-nats`** — ACP-over-NATS transport. `Bridge<N,C,J>` is `!Send` (contains `RefCell`); must run inside `tokio::task::LocalSet`. The `Bridge` is the client-side implementation; runner-side uses `acp-nats-agent`.

**`trogon-acp-runner`** — Claude runner. Owns the agentic loop (`trogon-agent-core`), session state (NATS KV via `trogon-runner-tools::NatsSessionStore`), bash execution (`trogon-wasm-runtime`), MCP server connections, and permission gates.

**`trogon-xai-runner`** — Grok runner. Uses xAI Responses API (`previous_response_id` for stateful conversations). Sessions are in-memory.

**`trogon-agent-core`** — The tool-use loop: send messages → parse `tool_use` blocks → dispatch → append results → repeat until `end_turn`. All tool dispatch goes through `trogon-tools::dispatch_tool`.

**`trogon-tools`** — All tool implementations: `fs` (read/write/list/glob), `editor` (str_replace), `git`, `web` (fetch_url), `search`, `todo`. `all_tool_defs()` returns the full list with prompt-cache markers.

**`trogon-runner-tools`** — Shared runner utilities: `NatsSessionStore` (session persistence in NATS KV), `WasmRuntimeBashTool` (stateful bash via wasm-runtime terminals), `TrogonMdLoading` (TROGON.md injection), `PermissionChecker` (approval gates), `EgressPolicy` (network allowlist).

**`trogon-registry`** — Live agent registry in NATS KV. Agents self-register at startup with `AgentCapability` (capabilities list, model list, NATS prefix). TTL = 30s, heartbeat = 15s. `find_by_model(model_id)` used by `CrossRunnerSwitcher`.

**`trogon-cli`** — Terminal REPL. `CrossRunnerSwitcher` handles `/model` cross-runner switches: exports history via `session/export` ext_method, opens new session on target runner, imports via `session/import`. `/init` always uses the startup runner (Claude) via an ephemeral session, never the currently active model.

**`trogon-compactor`** — Context compaction. Triggered at 85% of the context window. Summarizes the oldest portion of history into a structured checkpoint.

**`trogon-wasm-runtime`** — WASM-based bash execution environment. Terminals are stateful (persist `cd`, env vars across calls). The bash tool reuses the same terminal per session via `terminal_id`.

### NATS subject conventions

- Agent requests: `{prefix}.session.{session_id}.agent.{method}`
- Session notifications: `{prefix}.session.{session_id}.client.session.update`
- Registry: `trogon.registry.agents` (NATS KV bucket)
- Compactor: `trogon.compactor.compact` (global; not per-prefix)

### Cross-runner model switching

`CrossRunnerSwitcher::switch_model` in `trogon-cli/src/cross_runner.rs`:
1. Looks up target runner via `registry.find_by_model(model_id)`
2. Calls `session/export` ext_method on current runner → raw JSON `Vec<PortableMessage>`
3. Opens new session on target runner
4. Calls `session/import` ext_method with the exported messages
5. Returns new `(prefix, session_id)` — REPL replaces its state

## Code Conventions (from AGENTS.md)

- **Enums over booleans** at system edges (HTTP, NATS, databases)
- **Domain value objects** over primitives: `AcpPrefix` not `String`, `AcpSessionId` not `String`. Each lives in its own file (`acp_prefix.rs`, `session_id.rs`). Invalid instances must be unrepresentable at construction.
- **Typed errors**: structs or enums, never `String`. Implement `Display + std::error::Error`. Never convert a typed error to string to discard context.
- **Wire/input types**: untrusted input uses `*Request`/`*Wire` types; convert to domain types exactly once. Never persist wire types.
- **Test helpers**: share via `test-support` feature, not copy-paste.
- **Infrastructure traits**: production impls are zero-cost passthroughs — `self.sdk.method(args).await` with no wrapping. Conversion logic belongs in the caller.
- **Observability**: metrics, tracing, and logging go under a `telemetry` module within each crate.
- **NATS infrastructure**: always use `trogon-nats` crate for connection management, retry policies, and mock clients in tests.
