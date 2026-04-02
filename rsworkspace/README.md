# TrogonAI — Slack Integration

A production-ready Slack ↔ Claude integration built on NATS JetStream.

## Architecture

```
Slack ── Socket Mode ── slack-bot ── NATS JetStream ── slack-agent ── Claude API
```

Two independent services communicate through NATS:

- **slack-bot** — connects to Slack via Socket Mode, bridges all events to/from NATS JetStream
- **slack-agent** — consumes events from NATS, calls Claude, streams responses back to Slack

Conversation history is stored per-session in a NATS JetStream KV bucket with a 30-day TTL.

## Quick Start

### 1. Create the Slack App

1. Go to <https://api.slack.com/apps> → **Create New App → From manifest**
2. Upload `slack-app-manifest.yml` from this directory
3. Install the app to your workspace
4. Note the **Bot Token** (`xoxb-…`) and the **App-Level Token** (`xapp-…`)
5. Copy your bot's Member ID from the app's *Basic Information* page (optional but recommended)

### 2. Configure environment

```bash
cp .env.example .env
# Fill in SLACK_BOT_TOKEN, SLACK_APP_TOKEN, ANTHROPIC_API_KEY, NATS_URL
```

### 3. Run with Docker Compose

```bash
docker compose up --build
```

This starts NATS (with JetStream), `slack-bot`, and `slack-agent` as a single stack.

## Running Locally (without Docker)

```bash
# 1. Start NATS with JetStream
docker run --rm -p 4222:4222 nats:2.10-alpine -js

# 2. Load environment
set -a && source .env && set +a

# 3. Run each service in a separate terminal
RUST_LOG=info cargo run -p slack-bot
RUST_LOG=info cargo run -p slack-agent
```

## Configuration Reference

### slack-bot

| Variable | Default | Description |
|---|---|---|
| `SLACK_BOT_TOKEN` | **required** | Bot OAuth token (`xoxb-…`) |
| `SLACK_APP_TOKEN` | **required** | App-level token for Socket Mode (`xapp-…`) |
| `SLACK_BOT_USER_ID` | — | Bot's Slack member ID — filters the bot's own messages |
| `SLACK_MENTION_GATING` | `true` | Require `@mention` in channels; DMs always pass through |
| `HEALTH_PORT` | `8080` | HTTP health-check port (`GET /health`) |
| `NATS_URL` | `nats://localhost:4222` | NATS server URL |
| `NATS_TOKEN` | — | NATS auth token (mutually exclusive with `NATS_CREDS`) |
| `NATS_CREDS` | — | Path to a NATS `.creds` file |

### slack-agent

| Variable | Default | Description |
|---|---|---|
| `ANTHROPIC_API_KEY` | — | Enables Claude; without it the agent echoes messages |
| `CLAUDE_MODEL` | `claude-sonnet-4-6` | Claude model ID |
| `CLAUDE_MAX_TOKENS` | `8192` | Maximum tokens per response |
| `CLAUDE_SYSTEM_PROMPT` | — | System prompt (inline text) |
| `CLAUDE_SYSTEM_PROMPT_FILE` | — | Path to a file whose contents become the system prompt (takes precedence over inline) |
| `CLAUDE_MAX_HISTORY` | `40` | Conversation turns kept in KV per session |
| `SLACK_REPLY_TO_MODE` | `off` | Threading: `off` · `first` · `all` |
| `SLACK_ACK_REACTION` | — | Emoji added while processing, removed on completion (e.g. `eyes`) |
| `SLACK_WELCOME_MESSAGE` | — | Message posted to a channel when a new member joins |
| `HEALTH_PORT` | `8081` | HTTP health-check port (`GET /health`) |
| `NATS_URL` | `nats://localhost:4222` | NATS server URL |

## Slash Commands

| Command | Description |
|---|---|
| `/trogon clear` | Clear conversation history for the current session |
| `/trogon <text>` | One-shot query to Claude (not saved to history) |

## Features

- **Streaming responses** — Claude's output is streamed word-by-word via `chat.update` so users see text as it's generated
- **Conversation memory** — Per-session history stored in NATS KV; edited and deleted messages are reflected in history
- **File content** — Text files shared in Slack (≤ 50 KB) are downloaded and included in the prompt so Claude can read them
- **Mention gating** — In channels, the bot only responds when @mentioned (configurable)
- **Threading** — Replies can be threaded automatically (`first` = thread the first reply and continue in thread; `all` = always thread)
- **Ack reaction** — Adds an emoji reaction while processing to give immediate visual feedback
- **Rate limit handling** — Automatically retries Slack API calls on `ratelimited` responses
- **Graceful shutdown** — Waits up to 30 s for in-flight Claude calls to finish before exiting

## Development

```bash
# Run all tests
cargo test --workspace

# Lint
cargo clippy --all-targets -- -D warnings

# Format
cargo fmt --all
```

## Health Checks

Both services expose `GET /health` → `200 OK` with body `ok`.

- `slack-bot`: `http://localhost:8080/health`
- `slack-agent`: `http://localhost:8081/health`
