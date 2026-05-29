# Trogon CLI — status (2026-05-22)

Branch: `programming-cursor`  
Canonical plan: [docs/terminal-agent-definitive-plan.md](../../docs/terminal-agent-definitive-plan.md)

## Stage 0 complete (PRs 0–3)

| PR | Title | Status |
|----|-------|--------|
| 0 | Permission UI design gate | ✅ |
| 1 | Four-runner dev stack + NATS `-js` | ✅ |
| 2 | `trogon doctor` + model aliases + default `TROGON_MODE` | ✅ |
| 3 | Docs truth | ✅ |

## What works today

- REPL, `--print`, JSON output, NATS autostart with JetStream
- Slash commands: `/help`, `/cost`, `/clear`, `/compact`, `/config`, `/model`, `/init`
- Same-prefix `/model` (`set_model` on live session)
- Cross-runner `/model` export/import (needs LocalSet + ACP client — Stage 1)
- Four-runner dev launcher: `./scripts/trogon-dev.sh` or `trogon dev`
- Stack health: `trogon doctor` / `trogon --doctor`
- Default permission bridge: `TROGON_MODE=acceptEdits` on new sessions
- Model aliases: `sonnet`, `grok`, `openrouter`, `codex`, `o3`, `gpt-4o`, …

## Known gaps (Stage 1+)

| Area | Symptom | Fix stage |
|------|---------|-----------|
| Permission prompts | ~60s timeout on write/bash | Stage 1 — ACP client + `/dev/tty` |
| Cross-runner `/model` | `Bridge` `!Send`; no `set_model` after switch | Stage 1 |
| Tool output | Bash stdout not shown in REPL | Stage 2 — `ToolCallUpdate` |
| `/compact` | Wrong NATS subject + payload | Stage 2 |
| Session resume | No `--continue` | Stage 3 |
| MCP from CLI | Bridge exists, not wired | Stage 4 |

## Tests

```bash
cd rsworkspace
cargo test -p trogon-cli --lib    # ~143 unit tests (10 cross_runner need Docker)
cargo build -p trogon-cli
```

Cross-runner integration tests in `cross_runner.rs` require Docker (`testcontainers`); they fail without `/var/run/docker.sock`.

## Dev quick start

```bash
cd rsworkspace
cp .env.local.example .env.local   # fill API keys
cargo build --release -p trogon-cli -p trogon-acp-runner -p trogon-xai-runner \
  -p trogon-openrouter-runner -p trogon-codex-runner -p trogon-wasm-runtime -p trogon-compactor
./scripts/trogon-dev.sh
./target/release/trogon doctor
cd /your/project && ./target/release/trogon
```
