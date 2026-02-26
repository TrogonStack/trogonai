# AI Agent Infrastructure Landscape — February 2026

> Research report covering: OpenFang, GitNexus, awesome-claws / OpenClaw ecosystem, Spacebot

---

## 1. Overview

Four projects were studied. Together they paint a clear picture of where the AI agent
infrastructure space is in early 2026 and where it is heading.

| Project | Category | Language | Maturity |
|---|---|---|---|
| OpenFang | Full-stack agent OS | Rust | Production (~137k LOC, 1,767+ tests) |
| GitNexus | Code intelligence layer | TypeScript + WASM | Early/growing |
| OpenClaw / awesome-claws | Personal agent framework + ecosystem | TypeScript (ref), many ports | Mainstream (100k+ ⭐) |
| Spacebot | Team-oriented agent OS | Rust | Active development |

---

## 2. OpenFang — Agent Operating System

### What it is

A 14-crate Rust workspace that compiles to a single ~32 MB self-contained binary.
It is the most complete "agent OS" available today in open source.

### Core architecture

```
openfang-kernel     orchestration, workflow engine, scheduling, RBAC, budget tracking
openfang-runtime    agent loop, 26 LLM providers, 53 tools, WASM sandbox
openfang-memory     SQLite persistence, semantic recall, vector embeddings, compaction
openfang-wire       OFP peer-to-peer protocol with HMAC-SHA256 mutual auth
openfang-hands      schedule-driven autonomous capability packages ("Hands")
openfang-channels   40 messaging platform adapters
openfang-skills     60 bundled skills + ClawHub-compatible registry
openfang-extensions 25 MCP server templates, AES-256-GCM credential vault
openfang-api        140+ REST/WS/SSE endpoints, OpenAI-compatible API
openfang-cli        CLI + TUI + MCP server mode
openfang-desktop    Tauri 2.0 native app
openfang-types      core types, taint tracking, Ed25519 manifest signing
openfang-migrate    import from OpenClaw, LangChain, AutoGPT
```

### The "Hands" abstraction (key insight)

A Hand is a **pre-built, schedule-driven, autonomous capability package**.
Each bundles:
- `HAND.toml` manifest
- Multi-phase system prompt playbook
- `SKILL.md` expert knowledge
- Configurable settings
- Dashboard metrics

All compiled into the binary at build time. The 7 built-in Hands:
`Clip` (video-to-shorts), `Lead` (lead generation), `Collector` (target monitoring),
`Predictor` (probabilistic forecasting with Brier scores), `Researcher` (fact-checking),
`Twitter` (X account management), `Browser` (web automation).

The key property: **they run on schedules without human prompting, 24/7**.

### Protocol support

- **MCP** — both client (JSON-RPC 2.0 over stdio/SSE, with tool namespacing) and
  server (exposes OpenFang tools to external MCP consumers), 25 server templates
- **Google A2A** — agent-to-agent task delegation, both client and server roles
- **OFP (OpenFang Protocol)** — custom P2P protocol over openfang-wire for
  authenticated multi-node agent communication
- **OpenAI-compatible API** — allows existing tooling to target OpenFang agents

### Security model

16 independently-testable layers: WASM sandbox, Merkle audit trail, taint tracking,
Ed25519 signed identities, SSRF guard, memory zeroization (Zeroizing<String>), RBAC,
HTTP security headers, path traversal prevention, prompt injection scanner, subprocess
isolation, GCRA rate limiting.

### WASM sandbox

Wasmtime with **dual metering**: fuel metering (instruction budget) + epoch interruption
with a watchdog thread that kills runaway code. This is the correct model for safe
execution of untrusted third-party tool code.

---

## 3. GitNexus — Zero-Server Code Intelligence Engine

### What it is

Converts a repository into a **persistent, queryable knowledge graph** (KuzuDB) that
gives AI agents structural codebase understanding — call chains, import graphs, functional
clusters, execution flows — in single queries instead of multi-hop file reads.

### The problem it solves precisely

AI coding agents (Claude Code, Cursor, Copilot) operate by reading raw files and
searching with grep/glob. This is structurally blind: agents miss cross-file dependencies,
break call chains, fail to understand module boundaries. GitNexus precomputes the
structural model once so agents can query it cheaply on every turn.

