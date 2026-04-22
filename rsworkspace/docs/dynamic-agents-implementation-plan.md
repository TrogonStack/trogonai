# Dynamic Multi-Agent System — Implementation Plan

## Background

Trogon currently reacts to events (a PR opens, a CI fails, an incident fires) by running an
agent loop that processes the event and terminates. Every run starts fresh. There is no memory
between events, no coordination between agents, and no way to handle something the system was
not explicitly programmed for.

This document describes the architecture and implementation plan for a dynamic multi-agent
system that addresses these gaps. The design is directly inspired by Rivet Agent OS and informed
by current research on multi-agent orchestration (LangGraph, OpenAI Agents SDK, Anthropic's
orchestrator-workers pattern, Microsoft's multi-agent reference architecture).

---

## Core Concepts

### The Problem With the Current Approach

When a PR opens on Monday, the bot reviews it and the run completes. When the developer pushes
fixes on Wednesday, a new run starts with no memory of Monday's review. The agent may repeat
comments already made, or miss that a previously flagged issue was addressed. Each event is
an amnesia reset.

This is not a persistence problem — durable promises already handle crash recovery within a
single run. It is a **scope problem**: runs are scoped to one event, but meaningful context
lives across the entire lifecycle of an entity.

### The Solution: Three Layers

```
Event arrives
      ↓
Router Agent          ← AI-powered, queries live registry, decides who handles it
      ↓
Entity Actor          ← stateful, lives for the lifetime of the entity, spawns sub-agents
      ↓
Transcript            ← every message, tool call, routing decision — persisted in order
```

### Why Dynamic

A static system has a hardcoded map: "PR events go to PR Actor." Adding a new entity type
requires a code change and a redeploy.

A dynamic system works differently:

- Agents **register their own capabilities** at startup into a live registry
- The Router **queries that registry at runtime** — no hardcoded list anywhere
- Routing decisions are made by the **LLM reading event content**, not pattern matching
- Entity Actors can **spawn specialized sub-agents on demand** for parallelizable work
- When an unknown entity type arrives, the system **reasons about it** instead of failing

New agent types become available the moment they start up and register. The system grows
without touching the core architecture.

---

## Architecture

### NATS Subject Design

```
trogon.events.>                         all incoming events from the gateway (router subscribes)
actors.{type}.{key}                     entity actor inbox (actor subscribes to its own subject)
transcripts.{type}.{key}.{session_id}   transcript entries per entity session
AGENT_REGISTRY  (KV bucket)             live agent capabilities, TTL-based
ACTOR_STATE     (KV bucket)             persisted actor state per entity key
```

### Data Flow

```
Gateway publishes event to trogon.events.github.pull_request
      ↓
Router Agent reads event content
Router queries AGENT_REGISTRY for available agents
Router calls LLM: "Given this event and these agents, who handles it and with what key?"
LLM returns: { agent_type: "PrActor", key: "owner/repo/456", reasoning: "..." }
Router publishes to actors.pr.owner.repo.456
Router appends RoutingDecision to transcript
      ↓
PrActor receives event on actors.pr.owner.repo.456
Runtime loads state from ACTOR_STATE/pr/owner:repo:456 (empty on first event, full on subsequent)
Runtime opens new transcript session
Actor calls handle(state, ctx) — LLM calls and tool calls auto-appended to transcript
Runtime saves updated state to ACTOR_STATE with optimistic concurrency
Runtime closes transcript session
      ↓
If actor spawns sub-agent:
  ctx.spawn_agent("security_analysis", payload)
  Runtime queries registry for matching agent
  Publishes to that agent's inbox via NATS request-reply
  Appends SubAgentSpawn entry to transcript
  Returns result to parent actor
```

---

## Crates

### Crate 1: `trogon-transcript`

**Purpose:** Persistent, append-only audit trail for every agent session.

**Backend:** NATS JetStream — append-only, sequential, queryable by entity key and time range,
replayable.

**Stream:** `TRANSCRIPTS`
**Subject pattern:** `transcripts.{actor_type}.{actor_key}.{session_id}`

**Entry types:**

```rust
pub enum TranscriptEntry {
    Message {
        role:      Role,   // User, Assistant, System, Tool
        content:   String,
        timestamp: u64,
        tokens:    Option<u32>,
    },
    ToolCall {
        name:        String,
        input:       serde_json::Value,
        output:      serde_json::Value,
        duration_ms: u64,
        timestamp:   u64,
    },
    RoutingDecision {
        from:      String,
        to:        String,
        reasoning: String,
        timestamp: u64,
    },
    SubAgentSpawn {
        parent:     String,
        child:      String,
        capability: String,
        timestamp:  u64,
    },
}
```

**Public API:**
- `Transcript::open(nats, actor_type, actor_key) -> Transcript`
- `transcript.append(entry) -> Result<()>`
- `Transcript::query(nats, actor_key, since) -> Stream<TranscriptEntry>`
- `Transcript::replay(nats, session_id) -> Stream<TranscriptEntry>`

**Dependencies:** `main` only (`async-nats`, `trogon-nats`).

---

### Crate 2: `trogon-registry`

**Purpose:** Live, self-updating index of every agent and its capabilities. Makes the system
dynamic — the router discovers agents at runtime rather than reading a hardcoded list.

**Backend:** NATS KV bucket `AGENT_REGISTRY` with TTL.

**Heartbeat:** Agents call `refresh()` every 15 seconds. KV TTL is 30 seconds. Dead agents
expire and disappear from the registry automatically.

**Capability record:**

```rust
pub struct AgentCapability {
    pub agent_type:   String,
    pub capabilities: Vec<String>,   // e.g. ["code_review", "security_analysis"]
    pub nats_subject: String,        // subject pattern this agent listens on
    pub current_load: u32,
    pub metadata:     serde_json::Value,
}
```

**Public API:**
- `Registry::new(nats) -> Registry`
- `registry.register(capability) -> Result<()>`
- `registry.refresh() -> Result<()>` — heartbeat, call on a timer
- `registry.discover(query: &str) -> Vec<AgentCapability>` — find agents by capability
- `registry.list_all() -> Vec<AgentCapability>`

When a new Entity Actor type is built and deployed, it calls `register()` on startup. From
that moment the router knows it exists. No other change is needed anywhere.

**Dependencies:** `main` only (`async-nats`, `trogon-nats`).

---

### Crate 3: `trogon-actor`

**Purpose:** The Entity Actor primitive. Stateful agents tied to a specific domain entity,
backed by NATS KV, with automatic transcript recording and sub-agent spawning.

**The trait:**

```rust
pub trait EntityActor {
    type Key:   Serialize + DeserializeOwned + Display;
    type State: Serialize + DeserializeOwned + Default;

    /// Extract the entity key from an incoming NATS event.
    fn extract_key(subject: &str, payload: &[u8]) -> Option<Self::Key>;

    /// Handle one event. State is pre-loaded and will be auto-saved after return.
    async fn handle(
        &mut self,
        state: &mut Self::State,
        ctx:   ActorContext,
    ) -> Result<()>;

    /// Called once when the actor is created for the first time.
    async fn on_create(state: &mut Self::State) -> Result<()> { Ok(()) }

    /// Called when the actor's entity is destroyed (PR closed, incident resolved, etc.)
    async fn on_destroy(state: &mut Self::State) -> Result<()> { Ok(()) }
}
```

**`ActorContext`** gives the handler everything it needs:
- `ctx.llm` — call the configured LLM
- `ctx.tools` — invoke registered tools
- `ctx.transcript` — append entries manually if needed (most appending is automatic)
- `ctx.spawn_agent(capability, payload)` — spawn a sub-agent and await its result
- `ctx.entity_key` — the key of the current entity

**Runtime flow (per incoming event):**

1. Event arrives on `actors.{type}.{key}`
2. Load state from `ACTOR_STATE/{type}/{key}` (empty `Default` on first event)
3. Open new transcript session
4. Call `handle(state, ctx)`
5. Every LLM call and tool call inside `handle` is auto-appended to the transcript
6. Save updated state to `ACTOR_STATE` with **optimistic concurrency** (NATS KV revision check)
7. On revision conflict (two events arrived simultaneously), retry from step 2
8. Close transcript session

**Sub-agent spawning:**

```rust
// Inside a handle() implementation:
let result = ctx.spawn_agent("security_analysis", payload).await?;
```

The runtime queries `trogon-registry` for an agent with that capability, publishes to its
inbox via NATS request-reply, appends a `SubAgentSpawn` entry to the transcript, and returns
the result to the calling actor.

**Registration:** On startup, each actor type registers itself in `trogon-registry` and starts
a background task that calls `registry.refresh()` every 15 seconds.

**Dependencies:** `trogon-transcript`, `trogon-registry`, `main`.

---

### Crate 4: `trogon-router`

**Purpose:** The first layer. Receives every incoming event, reasons about who should handle
it using the LLM and the live registry, and routes accordingly.

**Subscribes to:** `trogon.events.>` — every event published by the gateway.

**For each event:**

1. Query `trogon-registry` for all available agents (`registry.list_all()`)
2. Call the LLM with the event payload and the live agent list:

```
You are a routing agent. An event has arrived. Based on its content, decide which
registered agent should handle it and what the entity key is.

Event subject: github.pull_request
Event payload: { ...raw payload... }

Available agents:
  - PrActor: handles pull requests. Capabilities: [code_review, security_analysis]
    Subject pattern: actors.pr.{key}
  - IncidentActor: handles incidents. Capabilities: [triage, timeline, escalation]
    Subject pattern: actors.incident.{key}

Return JSON:
{
  "primary":   { "agent_type": "...", "key": "...", "subject": "..." },
  "secondary": [],
  "reasoning": "..."
}
```

3. Parse routing decision
4. Publish event to the primary agent's subject
5. If secondary agents are listed, publish to each in parallel
6. Append `RoutingDecision` entry to transcript (including the LLM's reasoning)

**Handling unknown entity types:**

If the LLM cannot find a matching agent in the registry, the router appends an `Unroutable`
transcript entry with the LLM's explanation of why nothing matched. This is the signal that
a new Entity Actor type should be built — and the transcript provides the full context of
what that actor should handle.

**LLM backend:** Configurable via environment variables. Uses the xAI or Anthropic API
directly via HTTP, following the same pattern as `trogon-xai-runner`. No dependency on the
pending runner branches.

**Dependencies:** `trogon-transcript`, `trogon-registry`, `main`.

---

## Build Order

All four crates live in one branch: `feat/dynamic-agents` from `main`.

| Phase | Crate | Dependencies | Testable standalone |
|-------|-------|-------------|---------------------|
| 1 | `trogon-transcript` | `main` only | Yes — mock events, in-process NATS |
| 2 | `trogon-registry` | `main` only | Yes — mock agents, TTL verification |
| 3 | `trogon-actor` | transcript + registry | Yes — mock LLM, mock tools |
| 4 | `trogon-router` | all three | Yes — mock events, mock registry |

Each phase is fully testable before the next begins. No dependency on any other pending
branch in the repository.

---

## What Exists in `main` That This Builds On

| Infrastructure | Used by |
|---|---|
| `trogon-nats` — NATS KV | `trogon-registry` (agent registry), `trogon-actor` (state) |
| `trogon-nats` — JetStream | `trogon-transcript` (append-only log) |
| `trogon-nats` — request-reply | `trogon-actor` (sub-agent spawning), `trogon-router` (routing) |
| `trogon-nats` — lease | Can be used for router leader election in multi-instance deployments |
| `trogon-std` | Base abstractions, test helpers |
| `acp-telemetry` | OpenTelemetry tracing across all four crates |

---

## Example: PR Review Across Multiple Events

**Monday — PR #456 opens:**
- Router reads event, queries registry, asks LLM who handles it
- LLM routes to PrActor with key `owner/repo/456`
- PrActor loads state (empty — first event)
- PrActor calls LLM to review PR, posts GitHub comment
- PrActor saves state: `{ files_reviewed: [...], issues: ["auth.rs: null check missing"] }`
- Transcript: RoutingDecision + Message(user) + Message(assistant) + ToolCall(post_comment)

**Wednesday — developer pushes new commits:**
- Router reads event, routes to same PrActor key `owner/repo/456`
- PrActor loads state (has Monday's context)
- PrActor knows what it already reviewed
- PrActor posts incremental review: "null check in auth.rs is fixed. New issue in payments.rs"
- PrActor updates state
- Transcript: new session appended to same entity's transcript

**Friday — CI fails:**
- Router reads CI event, routes to PrActor (same key — same PR)
- PrActor loads state (has Monday + Wednesday context)
- PrActor correlates CI failure with payments.rs issue it flagged Wednesday
- PrActor posts: "CI failure is related to the payments.rs issue from Wednesday's review"
- Full transcript of the PR's lifecycle is queryable from the start

---

## Reliability Considerations

**Optimistic concurrency:** NATS KV tracks a revision number per key. If two events for the
same entity arrive simultaneously, the second save detects the conflict and retries: load
fresh state → handle → save. No external locking required.

**Idempotency:** Event handlers should be written to be safe to retry. If a tool call
(e.g., posting a GitHub comment) already succeeded before a crash, the retry should detect
and skip it.

**Short chains:** Research shows reliability degrades with each sequential agent hop. Keep
the chain short: Router → Entity Actor → (optional) Sub-agents. Avoid chains longer than
three hops.

**Registry TTL:** The 30-second TTL on registry entries means a crashed agent is removed
automatically. The router will not route to a dead agent on the next query.

---

## Future Integration

When `feat/durable-agent-runs` is eventually merged, the automation system's rule-based
trigger matching can be replaced or augmented by the Router Agent. The `Automation` struct's
`trigger` field maps directly to the routing patterns the LLM would reason about.

The Entity Actor's `State` and the durable promise's checkpoint serve complementary roles:
durable promises handle crash recovery within a single event handler invocation; Entity Actor
state handles knowledge accumulation across all events over the entity's lifetime.
