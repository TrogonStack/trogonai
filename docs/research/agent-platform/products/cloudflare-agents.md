# Cloudflare Agents SDK: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from developers.cloudflare.com/agents and
`cloudflare/agents` (`packages/agents/src/index.ts`).

## Source anchors

Sources retrieved 2026-07-13. Source-level claims are pinned to the official
[`agents` package version 0.17.4](https://github.com/cloudflare/agents/releases/tag/agents%400.17.4)
at `cloudflare/agents` commit
[`03cdc82`](https://github.com/cloudflare/agents/commit/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea).

- Definitions and architecture: [What are agents?](https://developers.cloudflare.com/agents/concepts/what-are-agents/),
  [Agents overview](https://developers.cloudflare.com/agents/), and [Agent
  class internals](https://developers.cloudflare.com/agents/runtime/lifecycle/agent-class/),
  including the `DurableObject > Server > Agent > AIChatAgent` hierarchy.
- Persistence and lifecycle: [Long-running agents](https://developers.cloudflare.com/agents/concepts/agentic-patterns/long-running-agents/),
  [Sub-agents](https://developers.cloudflare.com/agents/runtime/execution/sub-agents/),
  [Agents with Workflows](https://developers.cloudflare.com/agents/concepts/workflows/),
  and [platform limits](https://developers.cloudflare.com/agents/platform/limits/).
- Deployment configuration: [Configuration, sections "Migrations" and
  "Migration best practices"](https://developers.cloudflare.com/agents/runtime/operations/configuration/).
- Stable source symbols: [package version](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/package.json#L1-L17),
  [`Agent` class](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/src/index.ts#L1569-L1578),
  [`subAgent` and `runAgentTool`](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/src/index.ts#L7908-L8005),
  [`abortSubAgent` and `deleteSubAgent`](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/src/index.ts#L10544-L10607),
  and [`destroy`](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/src/index.ts#L10803-L10857).

## The `agent` noun (primary-source quotes)

- Behavioral definition: "An agent is an AI system that can autonomously
  execute tasks by making decisions about tool usage and process flow."
  ([What are agents?](https://developers.cloudflare.com/agents/concepts/what-are-agents/))
- Structural definition: "Each agent session has a durable identity, local
  SQL storage, real-time connections, scheduled work, and recoverable
  execution." ([Agents overview](https://developers.cloudflare.com/agents/))
- In code, `class Agent<Env, State, Props> extends Server`, inheritance
  chain **DurableObject > Server (partyserver) > Agent > AIChatAgent**;
  docs state plainly that "agents *are* Durable Objects."
  ([Agent class internals](https://developers.cloudflare.com/agents/runtime/lifecycle/agent-class/);
  [`Agent` source](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/src/index.ts#L1569-L1578))
- Instance identity: instances are "globally unique: given the same name (or
  ID), you will always get the same instance"; "Each unique name gets its
  own isolated agent with its own state"; one class can have "millions of
  instances functioning as independent micro-servers."
- The identity/process duality is explicit: long-running agents are "durable
  identities that exist continuously but run intermittently. They wake on
  demand, via HTTP requests, WebSocket connections, RPC calls, scheduled
  alarms, or emails, then hibernate when idle, consuming zero compute
  resources during downtime."
  ([Long-running agents](https://developers.cloudflare.com/agents/concepts/agentic-patterns/long-running-agents/))
- **Conceptual model: agent-as-durable-object (stateful actor).** The class
  is code; the *instance* is simultaneously a persistent identity, a state
  store (embedded SQLite), and an intermittently-running process. Uniquely
  in this study, identity and state and process are the same object.

## Subagents

- First-class: "Spawn child agents as co-located Durable Objects with their
  own isolated SQLite storage." (`Agent.subAgent(cls, name)`.)
  ([documentation](https://developers.cloudflare.com/agents/runtime/execution/sub-agents/);
  [source](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/src/index.ts#L7908-L7933))
- **Get-or-create by name:** "The first call for a given name triggers the
  child's `onStart()`. Subsequent calls return the existing instance."
- **Kill vs delete distinguish process from identity:** `abortSubAgent`,
  "The child stops executing immediately and restarts on the next
  `subAgent()` call. Storage is preserved, only the running instance is
  killed." `deleteSubAgent`, "permanently wipe its storage. The next
  `subAgent()` call creates a fresh instance with empty SQLite."
- **Isolation:** "Each sub-agent has its own SQLite database, completely
  isolated from the parent and from other sub-agents." Parent identity is
  persisted on the child (`cf_agents_parent_path` storage key; `parentPath`
  / `selfPath` accessors).
- **Communication is typed RPC:** `SubAgentStub<T>` exposes the child's
  public methods as async RPC calls; WebSocket handles stay with the root
  object and bridge to children. No nesting limit documented; paths imply
  arbitrary depth.
- **Agents-as-tools:** `runAgentTool()` spawns a child by class + runId,
  tracks it in a `cf_agent_tool_runs` SQLite table, and re-attaches after
  parent eviction, delegation that survives hibernation.
- Delegated deterministic work goes to **Workflows** instead: "Agents launch
  instances with `runWorkflow()`, dispatch events, respond to approvals,
  control execution, and query status."

## Configuration surface (what, where, why)

- **The class code is the config:** overridable handlers `onStart`,
  `onRequest`, `onConnect`, `onMessage`, `onStateChanged`, `onEmail`,
  `onClose`, `onError`, plus gating/recovery hooks (`validateStateChange`,
  `onFiberRecovered`, `onBeforeSubAgent`).
- **Per-instance state:** `this.state`/`setState()` (persisted in a
  `cf_agents_state` SQLite table, broadcast to connected clients) and
  `this.sql` tagged-template queries against the instance's own SQLite.
- **Scheduling registered at runtime:** `this.schedule` with four shapes,
  `scheduled` (absolute time), `delayed`, `cron`, `interval`. "Since Durable
  Objects only allow one alarm at a time, the Agent class works around this
  by managing multiple schedules in SQL and using a single alarm."
- **Static options:** `hibernate` (default true), `keepAliveIntervalMs`
  (30s), retry, memory-limit and fiber-recovery options.
- **Platform bindings** live in `wrangler.jsonc` (DO binding +
  `new_sqlite_classes` migration; "Never modify existing migrations, always
  add new ones").
- Stated rationale for state-with-agent: "This design eliminates the need
  for centralized session stores, state persists with the agent instance."
  Platform pitch: "Deploy once and Cloudflare runs your agents across its
  global network... No infrastructure to manage, no sessions to reconstruct,
  no state to externalize."

## Binding time

- **Deploy time:** class code and namespace binding.
- **First request:** instance creation is lazy by name, "the first request
  to a unique name-pair instantiates that agent."
- **Runtime-mutable:** everything per-instance, state (`setState`, SQL),
  schedules, spawned children, MCP server connections (all in the instance's
  SQLite).
- **Code deploys restart running instances:** the SDK exports
  `isDurableObjectCodeUpdateReset` and classifies it in scheduler retry
  logic; `onStart()` refires on every wake including post-deploy restarts.
  State survives (SQLite); in-flight execution does not, unless checkpointed
  as fibers.
- **No user-facing version primitive.** Internally the SDK migrates its own
  SQLite schema idempotently on every wake
  ([`CURRENT_SCHEMA_VERSION = 11`](https://github.com/cloudflare/agents/blob/03cdc828c0bc3c6bb1d9aa636bb46ceb00a4e0ea/packages/agents/src/index.ts#L1095)).

## Relationships between nouns

- **Class 1:millions instances;** instance 1:1 embedded SQLite; instance 1:N
  schedules, queues, workflows, fibers, tool-runs, MCP connections (each an
  internal table: `cf_agents_state`, `cf_agents_schedules`,
  `cf_agents_queues`, `cf_agents_workflows`, `cf_agents_runs`,
  `cf_agents_fibers`, `cf_agent_tool_runs`, `cf_agents_facet_runs`,
  `cf_agents_mcp_servers`).
- **Agent : session is undefined by design**, the docs list "chats,
  documents, sessions, shards, or projects" as things an instance can be;
  one instance can hold many concurrent WebSocket clients ("Shared rooms:
  multiple users share one instance") or be one-per-user. Naming strategy =
  session strategy; it's entirely application routing
  (`routeAgentRequest` matches `/agents/:class/:name`).
- **Agents vs Workflows** (their sharpest concept split): Agents =
  "long-lived identity that wakes on events," real-time channels, built-in
  SQL, application-defined failure handling; Workflows = "run to
  completion," step-level persistence, automatic retries, no real-time
  communication. Docs triad: "Agents: non-linear, non-deterministic...
  Workflows: linear, deterministic execution paths. Co-pilots: augmentative
  AI assistance."
- Four-part architecture from the overview: communication channels; the
  **agent harness** ("the loop: how the agent calls models, selects tools,
  handles tool results, streams responses, and decides whether to
  continue"); the Agents SDK runtime (durable infrastructure); tools. The
  chat harness has been split out to `@cloudflare/ai-chat`.

## Lifecycle

- **Create:** lazy by name. **Wake paths (5):** HTTP, WebSocket, RPC,
  alarms/schedules, email.
- **Hibernate:** default; WebSocket hibernation preserves connection state
  platform-side; `keepAlive()`/`keepAliveWhile()` pin the instance during
  long operations (LLM streaming).
- **Persists across hibernation/restarts:** "State stored via `setState()`,
  SQLite data and tables, scheduled tasks, fiber checkpoints..., WebSocket
  connection state." Does not survive: "in-memory variables and class
  fields, running timers, open fetch requests, local closures."
- **Destroy is explicit and manual:** `this.destroy()` "permanently delete[s]
  SQLite data, schedules, and state"; instances otherwise persist
  indefinitely.
- **Who runs the loop: the customer**, in handlers, the platform provides
  identity, persistence, routing, scheduling. Compute is billed
  per-request/message ("30 seconds max compute per request, refreshed per
  HTTP request / incoming WebSocket message"); limits page: "tens of
  millions+" concurrent agents, 1 GB state per agent, ~250,000 agent
  definitions per account.

## What makes it "an agent" here (our inference)

Our inference: Cloudflare's agent is a **named, addressable, stateful actor
that never dies**, identity, state, and (intermittent) process fused into
one Durable Object instance. There is no definition/instance split at all:
"creating an agent" is just addressing it by name, configuration is code
plus mutable per-instance state, and the LLM loop is whatever the class's
handlers do. Where every managed platform in this study separates the config
record from the running session, Cloudflare collapses them, the strongest
possible answer to binding time (everything binds at runtime, per instance)
and the weakest possible answer to fleet governance (no versions, no
roster, no central definition).

## Open questions

- No fleet management: how to enumerate/migrate/garbage-collect millions of
  named instances isn't covered (no list-instances primitive in the docs
  read).
- Code deploys vs long-lived instances: fiber checkpoints mitigate restarts,
  but cross-version state compatibility is the developer's problem.
- The `Props` generic and `props` on `getAgentByName` (per-wake input) vs
  persisted state deserves a closer read.
- Cloudflare's role as a Claude Managed Agents self-hosted sandbox provider
  is documented on Anthropic's side but not in these docs.