### The 7-phase indexing pipeline

```
1. Structure   (0–15%)   file tree → CONTAINS edges
2. Parse       (15–40%)  Tree-sitter ASTs → symbols (functions, classes, interfaces)
3. Imports     (40–55%)  language-aware path resolution → IMPORTS edges (85–95% conf.)
4. Calls       (55–75%)  call resolution via Symbol Table → CALLS edges (70–95% conf.)
5. Communities (75–85%)  Leiden community detection → labeled functional clusters
6. Processes   (85–95%)  BFS tracing → cross-community execution flows
7. Embeddings  (browser) 384-dim HNSW via snowflake-arctic-embed-xs (22M params)
```

### KuzuDB graph schema

```
Nodes: File, Symbol (function/class/method/interface), Community
Edges: CONTAINS, IMPORTS, CALLS (with confidence), EXTENDS, IMPLEMENTS, MEMBER_OF
```

Recall: BM25 + semantic cosine similarity + Reciprocal Rank Fusion (RRF).

### Dual-environment design

| Mode | Storage | Parsing | Embeddings |
|---|---|---|---|
| CLI + MCP | KuzuDB native (persistent) | Tree-sitter native | — |
| Browser WASM | KuzuDB WASM (in-memory) | Tree-sitter WASM | transformers.js (in-browser) |
| Bridge | CLI-indexed, browser-browseable | — | — |

The browser mode runs the **entire stack in WebAssembly** — zero server, zero upload,
complete data sovereignty.

### MCP server tools exposed to AI editors

`query`, `context`, `detect_changes`, `rename`, `wiki`, `hybrid_search`, `symbol_context`

### The PreToolUse hook pattern (most novel idea)

`gitnexus setup` injects hooks into Claude Code's configuration that **transparently
augment `grep`, `glob`, and `bash` tool calls** with knowledge graph context.
Agents get richer context without any change to their prompts or tool calls.

This is a **middleware pattern for agent observation and enrichment** — not a tool the
agent calls, but an invisible enrichment layer below the agent.

### Confidence scoring on edges

CALLS edges carry a confidence score (70–95%) and IMPORTS edges 85–95%. This models
uncertainty in static analysis rather than treating derived information as ground truth.

---

## 4. OpenClaw / awesome-claws — The Dominant Personal Agent Ecosystem

### What it is

OpenClaw is the reference implementation of a personal AI assistant framework.
It crossed **100k GitHub stars** in early 2026. awesome-claws catalogs the
dozens of implementations and forks that emerged from it.

### The file-driven configuration protocol (de facto standard)

```
~/.openclaw/
├── SOUL.md          # Agent identity — injected into every system prompt
├── AGENTS.md        # Multi-agent routing rules
├── TOOLS.md         # Tool declarations
├── HEARTBEAT.md     # Proactive task checklist (runs on schedule, no user input)
├── MEMORY.md        # Persistent session memory
└── skills/
    └── <skill-name>/
        └── SKILL.md # Domain-specific instructions (loaded lazily)
```

`SKILL.md` format: YAML frontmatter (name, description, triggers) + Markdown body.

**This file layout is now a de facto standard across the ecosystem.** Any agent that
adopts it can share skills with +5,700 community-built packages on ClawHub.

### The lazy skill loading pattern

Only a manifest (names + descriptions) is injected at session start.
The full `SKILL.md` body is read only when the LLM determines the skill is relevant.
This is **retrieval-augmented prompting applied to capabilities** rather than facts —
RAG for behavior, not knowledge.

### The Heartbeat / proactive loop

A scheduled prompt from `HEARTBEAT.md` runs every ~30 minutes.
The agent acts on it without user input. This gives agents proactive behavior
without requiring a complex scheduling infrastructure.

### Ecosystem implementations

| Implementation | Language | Key differentiator |
|---|---|---|
| OpenClaw | TypeScript | Reference, 12+ platform adapters, ClawHub registry |
| Moltis | Rust | Single binary, MCP tools, sandboxed execution |
| IronClaw | Rust | Privacy-first, local encrypted storage |
| Clawlet | Python | 2-minute setup |
| TinyClaw | TypeScript/Shell | Multi-team, fan-out, isolated workspaces |
| NanoClaw | TypeScript | Container-sandboxed |
| subzeroclaw | C | Edge hardware |
| NullClaw | Zig | Tiny binary, low memory |
| MimiClaw | C | ESP32-S3, no OS |

