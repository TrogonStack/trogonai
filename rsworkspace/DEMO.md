# TrogonAI — Working Demo

> **What this is:** TrogonAI is an event-driven agentic system that listens to webhooks from GitHub, Linear, Slack, and others, then runs Claude to take autonomous actions — code reviews, issue triage, on-call escalation, and more.

---

## Architecture

```
GitHub / Linear / Slack
        │
        ▼  (HTTPS webhook)
┌───────────────────┐
│  trogon-gateway   │  ← validates signature, routes
│  (port 8081)      │    to the right source handler
└────────┬──────────┘
         │ NATS JetStream publish
         ▼
    ┌─────────┐         durable, replayable
    │  NATS   │  ←────  event bus (JetStream)
    └────┬────┘
         │ pull consumer
         ▼
┌────────────────────┐
│   trogon-agent     │  ← agentic loop:
│   (Claude loop)    │    reads event → calls Claude →
└────────┬───────────┘    uses tools (GitHub API, Linear API)
         │
         ▼  (opaque tok_ token)
┌─────────────────────┐
│ trogon-secret-proxy │  ← resolves tok_xxx → real API key
│    + worker         │    agent never sees real credentials
└────────┬────────────┘
         │  real API key
         ▼
   api.github.com
   api.anthropic.com
   api.linear.app
```

**Key properties:**
- **Durable delivery**: events survive agent restarts (JetStream)
- **Exactly-once processing**: promise store prevents duplicate runs
- **Credential isolation**: real API keys never reach the agent process
- **Hot-reload**: automations can be added/changed without restart

---

## What the demo shows

A single `curl` command triggers the full pipeline:

1. Webhook arrives at `trogon-gateway` → HMAC signature verified → **200 OK**
2. GitHub PR event published to NATS JetStream stream `GITHUB`
3. Agent picks it up, extracts PR metadata (owner, repo, number, title, diff URL)
4. Agent calls Claude with a code review prompt
5. Claude generates an actionable review
6. (With GitHub token) Agent posts the review comment back to the PR

Steps 1–4 are visible in real-time in the terminal. Step 5 requires a valid Anthropic key.

---

## Requirements

| Requirement | Notes |
|-------------|-------|
| **Docker** | For the NATS message broker |
| **`ANTHROPIC_API_KEY`** | The only external secret needed for the base demo |
| **GitHub token** *(optional)* | Enables reading PR diff + posting review comments back |
| **Linear token** *(optional)* | Enables reading issue details + posting updates back |

The Rust binaries are pre-built in `target/debug/`.

---

## Running the demo

### 1. Start NATS

```bash
docker run -d --name trogon-nats -p 4222:4222 nats:2.10-alpine --jetstream
```

### 2. Start the proxy (detokenization layer)

```bash
NATS_URL="nats://127.0.0.1:4222" \
  PROXY_PREFIX="trogon" \
  PROXY_PORT="8080" \
  RUST_LOG="info" \
  ./target/debug/proxy &
```

### 3. Start the vault worker (with your Anthropic key)

```bash
NATS_URL="nats://127.0.0.1:4222" \
  PROXY_PREFIX="trogon" \
  WORKER_CONSUMER_NAME="proxy-workers" \
  VAULT_TOKEN_tok_anthropic_prod_demo01="sk-ant-YOUR_KEY_HERE" \
  VAULT_TOKEN_tok_github_prod_demo01="ghp-placeholder" \
  VAULT_TOKEN_tok_linear_prod_demo01="lin-placeholder" \
  RUST_LOG="info" \
  ./target/debug/worker &
```

### 4. Start the agent

```bash
NATS_URL="nats://127.0.0.1:4222" \
  PROXY_URL="http://localhost:8080" \
  ANTHROPIC_TOKEN="tok_anthropic_prod_demo01" \
  GITHUB_TOKEN="tok_github_prod_demo01" \
  LINEAR_TOKEN="tok_linear_prod_demo01" \
  TENANT_ID="demo" \
  AUTOMATIONS_API_PORT="8090" \
  RUST_LOG="info" \
  ./target/debug/agent &
```

### 5. Start the gateway (webhook receiver)

```bash
NATS_URL="nats://127.0.0.1:4222" \
  TROGON_GATEWAY_PORT="8081" \
  TROGON_SOURCE_GITHUB_WEBHOOK_SECRET="demo-webhook-secret" \
  RUST_LOG="info" \
  ./target/debug/trogon-gateway serve &
```

Wait ~5 seconds for all services to connect to NATS.

### 6. Send a GitHub PR webhook

```bash
BODY='{
  "action": "opened",
  "number": 42,
  "pull_request": {
    "number": 42,
    "title": "Add rate limiting middleware to API endpoints",
    "body": "Adds token-bucket rate limiter. 100 req/min per IP. Returns 429 with Retry-After on breach.",
    "user": { "login": "alice-dev" },
    "base": { "ref": "main" },
    "head": { "ref": "feat/rate-limiting" },
    "html_url": "https://github.com/demo-org/backend-api/pull/42",
    "additions": 87, "deletions": 3, "changed_files": 4
  },
  "repository": {
    "full_name": "demo-org/backend-api",
    "name": "backend-api",
    "owner": { "login": "demo-org" }
  },
  "sender": { "login": "alice-dev" }
}'

SIG=$(echo -n "$BODY" | openssl dgst -sha256 -hmac "demo-webhook-secret" | awk '{print "sha256="$2}')

curl -X POST http://localhost:8081/github/webhook \
  -H "Content-Type: application/json" \
  -H "X-Hub-Signature-256: $SIG" \
  -H "X-GitHub-Event: pull_request" \
  -H "X-GitHub-Delivery: demo-001" \
  -d "$BODY"
```

### 7. Watch the agent process it

```bash
tail -f /tmp/trogon-agent.log
```

You will see:

```
INFO  Starting PR review agent  owner="demo-org" repo="backend-api" pr_number=42
INFO  Claude response: [code review output]
```

---

## What you observe (terminal split view)

| Left: agent log | Right: curl |
|---|---|
| Agent runner started | `curl` → HTTP 200 |
| Starting PR review agent | ← event arrived in <100ms |
| [Claude generates review] | |
| PR review complete | |

---

## What's not in this demo (and why)

| Missing | Why | What it takes |
|---------|-----|---------------|
| Review comment posted to GitHub | No GitHub token | Add `ghp-xxx` to worker env |
| Linear issue triage | No Linear token | Add `lin-xxx` to worker env |
| Discord / Slack / Telegram | Need real tokens | Same pattern |
| Production hardening | Single-node NATS | 3-node cluster for HA |

---

## The bottom line

> The hard part is built. Webhook ingestion, event routing, durable delivery, credential isolation, and the Claude agentic loop are all running. Adding write-back to GitHub or Linear is **configuration, not code** — provide the API keys and it works.

---

## Repo

All source code, tests, and this demo are in the monorepo at `rsworkspace/`.

Integration tests: `cargo test -p trogon-gateway --test routes_e2e` (requires Docker).