The pattern is **language-agnostic down to microcontrollers**.

### ClawHub — "npm for agents"

The natural packaging unit for LLM capabilities is a Markdown document, not a library.
5,700+ community skills. YAML frontmatter for metadata, Markdown for instructions.
Versioned, discoverable, composable.

---

## 5. Spacebot — Team-Oriented Agent OS

### What it is

A Rust-based agent OS targeting **team and multi-user environments** (Discord servers,
Slack workspaces, Telegram groups). Supports 50+ simultaneous users without any
single conversation blocking another.

**Stack**: Rust 2024, tokio, Rig v0.30, SQLite/sqlx, LanceDB, redb, single binary.

### The five-process-type model (central innovation)

Every LLM call is one of five types, each with its own system prompt, tool set,
history policy, and lifecycle:

```
Channel    User-facing. Has identity (SOUL.md). NEVER executes directly — only delegates.
           Always responsive. One per active conversation.

Branch     Forked thinking. Clones channel context. Max 10 turns. Non-blocking.

Worker     Stateless executor. No history, no personality. Runs concurrently.
           Max 5 per channel. Reports status via set_status tool.

Compactor  Programmatic (no LLM at 95%). Monitors context utilization:
           80% → background summarization
           85% → aggressive summarization
           95% → emergency truncation (no LLM, preserves tool call names/args/results)

Cortex     Singleton per agent. Runs every 60 min. Synthesizes Memory Bulletin.
           Writes to a shared location read lock-free by every channel every turn.
```

### Model routing by process type

```toml
[defaults.routing]
channel   = "anthropic/claude-sonnet-4"
branch    = "anthropic/claude-sonnet-4"
worker    = "anthropic/claude-haiku-4.5"
compactor = "anthropic/claude-haiku-4.5"
cortex    = "anthropic/claude-haiku-4.5"
```

Cost/quality tradeoff is a configuration concern, not a code concern.

### The Memory Bulletin pattern

Instead of per-turn RAG retrieval, the Cortex runs on a 60-minute cycle and
**pushes** a synthesized briefing to a shared location. Every channel reads it
**lock-free, zero-copy** on every turn. Trades memory freshness for zero retrieval
latency.

### Typed memory graph

```
Nodes: Fact, Preference, Decision, Identity, Event, Observation, Goal, Todo
Edges: RelatedTo, Updates, Contradicts, CausedBy, PartOf
```

Recall: hybrid BM25 + LanceDB vector similarity + RRF.

### Agent workspace layout

```
/data/agents/<id>/
├── workspace/
│   ├── SOUL.md / IDENTITY.md / USER.md
│   └── skills/         ← compatible with OpenClaw SKILL.md format
├── data/               ← SQLite, LanceDB, redb
├── archives/           ← compaction transcripts
└── ingest/             ← file watcher: text/MD/PDF → memory extraction
```

### Built-in tools (16)

```
reply, branch, spawn_worker, route, cancel, skip, react
memory_save, memory_recall, set_status
shell, file, exec, browser, cron, web_search
```

### Cron as first-class agent capability

```rust
CronConfig { prompt, interval, active_hours, notify }
```
Circuit breaker: 3 consecutive failures → auto-disable.

### OpenCode worker integration

Spacebot spawns OpenCode as a **persistent subprocess worker** for deep coding sessions.
The channel LLM chooses `worker_type: "builtin"` or `worker_type: "opencode"` based
on task complexity. Multiple workers targeting the same directory share one server.
This is a "best tool for each job" composition pattern — no reimplementation.

### Critical gap: no external message broker

> *"NATS is not a current dependency —
> cross-agent communication via external broker is on the roadmap."*

Inter-process communication is currently tokio mpsc channels and an internal event bus.
This limits Spacebot to single-process, single-host deployments.

---

## 6. Cross-cutting findings

### 6.1 The ecosystem has converged on SOUL.md + SKILL.md

OpenClaw established it. Spacebot adopted it. Moltis (Rust) adopted it.
OpenFang has HAND.toml + SKILL.md (slight variation). This is now the de facto
agent identity + capability extensibility protocol. Any infrastructure that wants
to interoperate with this ecosystem must speak this format.

### 6.2 Every serious project is Rust + single binary

OpenFang, Moltis, IronClaw, Spacebot, NullClaw, subzeroclaw, MimiClaw — all Rust.
The operational simplicity argument (no Docker, no microservices, no dependency
management at runtime) has won. The pattern has spread all the way to ESP32
microcontrollers. TypeScript is for prototypes and the reference implementation.

### 6.3 NATS is the missing transport layer in all of them

OpenFang has OFP (a custom P2P protocol). Spacebot has tokio mpsc (single-host only).
OpenClaw-pattern agents are fully local. None of them have a production-grade
distributed message broker that provides:
- At-least-once delivery with persistence
- Leader election and distributed coordination
- Pub/sub + request/reply + KV in one system
- Multi-node agent communication without custom protocols

NATS JetStream solves all of these simultaneously.

### 6.4 Scheduling is solved locally but not at scale

Every framework has a heartbeat/cron mechanism — but they are all single-process,
single-host, no-persistence implementations. None of them handle:
- Distributed leader election (only one node fires)
- At-least-once tick delivery
- Hot-reload of job configurations
- Multiple schedule types (interval + cron expression)
- Job lifecycle (enable/disable/remove without restart)

This is a real production gap. A heartbeat that only fires on one node in a cluster
that exits when the process dies is not production infrastructure.

### 6.5 The WASM sandbox is the correct answer for untrusted skill execution

OpenFang uses Wasmtime with dual metering (fuel + epoch watchdog). The ClawHub
ecosystem will eventually need sandboxed skill execution. Running arbitrary
community-submitted `SKILL.md` instructions through an LLM is safe; running
arbitrary community-submitted WASM tool code is only safe with a sandbox.
The fuel metering model (instruction budget per execution) is the right granularity
for billing and DoS prevention.

### 6.6 The PreToolUse hook pattern (GitNexus) is underexplored

Injecting enrichment transparently **below** the agent's tool calls — without
changing the agent's prompts or tool definitions — is a powerful middleware model.
The agent calls `grep`, but actually gets `grep` + structural graph context.
This pattern is applicable to any enrichment layer: security scanning, audit
logging, context injection, rate limiting, cost tracking.

### 6.7 Context compaction is a first-class engineering problem

Spacebot's tiered compaction (80/85/95% with LLM only for non-emergency tiers) and
OpenFang's memory compaction module both treat context management as core
infrastructure, not an afterthought. The key insight: **emergency truncation must
never use an LLM** (the LLM may itself be the bottleneck when context is at 95%).
Programmatic truncation with tool call preservation is the safe fallback.

### 6.8 Memory should be typed, not flat

Flat vector stores lose semantic structure. "A Fact that Contradicts another Fact"
is information that a flat embedding cannot represent. Both Spacebot (8 node types,
5 edge types) and OpenFang (structured memory with semantic recall) have moved past
flat vector search. The graph + vector hybrid (KuzuDB in GitNexus, LanceDB + redb
in Spacebot) appears to be the converging pattern.

### 6.9 The five-process-type model solves a real problem

The monolithic session bottleneck — agent goes dark during task execution — is a
real UX failure mode. Spacebot's solution (Channel never blocks, Workers run in
parallel, Compactor runs independently) is the right architecture for any system
serving more than one user. The model routing table (cheap models for background
processes, expensive models for conversational quality) is the cost-optimization
complement.

### 6.10 "npm for agents" (ClawHub) validates the Markdown-as-package model

5,700 skills in production demonstrates that LLM practitioners will adopt a
packaging ecosystem if the barrier is low enough. YAML frontmatter + Markdown body
is learnable in minutes. The versioned, discoverable, composable skill registry
pattern will likely become infrastructure, not application code.

---

## 7. What is worth copying

### High priority

**The SOUL.md + SKILL.md file layout as extensibility protocol**
Every project has converged on this. Adopting it means instant compatibility with
5,700+ existing skills and every framework in the ecosystem. The lazy loading pattern
(manifest first, full body on demand) is the right retrieval model.

**The five-process-type model (Spacebot)**
Channel / Branch / Worker / Compactor / Cortex is an elegant, composable
decomposition. It solves the session bottleneck cleanly and the routing table
pattern (model selection by process type in config, not code) is directly adoptable.

**Tiered LLM routing by process type**
Cheap models for background work (compaction, memory synthesis, task execution).
Expensive models for user-facing conversation. This is the correct cost model.

**Dual metering WASM sandbox (OpenFang)**
Fuel metering + epoch watchdog is the correct model for executing untrusted code.
If the platform ever allows third-party skill code (not just SKILL.md instructions),
this is the architecture to copy verbatim.

**The Memory Bulletin pattern (Spacebot Cortex)**
Push-based ambient memory injection: one scheduled synthesis job, zero per-turn
retrieval latency. The 60-minute synthesis cycle is a reasonable default for most
agent memory workloads.

**Tiered compaction with programmatic emergency fallback**
80% → background LLM summary. 95% → programmatic truncation preserving tool call
names, args, results. Never block on LLM when the context is at 95%.

**Typed memory graph**
8 node types and 5 edge types (Spacebot) or equivalent structured schema (OpenFang).
Don't start with a flat vector store. Model the semantic relationships from day one.

**The PreToolUse hook middleware pattern (GitNexus)**
Transparent enrichment below tool calls is a better UX than asking agents to call
new tools. Any cross-cutting concern (audit logging, context injection, rate limiting)
should be evaluated as a hook layer first.

**The Leiden community detection for codebase clustering (GitNexus)**
If code intelligence tooling is in scope: Leiden + KuzuDB is the implementation to
follow. The confidence-scored graph edges (CALLS: 70–95%) are the right model for
reasoning under static analysis uncertainty.

**OpenCode worker integration pattern (Spacebot)**
Delegate deep coding tasks to a purpose-built coding agent as a managed subprocess
rather than reimplementing coding capabilities. "Best tool for each job" composition
over monolithic capability stacks.

### Medium priority

**The Global Registry pattern (GitNexus)**
`~/.gitnexus/registry.json` mapping names to disk paths enables a single MCP server
daemon to lazily serve any indexed project without restart. Applicable to any
multi-project tool daemon.

**Multi-repo MCP server with connection pool (GitNexus)**
One server process, max 5 repos active, 8 connections each, LRU eviction after 5
minutes inactivity. This is the right resource model for a shared tool daemon.

**GCRA rate limiter (OpenFang)**
Generic Cell Rate Algorithm is the correct rate limiting model for bursty LLM
traffic. Token bucket gives too much credit; leaky bucket is too rigid.
GCRA handles burst with fairness.

**Ed25519 manifest signing (OpenFang)**
Skill/Hand manifests signed at build time. If a skill registry is ever run, manifest
signing is the trust model to implement. Not X.509, not HMAC — Ed25519 for its
small key size and fast verification.

**Circuit breaker on cron jobs (Spacebot)**
3 consecutive failures → auto-disable. This is the correct operational safeguard for
any scheduled job that makes external calls. Re-enable requires explicit intervention.

---

## 8. What is worth dropping (or never building)

**Custom P2P protocols (OpenFang's OFP)**
OpenFang built a custom P2P protocol with HMAC-SHA256 mutual auth. This solves
a real problem (secure multi-node agent communication) but NATS JetStream already
provides persistence, delivery guarantees, KV, leader election, and pub/sub in one
system. Custom P2P protocols should not be built when a mature broker exists.

**Single-host-only concurrency (Spacebot's tokio mpsc)**
Spacebot's internal event bus is tokio channels — this is explicitly acknowledged
as a limitation on the roadmap. Designing concurrency exclusively around in-process
channels from the start means a major architectural rewrite when multi-node is needed.

**Monolithic LLM calling convention (one model for everything)**
Every project that started with one model for all tasks has either moved to routing
tables (Spacebot) or is planning to. The cost difference between claude-haiku-4.5
and claude-sonnet-4 is large enough that using sonnet for background compaction is
not defensible in production. Route by process type from day one.

**Flat vector stores for memory**
A flat embedding store loses the Fact/Decision/Contradicts relationships that make
memory useful for long-running agents. The query "what decisions have we made that
contradict each other?" is unsolvable with cosine similarity alone. Build typed
graphs from the start.

**File-based persistence for memory (old OpenClaw pattern)**
The original OpenClaw pattern stores MEMORY.md as a flat text file. This does not
scale past single-user deployments. The SQLite + LanceDB + redb combination
(Spacebot) or SQLite + embeddings (OpenFang) is the production pattern.

**In-binary skill compilation (OpenFang's Hands)**
OpenFang compiles Hands into the binary at build time. This gives the best
performance and security but it means adding a new skill requires recompiling and
redeploying the binary. For a personal OS this is fine; for a platform serving
many different agent configurations it creates operational friction. The ClawHub
dynamic loading model (load SKILL.md at runtime) scales better for multi-tenant
deployments.

**Reimplementing coding capabilities**
Spacebot delegates coding to OpenCode. OpenFang has a Browser Hand but defers
to specialized tools for deep coding. Any new agent platform should integrate
with existing coding agents (Claude Code, OpenCode, etc.) via MCP or subprocess
rather than building from scratch.

**Bloated channel adapters as core infrastructure**
OpenFang has 40 messaging adapters built into the main binary. This creates a
large maintenance surface and means every agent deployment includes dependencies
it doesn't need. The gateway / adapter pattern should be a plugin or separate
process, not compiled into the core.

---

## 9. Architectural summary

The converging architecture across the strongest projects looks like this:

```
┌─────────────────────────────────────────────────────────────┐
│                      Gateway Layer                          │
│  (Discord, Telegram, Slack, WhatsApp, REST, CLI)            │
└──────────────────────┬──────────────────────────────────────┘
                       │ events / messages
┌──────────────────────▼──────────────────────────────────────┐
│                    Channel Process                           │
│  SOUL.md identity · full conversation history               │
│  delegates to workers · never blocks                        │
└──────┬──────────────────┬───────────────────────────────────┘
       │ spawn_worker      │ branch
┌──────▼──────┐   ┌───────▼───────┐   ┌──────────────────────┐
│  Worker(s)  │   │   Branch(es)  │   │    Cortex (60 min)   │
│  stateless  │   │  deep reason  │   │  synthesizes Memory  │
│  concurrent │   │  max 10 turns │   │  Bulletin → push     │
└──────┬──────┘   └───────────────┘   └──────────────────────┘
       │ tool calls
┌──────▼──────────────────────────────────────────────────────┐
│                     Tool Layer                               │
│  shell · file · browser · cron · web_search · MCP clients  │
│  (WASM-sandboxed for untrusted third-party tools)           │
└──────┬──────────────────────────────────────────────────────┘
       │
┌──────▼──────────────────────────────────────────────────────┐
│                   Memory / Storage Layer                     │
│  Typed graph (Fact, Decision, Contradicts…)                 │
│  Vector index (hybrid BM25 + embedding + RRF)               │
│  KV store (redb) · Relational (SQLite)                      │
└──────┬──────────────────────────────────────────────────────┘
       │
┌──────▼──────────────────────────────────────────────────────┐
│              Distributed Transport (the gap)                │
│  "NATS is on the roadmap" — Spacebot                        │
│  Custom OFP P2P — OpenFang                                  │
│  None — OpenClaw ecosystem                                  │
└─────────────────────────────────────────────────────────────┘
```

### The one universally unsolved problem

**Distributed, multi-node agent communication with delivery guarantees.**

Every framework either:
- Has no solution (OpenClaw pattern — fully local, single process)
- Has a single-host workaround (Spacebot — tokio mpsc, acknowledged on roadmap)
- Built a custom protocol (OpenFang OFP) that lacks the operational maturity of
  a production-grade broker

None of them have:
- At-least-once tick delivery with replay
- Distributed leader election (only one node fires a scheduled task in a cluster)
- Hot-reload of agent configurations across a fleet
- NATS KV as the coordination primitive

This is the architectural gap that all four projects share.
It is not a minor limitation — it is the difference between a local assistant
and production-grade distributed agent infrastructure.

---

*Report compiled: 2026-02-26*
*Sources: github.com/RightNow-AI/openfang, github.com/abhigyanpatwari/GitNexus,*
*github.com/machinae/awesome-claws, spacebot.sh*
