# OpenComputer: Sessions, Agents, Secrets

Part of the [provider agent contracts research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-09-11. Every claim is sourced inline to a
vendor documentation page or machine-readable spec. These documentation sites
are unversioned and publish no commit identifiers, so the authoritative
anchors are the pinned entry points below plus the API version header each
surface requires:

- OpenComputer Serverless Agents,
  [overview](https://docs.opencomputer.dev/agents/overview),
  [API](https://docs.opencomputer.dev/agents/api), and
  [secrets](https://docs.opencomputer.dev/agents/secrets).
- No version pin exists. The site's `llms.txt` is incomplete, so
  [`sitemap.xml`](https://docs.opencomputer.dev/sitemap.xml) was used as the
  page index.

OpenComputer's "Serverless Agents" product is a managed runtime for agents that are *authored as TypeScript source in a repository* and *deployed as immutable builds* into a cloud project. The defining choice is that an agent is a synchronous function re-rendered by a managed harness before every model step: "The exported function is a reactive description of the agent. OpenComputer renders it for the current input and uses its return value as instructions for the managed agent loop" (https://docs.opencomputer.dev/agents/overview). The platform owns the loop (model call, tool validation and execution, streaming, event recording, continuation) while the function owns only *what the agent is for this step*. Around that sit three durable server-side objects: a **deployment** (immutable build of one agent), a **session** (durable conversation pinned to a deployment, with an append-only `seq`-ordered event log), and **memory** (project-and-environment scoped documents that outlive sessions). Credentials are deliberately *not* a thing agent code can read: a managed secret is write-only and is injected by the platform only into a statically declared outbound HTTP connection, after destination, method, path, agent, project and environment have been validated (https://docs.opencomputer.dev/agents/secrets). A second, separate secret system exists on the Sandboxes product (sealed placeholder env vars plus a substituting proxy); the two are not the same mechanism.

Note on scope: `docs.opencomputer.dev` hosts at least two products, Serverless Agents (`/agents/*`) and Sandboxes (`/sandboxes/*`, `/reference/*`, `/cli/*`). They use overlapping vocabulary ("agent", "session", "secret", "checkpoint") for different things. Everything in the Agents/Sessions/Secrets sections below is from `/agents/*` unless stated. There is also an apparently older surface at `/reference/cli/agent` ("Durable Agent Sessions API", `agent.toml`, "Hook URLs") that does not reconcile with `/agents/*`; see Gaps.

## Resource model

```
Organization
  |  (one API key reaches every project, agent and session in the org)
  |
  +-- Project  (id, slug, name, environments, agents[], createdAt, updatedAt)
  |     |        cloud boundary for agents, environments, deployments, sessions
  |     |        source repo layout: opencomputer/project.ts + opencomputer/agents/<id>/agent.ts
  |     |
  |     +-- Environment: "development" | "production"     (exactly two, per project)
  |     |     |  separate: secrets, runtime variables, schedule state, webhook
  |     |     |  tokens+URLs, channel bindings, memory store, outbox delivery history
  |     |     |
  |     |     +-- Alias: "development" | "production"  ---> points at one Deployment
  |     |
  |     +-- Agent  (id, name, activeAlias, activeDeploymentId, deploymentCount)
  |     |     |      authored as opencomputer/agents/<id>/agent.ts, listed in project.ts
  |     |     |      owns: tools/, skills/, schedules/, channels/, outboxes/ subdirs
  |     |     |
  |     |     +-- Deployment  (id, agentId, alias, memory declarations, createdAt)
  |     |           IMMUTABLE build of ONE agent. Addressed by id, or by "<agent-id>@<alias>".
  |     |
  |     +-- Memory resource  (declared by defineMemory, id, provider, maxBytes)
  |     |     +-- Document  (id, title, text, summary, agentWrites, revision, bytes, ...)
  |     |
  |     +-- Channel / Outbox / Schedule / Webhook / Event subscription
  |
  +-- Session  (durable conversation with ONE deployed agent; pins its deployment)
        |   status: new | connecting | idle | running | waiting_runtime |
        |           suspending | suspended | resuming | failed | ended
        |   memory bindings fixed at creation (max 8)
        |
        +-- Turn  (id, input, mode, status, createdAt, updatedAt, deliveries[])
        |     mode: queue | steer | interrupt
        |
        +-- Event log  (append-only; seq starts at 1 and only grows)
              { id, seq, timestamp, sessionId, turnId?, type, data }

Per turn, the harness renders the agent function ONCE PER MODEL STEP:
  Session -> Harness -> render agent fn -> { instructions, model, tools,
  subagents, MCP servers } -> model request -> tool calls -> events -> render again
```

Trigger sources that start or feed a session: direct API/CLI/playground turn (`user`), `channel`, `schedule`, `webhook`, `subagent`, `system`, `event` (https://docs.opencomputer.dev/agents/inputs).

## Agents

### What is code versus what is a stored resource

Code (in the repo, under `opencomputer/`, packaged into a deployment):
the agent function itself, `defineTool`, `defineMcpServer`, `defineConnection`, `defineMemory`, `defineSchedule`, `defineChannel`, `defineOutbox`, `registerChannel`, `registerOutbox`, skills under `skills/`, and the `project.ts` agent list. "Agent behavior belongs in the repository under `opencomputer/`. The dashboard is primarily for selecting environments, testing agents, and inspecting what was deployed" (https://docs.opencomputer.dev/agents/overview).

Stored server-side resources (not code): projects, deployments, aliases, sessions, turns, events, memory documents, secrets and agent runtime variables, webhook tokens and their request ledger, Slack channel credentials/destination bindings, outbox delivery records, event subscriptions. "Secrets, runtime variables, schedules, channels, outboxes and logs are managed with the CLI and the dashboard" (https://docs.opencomputer.dev/agents/api). Webhooks are explicitly "operational configuration, so create them in the dashboard or CLI rather than in agent source" (https://docs.opencomputer.dev/agents/webhooks).

### Hook inventory (verbatim)

From https://docs.opencomputer.dev/agents/hooks and https://docs.opencomputer.dev/agents/reactive-agents:

| Hook | Required | What it does |
| --- | --- | --- |
| `useInput()` | no | Reads the current input source, text, and structured payload |
| `useCurrentInput()` | no | Alias for `useInput()` |
| `useModel(model)` | no (see note) | Selects the model used for the next step |
| `useTool(tool)` | no | Exposes a named or code-defined tool |
| `useMcpServer(server)` | no | Exposes tools from a managed HTTPS MCP server |
| `useSubagent(agent)` | no | Makes another project agent available for delegation |
| `useSessionData<T>(key)` | no | Reads a durable value associated with the session |
| `useMemory(memory)` | no | Reads bound memory and selects its permitted tools for this model request |
| `useSecret(name)` | no | Referenced inside `defineConnection` headers, e.g. `bearer(useSecret("GITHUB_TOKEN"))` |

Note: whether `useModel` is mandatory is **not documented**; the minimal example `export default function Agent() { return "You are a helpful assistant."; }` omits it (https://docs.opencomputer.dev/agents/reactive-agents), and no default model is stated.

"Unlike React component hooks, OpenComputer resource hooks can be conditional." "Hooks can only run while OpenComputer is rendering an agent." Custom hooks are "ordinary synchronous functions" and the docs recommend the `use` prefix (https://docs.opencomputer.dev/agents/hooks). Skills are the exception: "Skills do not need a hook. OpenComputer packages the entire `skills/` directory with that agent" (https://docs.opencomputer.dev/agents/skills).

### What "reactive" means and what gets rendered

"OpenComputer renders the exported function before each model step. Every hook called during that render contributes to the next step's configuration. Hooks describe capabilities; they do not call the model or run a tool themselves" (https://docs.opencomputer.dev/agents/hooks). The harness loop is enumerated as: "1. renders the agent function; 2. builds a model request from the returned instructions and selected resources; 3. streams the model response; 4. validates and executes requested tools; 5. records the resulting events; and 6. renders again when another model step is required" (https://docs.opencomputer.dev/agents/mental-model). Consequence stated by the docs: "Because a function can render more than once during a turn, keep it synchronous and free of external side effects."

The rendered configuration is observable verbatim as the `agent.rendered` event `data` (https://docs.opencomputer.dev/agents/events):

| `agent.rendered` data field | Meaning |
| --- | --- |
| `renderId` | Identifies the render |
| `instructions` | The string the function returned |
| `model` | Selected model |
| `enabledTools` | Tools selected by this render |
| `enabledSubagents` | Subagents selected by this render |
| `enabledMcpServers` | MCP servers selected by this render |
| `requiredConnections` | Declared outbound connections required |
| `input` | The input for this render |
| `tools` | (listed verbatim alongside `enabledTools`; the distinction is **not documented**) |
| `renderedAt` | Timestamp |

"One turn can carry several renders, one per model step."

Governing separation: "A deployment packages the agents and resources that are allowed to exist. A render selects which of those resources are available for one model step" (https://docs.opencomputer.dev/agents/mental-model).

### Tool definition (verbatim field table)

https://docs.opencomputer.dev/agents/tools

| Field | Required | Purpose |
| --- | --- | --- |
| `name` | yes | ID the model calls; letters, numbers, underscores, and hyphens only |
| `description` | yes | Explains what the tool does and when to use it |
| `input` | no | JSON Schema for tool arguments |
| `output` | no | JSON Schema describing the result |
| `run` | yes | Synchronous or asynchronous implementation |

`run` receives: `input`, `sessionId`, `messageId`, `agentId`, `signal` (optional cancellation signal), `reportProgress(metadata)`. "Tool results and progress metadata must be JSON-compatible values." "Tool code runs in the managed agent runtime, not in the browser." `useTool()` also accepts a tool ID string, e.g. `useTool("web-search")`, "when the runtime already provides that tool" (the full inventory of runtime-provided tool IDs is **not documented**; only `web-search` appears in examples).

### MCP server definition (verbatim)

https://docs.opencomputer.dev/agents/mcp

| Field | Required | Purpose |
| --- | --- | --- |
| `id` | yes | Stable ID for the server in the deployment |
| `url` | yes | HTTPS MCP endpoint |
| `connection` | no | Secret-backed HTTP connection used to authorize the server |

"Managed MCP URLs must use HTTPS." `useMcpServer()` accepts a definition or an existing server ID. Only remote HTTPS MCP is documented; stdio/local MCP is **not documented**.

### Skills

Declared purely by filesystem convention: `opencomputer/agents/<agent-id>/skills/<skill-name>/SKILL.md`, with YAML frontmatter `name` and `description`, plus arbitrary supporting files. "Skills in an agent's `skills/` directory are packaged with that agent and can be loaded by the runtime when their description matches the task" (https://docs.opencomputer.dev/agents/reactive-agents). "To reuse a skill, copy or generate the skill directory into each agent that needs it" (https://docs.opencomputer.dev/agents/skills). There is no skill registry, no versioning, and no cross-agent sharing.

### Subagents

`useSubagent("<agent-id>")` where the value "is the agent ID from `opencomputer/project.ts`". Delegation is intra-project only: "A subagent is another agent in the same project that can take a focused task." The delegated agent sees `useInput().source === "subagent"` (https://docs.opencomputer.dev/agents/subagents). **Not documented**: whether a subagent runs in its own session, whether its events land in the parent session's log, depth/fan-out limits, how results return, and whether a subagent can bind memory.

### Models

`useModel("anthropic/claude-sonnet-4.6")` or `useModel({ provider: "anthropic", model: "claude-sonnet-4.6" })`. Managed OpenRouter uses the `openrouter/provider/model` form, e.g. `useModel({ provider: "openrouter", model: "openai/gpt-5" })`. BYOK (Pro and Max plans) connects one org-level Codex account via `opencomputer model-access connect codex`; use the native `openai` provider to route through it, with automatic fallback: "If no connected and enabled account can serve the request, OpenComputer uses Managed inference when available" (https://docs.opencomputer.dev/agents/byok). The fallback is observable as the `model.access_fallback` event.

### Deployment lifecycle

| Environment | Alias | How it changes |
| --- | --- | --- |
| Development | `development` | `npm run deploy -- --watch` publishes source changes |
| Production | `production` | An explicit deploy advances it |

(https://docs.opencomputer.dev/agents/deployments)

- "A deployment is an immutable build of one agent. An alias gives a stable name to the deployment clients should run."
- "Every deploy first runs `opencomputer doctor`, a local static scan for invalid connection origins, misplaced tools, inconsistent channel declarations, and missing secret declarations." `opencomputer doctor --json` gives structured diagnostics.
- Deploy to production: `npm run deploy -- --alias production`.
- Clients address `agent-id@alias`, e.g. `support@development`, `support@production`.
- Linking source to a cloud project is explicit: `opencomputer link --project <id|slug>` or `--create-project "<name>"`. "There is no project picker. If you skip linking, project-scoped commands return a structured `binding_required` error with the explicit command to run" (https://docs.opencomputer.dev/agents/projects). Local binding is cached in `.opencomputer/project.json`.
- Deployment read API: `GET /deployments/<id>` returns `id`, `agentId`, `alias`, `memory` declarations, `createdAt`.

## Sessions

### What a session is and what it pins

"A session is a durable conversation with a deployed agent. Each turn appends input and streamed runtime events to the session" (https://docs.opencomputer.dev/agents/sessions). Pinning is explicit and total for code: "A session pins the deployment it was created on. Advancing Development or promoting to Production changes which deployment new sessions get; turns sent to an existing session keep running the code it started with, including its declared tools and memory resources. To run new code, start a new session." The one exception is memory data: "Memory documents are not pinned: they belong to the project and environment, so a new session bound to the same document reads what the old one saved."

### Create: full field list (verbatim)

`POST https://app.opencomputer.dev/api/managed-agents/sessions`, header `x-api-key`, optional header `Idempotency-Key` (at most 256 characters) (https://docs.opencomputer.dev/agents/api).

| Field | Required | Meaning |
| --- | --- | --- |
| `agentId` | one of `agentId`/`deploymentId` | "`<agent-id>@development` or `<agent-id>@production`. The alias selects the environment and the deployment active in it. A bare agent ID means `production`." |
| `deploymentId` | one of `agentId`/`deploymentId` | "Pin one deployment instead of resolving an alias. Send `environment` with it; it may be omitted only when the deployment is promoted to exactly one environment." |
| `environment` | conditional | "`development` or `production`. Optional with `agentId`; it must agree with the alias." |
| `memory` | no | "Bindings keyed by resource ID, at most eight. Needs a deployed agent and an environment." |
| `source` | no (default `api`) | "`api` (default), `playground`, `channel` or `webhook`. The dashboard groups sessions by it." |

Response: `{ session: { id, executionMode, status, createdAt }, deployment }`. `201` on create, `200` "when the key had already created it". "The session starts without a turn; send one with the turns route." The meaning of `executionMode` and its value set are **not documented anywhere in the fetched pages**.

Create error codes (verbatim): `400 invalid_environment`, `400 invalid_memory_binding`, `404 deployment_not_found`, `409 deployment_not_promoted` ("a pinned deployment is not active in the named environment"), `409 idempotency_conflict`, `402 insufficient_credits`, `503 memory_admission_unconfirmed`.

Idempotency identity rule (verbatim): "The same key with the same agent, deployment, environment and memory bindings returns the existing session with `200`. Anything else under that key is `409 idempotency_conflict`. The deployment is part of the identity, so after a redeploy the same key conflicts: a session pins its deployment, and new code needs a new session under a new key."

### Session object (`GET /sessions/<id>`)

| Field | Meaning |
| --- | --- |
| `id`, `agentId`, `deploymentId` | The session and the deployment it pins |
| `environment` | `development` or `production`, when the request named one |
| `status` | `new`, `connecting`, `idle`, `running`, `waiting_runtime`, `suspending`, `suspended`, `resuming`, `failed` or `ended` |
| `source` | The `source` given at creation |
| `memory` | Bindings with `resource`, `scope`, `id`, `access` and `writable` |
| `turns` | Every turn: `id`, `input`, `mode`, `status`, `createdAt`, `updatedAt`, and `deliveries` |
| `createdAt`, `updatedAt` | Timestamps |

`GET /sessions` returns `{ sessions }`: "the fifty most recently updated sessions of the organization, newest first, in the same shape. There are no filters or paging parameters; keep your own index of session IDs."

Session states: ten values as listed above. Terminal states: the docs name `ended` (via `POST /sessions/<id>/end`; "Ending an ended session returns it unchanged") and `failed` (`session.failed`: "The session cannot continue"). Whether `failed` is recoverable is **not documented**.

End and interrupt:
- `POST /sessions/<id>/end`: "Queued and running turns are cancelled and memory write access is revoked; do not send further turns to it. `503 session_end_unconfirmed` means the session is ended but the memory revocation was not acknowledged; retry, or freeze the document."
- `POST /sessions/<id>/interrupt`: "stops the running turn without spending a model turn. The turn settles as `cancelled` with reason `interrupted`, the runtime is told to stop, and the next queued turn starts. An idle session is returned unchanged."

### Turns

`POST /sessions/<session-id>/turns`:

| Field | Required | Meaning |
| --- | --- | --- |
| `input` | yes | "The user text for this turn. Required, not empty." |
| `idempotencyKey` | no | "The same key returns the existing turn; without one every request starts a turn." |
| `mode` | no (default `queue`) | see enum below |

`mode` enum, verbatim:
- `queue` (default): "run after earlier turns."
- `steer`: "deliver into the running turn when the runtime supports it, otherwise queue."
- `interrupt`: "cancel running turns and run next."

Response: `202 { turnId, status, duplicate }` for a new turn; `200` with `duplicate: true` when the key had already created one. `status` is `queued` or `running`. "Follow the turn in the event log; the response does not wait for it."

Turn error codes: `400 invalid_turn`, `402 insufficient_credits`, `409 memory_admission_pending` ("retry the session creation with its key first"), `409 memory_admission_rejected` ("the session is ended; create a new one").

Turn status values observed: `queued`, `running`, `cancelled` (with reason `interrupted`); plus the terminal event types `turn.completed`, `turn.failed`, `turn.cancelled`. A single canonical turn-status enum is **not published**.

### Event log

`GET /sessions/<id>/events?after=<seq>` returns `{ events }`.

Envelope (verbatim, https://docs.opencomputer.dev/agents/events):

| Field | Meaning |
| --- | --- |
| `seq` | "Position in the log, starting at 1 and increasing with every event" |
| `id` | "A unique event ID" |
| `timestamp` | "When the event was recorded" |
| `sessionId` | "The session" |
| `turnId` | "The turn the event belongs to; absent on session-level events" |
| `type` | "One of the types below" |
| `data` | "Fields specific to the type" |

Cursor and paging rules (verbatim): "`seq` is the cursor. A read with `after=<seq>` returns events with a greater `seq`, in ascending order, up to 500 at a time; repeat from the last `seq` you received until a page is empty, and keep polling from there to follow a live session." "A full page means more may follow; read on without waiting." "Events at or below the cursor never change. New types can appear; treat an unknown type as informational and keep reading." "a page that overlaps one already read is harmless: apply events whose `seq` is greater than what you have applied."

Durability: "The log is durable, so a consumer that stops can resume from its cursor and miss nothing." Transport is polling only. There is no documented SSE/WebSocket stream for the event log; the React hook's default poll interval is `pollIntervalMs` "default 1000, 500 while a turn runs" (https://docs.opencomputer.dev/agents/react).

Event types, verbatim, with `data`:

Session lifecycle: `session.created` (`agentId`, `deploymentId`; "Always the first event"), `session.status_changed` (`from`, `to`), `session.ended` (none), `session.failed` (`code`, `message`).

Turns: `message.received` (`input`, `mode`), `turn.queued` (`mode`), `turn.steered` (`activeTurnId`), `turn.interrupted` (`interruptedTurnIds`), `turn.started` (none), `turn.completed` (none), `turn.failed` (`code`, `message`, and `model` or `tool` when named), `turn.cancelled` (`reason`: `interrupted`; `replacementTurnId`).

Messages: `message.delta` (`text` fragment), `message.completed` (`text` whole reply), `reasoning.delta` (`text`), `reasoning.completed` (`text`).

Tools: `tool.started` (`tool`, `callId`, `title`, `input`), `tool.progress` (runtime-defined), `tool.completed` (`tool`, `callId`, `title`, `output`), `tool.failed` (`tool`, `callId`, `message`). "read them as optional and key a tool call on `callId` when it is present."

Memory: `memory.saved` (`resource`, `documentId`, `revision`, `bytes`).

Model and usage: `model.route_resolved` (`providerCallId`; `requested` and `effective` as `{ provider, model }`; `runtime`; `access`), `model.access_fallback` (`providerCallId`, `requested`, `from`, `reason`), `usage.recorded` (`provider`, `model`, `inputTokens`, `outputTokens`, `reasoningTokens`, `cachedTokens`, `cacheWriteTokens`, `costUsd`, `payer`).

Outbound: `egress.request` (`connectionId`, `method`, `path`), `egress.response` (+ `status`, `durationMs`), `egress.failed` (+ `message`).

Runtime: `runtime.connected`, `runtime.disconnected` ("a running turn is queued again and resumes on the next runtime"), `runtime.suspended`, `runtime.resumed`, `runtime.log` (`level`, `stream`, `message`, or `phase` and `message`).

Renders: `agent.rendered` (fields listed in the Agents section).

Public failure codes (the closed set for `session.failed` and `turn.failed`; "The runtime's own error text is never sent; a failure no rule recognizes is `agent_failed`"):
`interrupted`, `session_ended`, `runtime_lost`, `runtime_failed`, `deployment_invalid`, `model_unavailable` (param `model`), `model_rejected`, `context_too_long`, `tool_failed` (param `tool`), `sandbox_timeout`, `sandbox_failed`, `agent_failed`.

### session-data versus memory versus document-memory

| | `useSessionData(key)` | Memory / `documentMemory` |
| --- | --- | --- |
| Scope | One session | Project **and** environment |
| Written by | "your application" | The model via `memory_save`, and the owner via CLI/dashboard/API |
| Read by agent | `useSessionData<T>("key")`, returns `undefined` when absent | `useMemory(resource)` returns `{ text, sources, writable }` |
| Write from agent code | "does not yet include a hook for setting or updating a value from agent code" | via model tools only |
| Lifetime | The session | "survives session end, sandbox loss, conversation compaction and new deployments" |

"Memory" is the concept page; "Document memory" is the built-in provider `documentMemory` plus its owner APIs; "Memory providers" describes provider responsibilities and a **preview-only** `httpMemory` contract ("HTTP memory is a contract preview: `httpMemory` declarations compile, but namespace bindings and HTTP execution are unavailable").

### Memory admission gate (exact codes)

Binding is an admission decision made at session creation, separate from document existence:
- "Resources must be declared by the deployment and bound documents must exist. Invalid bindings fail admission." "Bindings stay fixed. Schedules, channels and webhooks do not configure them." Max 8 bindings per session.
- On create: `503 memory_admission_unconfirmed` = "the memory grants were not confirmed in time. Retry with the same key; the retry resumes the wait."
- On turn: `409 memory_admission_pending` = "retry the session creation with its key first".
- On turn: `409 memory_admission_rejected` = "the session is ended; create a new one".
- On end: `503 session_end_unconfirmed` = "the session is ended but the memory revocation was not acknowledged; retry, or freeze the document."
- Also on create: `400 invalid_memory_binding`.

Binding scopes and the tools each grants (verbatim, https://docs.opencomputer.dev/agents/document-memory):

| Scope and access | Projection | Model tools |
| --- | --- | --- |
| Document, read | Full text | None |
| Document, read-write | Full text | `memory_save` |
| Collection, read | Recent summaries | `memory_list`, `memory_read` |

"Every bound resource is recalled before render; any failure blocks inference, even if that render would omit the hook."

`memory_save` result union (verbatim TypeScript):
`{ status: "saved"; revision; bytes }` | `{ status: "conflict"; text; summary }` | `{ status: "rejected"; reason: "agent_writes_disabled" | "not_found" | "session_closed" }` | `{ status: "rejected"; reason: "too_large"; field: "text" | "summary"; bytes; maxBytes }` | `{ status: "rejected"; reason: "not_bound" | "read_only"; bound: string[] }`.

Document object: `id`, `title`, `text`, `summary`, `agentWrites` (`"enabled"` | `"disabled"`), `revision` (opaque), `bytes`, `maxBytes`, `updatedAt`, `writer` (`{ kind: "owner" }` or `{ kind: "agent", sessionId }`). Owner routes use `ETag` / `If-Match` / `If-None-Match: *`. Memory HTTP error codes: `400 invalid_request`, `404 not_found` / `project_not_found`, `412 precondition_failed`, `413 memory_limit_exceeded`, `428 precondition_required`.

Limits: Resource ID 1-128 lowercase letters/digits with single hyphen separators; Document ID 1-128 ASCII letters, digits, `-`, `_`; `maxBytes` integer 1-16,384 (default 8,192 UTF-8 bytes); Title 1-240 bytes; Summary 0-240 bytes; mutation JSON body 64 KiB; bindings per session 8; collection overview 50 entries within 16 KiB; list page 50 documents.

### Event subscriptions (agent to agent delivery)

"An event subscription delivers the recorded outcome of turns run by agents in a project to a session in the same project, as a new turn of that session." "Destinations are sessions only; there is no public HTTPS destination, and this is not the outbound webhooks feature" (https://docs.opencomputer.dev/agents/api).

Create body:

| Field | Required | Meaning |
| --- | --- | --- |
| `agentId` | no | "Outcomes of this agent only; omitted, every agent in the project." |
| `events` | yes | "One or more of `turn.completed`, `turn.failed`, `turn.cancelled`. Turn outcomes, not session ends." |
| `destination` | yes | "`{ type: "session", sessionId }`: a session of an agent in this project that can still take turns." |
| `environment` | yes | "`development` or `production`. A subscription is scoped to one environment." |

"A subscription is immutable; `{ subscription }` carries `id`, `projectId`, the fields above and `createdAt`." Codes: `400 invalid_event_subscription`, `403 project_scope_violation`, `404 destination_session_not_found`, `409 destination_session_ended`, `404 event_subscription_not_found`. `DELETE` returns `204`; "pending deliveries stop".

Idempotency key construction (verbatim): "The destination turn's idempotency key is `<subscription-id>:<event-id>`, so a retried delivery never starts a second turn. Deliveries are retried with backoff and stop when the subscription is deleted or the destination has ended."

Delivery record fields (verbatim), surfaced on the **source** session's turns as `deliveries`:

| Field | Meaning |
| --- | --- |
| `id` | `<subscription-id>:<event-id>` |
| `subscriptionId`, `eventId`, `eventType` | What was delivered, to which subscription |
| `destination` | The subscription's destination |
| `status` | `pending`, `delivered` or `failed` |
| `attempt` | Delivery attempts so far |
| `receipt` | `{ sessionId, turnId }`: the turn the destination admitted, once delivered |
| `nextAttemptAt` | When a pending delivery is retried |
| `error` | `subscription_unavailable`, `target_missing`, `target_ended`, or `delivery_failed` |

Received shape at the destination: `useInput().source === "event"` and `input.event` with `event.id`, `event.type`, `event.sessionId`, `event.turnId`, `event.agentId`, `event.occurredAt`, `event.reason`, `event.error`, `event.result` = `{ text, truncated }` bounded to 16 KB. "An event input carries no `text`; read `event`."

## Secrets

There are **two distinct secret systems** on this docs site.

### A. Serverless Agents managed secrets (the `/agents/secrets` model)

Definition: "OpenComputer secrets are write-only values used by declared outbound connections. Plaintext values are not included in source bundles, prompts, deployment manifests, runtime environment variables, logs, or API responses" (https://docs.opencomputer.dev/agents/secrets).

Scope: project-level by default, with per-agent override, separate per environment.

| Property | Value |
| --- | --- |
| Primary scope | Cloud project ("Project secrets are available to declared connections in every agent in the project") |
| Override scope | One agent (`--agent current`) |
| Environment | `development` and `production` are separate stores |
| Readable back | No. "Secret values are never returned." List output has "the name, scope, environment, and allowed origins" only |
| Set | `printf %s "$TOK" \| opencomputer secrets set NAME --value-stdin` (value accepted "only from piped standard input") |
| List / remove | `opencomputer secrets list --environment development`, `opencomputer secrets remove NAME --environment development` |
| Dev sync | `opencomputer/.env.local` (gitignored) + `opencomputer/.env.example` for names; `opencomputer doctor` compares local names with `useSecret()` references. "Upload is always explicit; deployment never opens a value prompt." |

Declaring the outbound destination, verbatim example:

```
const github = defineConnection({
  id: "github-api",
  origin: "https://api.github.com",
  methods: ["GET"],
  pathPrefix: "/repos/",
  headers: {
    Authorization: bearer(useSecret("GITHUB_TOKEN")),
  },
});
```

| `defineConnection` field | Observed in docs | Meaning |
| --- | --- | --- |
| `id` | yes | Connection identifier, appears in `egress.*` events as `connectionId` and in `agent.rendered.requiredConnections` |
| `origin` | yes | "The origin must use HTTPS." |
| `methods` | yes | Allowed HTTP methods (array) |
| `pathPrefix` | yes | Allowed path prefix |
| `headers` | yes | Header map; secret references via `useSecret()` wrapped in helpers such as `bearer()` |

A formal required/optional table for `defineConnection` is **not published**; `methods` and `pathPrefix` are omitted in the MCP example (https://docs.opencomputer.dev/agents/mcp), which implies they are optional, but the docs do not state their defaults. Helper functions other than `bearer()` are **not documented**.

The table above is derived from the documentation only. The published `@opencomputer/agent` package's own type declarations were not inspected for this snapshot, and a package's types can carry fields the docs site has not caught up with, so the table is a lower bound on the contract rather than an inventory of it. Redirect handling in particular is undocumented: no page in the agents documentation set states whether the connection `fetch` follows redirects, whether a redirect target is revalidated against the declared `origin` and `pathPrefix`, or whether managed credential headers are stripped before a redirect is followed. Given "The origin must use HTTPS." and "The credential is attached only after the destination, method, path, agent, project, and environment have been validated.", a redirect that bypassed revalidation would defeat both, which makes the silence worth flagging rather than reading as permissive.

How a secret reaches running code: it does not. Code calls `github.fetch("/repos/opencomputer/example")`. "Only a relative path is accepted. OpenComputer checks the declared origin, path prefix, method, agent, and environment before sending the outbound request." "The credential is attached only after the destination, method, path, agent, project, and environment have been validated."

Undeclared destinations: an undeclared destination has no injection path at all, because injection happens inside the connection's own `fetch`. A plain `fetch()` in a tool to an arbitrary host is allowed (the Hacker News tool example does exactly that, https://docs.opencomputer.dev/agents/reactive-agents) but simply carries no credential. The docs state the negative controls explicitly: "Variables without a matching declaration are skipped. OpenComputer never grants an unmatched value access to every host." "OpenComputer rejects hard-coded sensitive headers such as `Authorization`, `Cookie`, and `X-API-Key`; reference a managed secret instead." "Request headers that could override managed credentials are discarded before the outbound request is sent" (https://docs.opencomputer.dev/agents/logs). A general network egress allowlist for the agent runtime is **not documented**.

Deletion semantics worth noting: "removing a local variable does not delete its cloud value. Use `opencomputer secrets remove` when deletion is intentional."

### B. Agent runtime variables (the escape hatch)

Same page, deliberately contrasted. `opencomputer env set DATABASE_URL --value-stdin` / `env list` / `env remove`, project-wide or `--agent current`, separate per environment, "No declaration in agent source is needed." "A newly started agent runtime receives the resolved values in its process environment, so agent code, tools, commands, and child processes can read them. Because the agent can access the plaintext, runtime variables are appropriate for personal-agent credentials such as `DATABASE_URL`, but they do not provide the destination isolation of managed secrets. Restart a running agent runtime after changing a value." Values are "stored encrypted" and "never returned through the dashboard or management API".

### C. Sandbox secrets (a different product, different mechanism)

Sandboxes use **secret stores** attached at sandbox creation: `Sandbox.create({ secretStore: "prod-keys" })`. "The environment variable holds a **sealed placeholder**, never your secret" (literally `osb_sealed_...`). "When the sandbox makes a request to a host that secret is scoped to, a proxy swaps the real value into the outbound request" (https://docs.opencomputer.dev/sandboxes/secrets). Properties claimed: the sandbox never holds the value; "Bypass fails closed" (direct dialling sends the worthless placeholder); substitution is host-scoped; "Rotation needs no restart".

The stated architectural change on the v2 runtime: "The proxy moved **from a host outside your sandbox to a root-owned process inside it.**" Consequence, verbatim warning: "The difference is the blast radius of a **privilege escalation inside your sandbox**. On the current runtime, root in the guest still could not reach the proxy. Here, it can reach the secrets scoped to that sandbox." Rejected alternative, with reasoning: "The obvious alternative was a shared proxy service per region. We rejected that: it would be a single service holding **every customer's** secrets and a single point of failure on every customer's egress path... Concentrating everyone's secrets to mitigate a single-tenant risk is a worse trade."

Sandbox secret API surface (https://docs.opencomputer.dev/reference/typescript-sdk/secrets):
- `SecretStore.create({ name, egressAllowlist })`, `.list()`, `.get(storeId)`, `.update(storeId, { name, egressAllowlist })`, `.delete(storeId)`.
- `SecretStore.setSecret(storeId, name, value, { allowedHosts })`, `.listSecrets(storeId)` ("Returns secret metadata only. Values are never exposed"), `.deleteSecret(storeId, name)`.
- `SecretStoreInfo`: `id`, `orgId`, `name`, `egressAllowlist`, `createdAt`, `updatedAt`. `SecretEntryInfo`: `id`, `storeId`, `name`, `allowedHosts`, `createdAt`, `updatedAt`.
- Rotation: `PUT /api/secret-stores/{id}/secrets/{name}` with `{"value": "new-value"}`; the response "now reports how many sandboxes were actually refreshed" (`refreshed`) and the docs say to check that number rather than the status code.
- Layering on a checkpoint fork: "secrets merge (fork's store wins on collision) and egress allowlists aggregate."
- "A store can restrict outbound HTTPS to a set of hosts. Requests to anything else are refused... The sandbox also cannot use the proxy to reach cloud instance metadata."

Key contrast: the Agents model scopes a credential to `(origin, method, pathPrefix, agent, project, environment)` and requires a *static source declaration*; the Sandbox model scopes to `(store, allowedHosts)` chosen at sandbox creation and injects a placeholder into the process environment.

### Third-party OAuth

There is **no third-party OAuth account feature for Serverless Agents**. The docs say so directly: "`defineConnection()` declares a secret-backed HTTP destination. It is not an OAuth account selector and does not itself give the model a tool" (https://docs.opencomputer.dev/agents/capabilities). The only OAuth flows documented anywhere are (a) BYOK connecting an organization Codex account for model inference (https://docs.opencomputer.dev/agents/byok) and (b) Slack app installation for channels, whose credentials live in per-environment dashboard configuration (https://docs.opencomputer.dev/agents/channels). Per-end-user delegated OAuth is not documented.

## Identity and versioning

- **Project**: `{ id, slug, name, environments, agents, createdAt, updatedAt }`. Example IDs look like `prj_...`. Addressed by ID or slug in CLI (`--project <project-id-or-slug>`), by `<p>` in API paths.
- **Agent**: string ID equal to its source directory name under `opencomputer/agents/` and listed in `opencomputer/project.ts`. Listed via `GET /agents` as `id`, `name`, `activeAlias`, `activeDeploymentId`, `deploymentCount`.
- **Deployment**: opaque `id`, immutable, one per agent build. Two ways to address the code a session runs: the alias form `<agent-id>@<alias>` (late binding, resolved at session creation) or `deploymentId` (early binding, pinned). "A bare agent ID means `production`."
- **Alias**: exactly two per project, `development` and `production`, each pointing at one deployment. Aliases move; deployments never change.
- **Environment**: `development` | `production`. It is a hard partition for secrets, runtime variables, memory stores, schedule state, webhook URLs/tokens, channel credentials and destination bindings, outbox delivery history, and event subscriptions.
- **Session**: opaque `id`. Pins `deploymentId` at creation. Identity for retry is `Idempotency-Key` scoped to the organization, with the tuple (agent, deployment, environment, memory bindings) forming the equality check.
- **Turn**: opaque `id`, optional caller-supplied `idempotencyKey`.
- **Event**: `id` plus monotonically increasing `seq` per session, starting at 1. `seq` is the only cursor.
- **Memory resource**: application-chosen `id` (1-128 lowercase letters/digits, single hyphen separators), declared in code, registered by deployment; "Renaming an ID requires migration; reusing it addresses the same stored data." Retired resources "remain discoverable, editable and exportable."
- **Memory document**: application-chosen `id`, plus an opaque `revision` exposed as a quoted `ETag`. "Deletion removes title, text and summary but reserves the ID against delayed recreation."
- **Webhook**: `<id>` plus a `token` in the URL path (`/api/agent-webhooks/<id>/<token>`), example token prefixes `wh_...` and `ocwh_...`; rotatable, shown once.
- **Event subscription**: `id`, immutable; derived delivery id `<subscription-id>:<event-id>`.
- **Skill**: directory name plus frontmatter `name`; no version, no registry.
- **Model**: `provider/model` string, e.g. `anthropic/claude-sonnet-4.6`, `anthropic/claude-haiku-4.5`, `openai/gpt-5.6-sol`, `openrouter/openai/gpt-5`.

## Notable design decisions

1. **The agent is a pure function re-rendered per model step, not a stored config record.** This is the central bet. It buys conditional capability attachment ("a capability can be attached only when the current request needs it") and makes the effective configuration auditable per step via `agent.rendered`. It costs: the function must be synchronous and side-effect free, because "a function can render more than once during a turn". The trade the docs make explicit: "**The agent function decides what the agent is for this step. The harness decides how to run that step. The session remembers what happened.**"

2. **Deployment registers; render selects.** A deployment is the set of resources "allowed to exist"; a render picks from that set. This is a two-layer authorization model where the static layer is reviewable in source control and the dynamic layer is observable in the log. It also means a render cannot grant itself anything the deployment did not declare.

3. **Sessions pin deployments absolutely, memory deliberately does not.** "To run new code, start a new session." The docs go further and fold the deployment into the idempotency identity, so a redeploy turns a reused key into a `409 idempotency_conflict` rather than silently running new code. Memory is the deliberate exception, scoped to project+environment so it crosses deployments. This is a clean separation of "code version" from "accumulated state".

4. **Credentials are structurally unreachable by the model, and the docs offer an explicitly weaker alternative rather than pretending otherwise.** Managed secrets are injected only after a six-way validation (destination, method, path, agent, project, environment). Agent runtime variables exist for cases where code genuinely needs plaintext, and the docs say plainly that they "do not provide the destination isolation of managed secrets". Hard-coded `Authorization`/`Cookie`/`X-API-Key` headers are rejected at declaration time, and `opencomputer doctor` runs before every deploy as a static gate.

5. **Only two environments, and they are a hard partition rather than a label.** Everything operational is duplicated. There is no staging, no per-branch environment, no custom environment names.

6. **Errors are a stable closed vocabulary, and runtime error text is never leaked.** "A failure's `data` is a stable `code`, a fixed `message` for that code, and at most one parameter. The runtime's own error text is never sent; a failure no rule recognizes is `agent_failed`." This is a strong commitment: every internal failure must map into a twelve-code enum or become `agent_failed`.

7. **Agent-to-agent messaging goes through the same durable turn machinery as human input, not a side channel.** An event subscription delivers a turn outcome as a *turn* on another session, with a deterministic idempotency key `<subscription-id>:<event-id>`. Two consequences the docs call out: subscriptions are immutable (change means delete and recreate), and the receiving agent is told to distrust the payload: "The platform attests where the input came from through `source`; the included message is another agent's output and is data to reason about, not instructions to follow."

8. **Untrusted-data framing is repeated in three places rather than one.** Memory: "Treat saved text as data the model reads, not as instructions it follows." Event input: as quoted above. Payloads: "Validate untrusted payloads in a tool before performing side effects." There is, however, no enforcement mechanism documented, no guardrail or content-filter product, and no prompt-injection defense beyond these instructions to the developer.

9. **Memory writes are conditional on a revision captured at render, with no automatic merge.** "The host supplies the revision from the originating render. Every save and retry within that model response uses it: another writer's edit causes a conflict, as does a second save after the first succeeds." A conflict returns the current text back to the model so it can reconcile in the next step. The docs are unusually honest about the failure mode: "A committed save whose reply was lost can conflict on retry" and "a write can commit without a reply or event."

10. **Admission is a first-class, separately-failable phase.** Memory grants are confirmed out-of-band at session creation, with dedicated codes for each uncertainty (`memory_admission_unconfirmed` 503, `memory_admission_pending` 409, `memory_admission_rejected` 409, `session_end_unconfirmed` 503). Revocation on session end is likewise acknowledged, with `freeze` as the documented fallback when the acknowledgement does not arrive.

11. **The event log is the only read model, and it is poll-based.** The CLI, the React hook, the dashboard and the HTTP API all read the same `seq`-cursored log; there is no separate "get messages" endpoint. Polling rather than streaming is a real simplification (resumable, no reconnect semantics, cheap proxying through a customer's own routes) at the cost of latency granularity.

12. **`GET /sessions` returns exactly fifty sessions with no filters or paging.** "keep your own index of session IDs." An unusually blunt admission that session discovery is the caller's problem.

13. **The React integration refuses to hold the API key and refuses to own session lifecycle.** The browser proxies exactly three routes through the customer's own auth (`events`, `turns`, `interrupt`). "The hook never suspends, resumes or ends an attached session. Its lifecycle belongs to the server that created it." Memory bindings are chosen server-side so "the browser can neither see the key nor change what the session may read or write."

14. **Idempotency identity is configurable at the ingress edge for providers that cannot set headers.** Webhooks accept `identity` as `header:<name>` or `body:<json-pointer>`, with a blocklist of headers the platform sets or strips (`Authorization`, `Cookie`, `Idempotency-Key`, `X-Request-Id`, `X-Api-Key`, transport headers, and prefixes `x-oc-`, `cf-`, `x-forwarded-`, `x-real-`). The docs give a concrete reasoning example: Sentry's `Request-ID` changes on retry, so use `body:/data/event/event_id`.

15. **Provenance is explicitly not authorization.** On schedules: "The platform sets `input.source` to `"schedule"` as provenance, but transport provenance should not be the agent's business-mode switch." Use a payload field such as `mode` instead.

16. **Slack/Twilio/email channels are named after vendors, not media, with stated reasoning**: "Twilio signs the request URL and its sorted parameters, Slack signs a timestamp and the raw body. A different SMS vendor is a different adapter, not a variant of this one."

17. **The sandbox secret proxy was moved into the guest and the docs publish the resulting weakening.** See Secrets section C. Publishing a threat-model regression with the rejected alternative and its reasoning is notable.

## Gaps and open questions

- **`executionMode`** appears in the session create response (`{ session: { id, executionMode, status, createdAt } }`) and is never defined. Its value set and meaning are not documented anywhere in the fetched pages.
- **Writing session data is not documented.** `useSessionData` reads values "written by your application", but no route, SDK call, CLI command, or session-create field for setting them appears on `/agents/api`, `/agents/session-data`, or any other fetched page. The docs only say agent code cannot write: "The current `@opencomputer/agent` API exposes session data as a read-only snapshot."
- **Subagent mechanics.** Not documented: whether a subagent gets its own session and event log, how its result returns to the parent, nesting depth or fan-out limits, whether it can bind memory, whether its usage is attributed separately, and whether subagent delegation appears in the parent's event log (no `subagent.*` event type is listed).
- **Whether `useModel` is required**, and what model is used when it is omitted.
- **The inventory of runtime-provided tool IDs.** `useTool("web-search")` and `useTool("github-search")` appear in examples; no catalogue exists.
- **`defineConnection` formal contract.** No required/optional table; defaults for `methods` and `pathPrefix` when omitted are unstated; helper functions besides `bearer()` are unlisted; whether multiple headers or non-header injection (query param, body) is supported is unstated.
- **General egress policy for the agent runtime.** Whether a tool's plain `fetch()` to an arbitrary host is permitted, blocked, or allowlisted is not stated. The Hacker News example implies it is permitted, but no policy is published.
- **`agent.rendered` carries both `enabledTools` and `tools`;** the difference is not explained.
- **Turn status enum.** `queued`, `running`, `cancelled` are named in prose; the full set (is there a `completed`/`failed` status distinct from the events?) is never given as an enum.
- **Whether `failed` is a terminal session state**, and whether a `failed` session can accept further turns or be recovered.
- **Rate limits and quotas.** `429` is listed with `Retry-After`, but no documented limits on sessions, turns per session, event log size, session retention, or concurrent runtimes.
- **Session and event retention.** Nothing states how long sessions or their event logs persist.
- **Secret value size limits, naming rules, and rotation semantics** for Agents-side managed secrets are not documented (the Sandboxes-side rotation endpoint is documented, but that is the other system).
- **No documented API for secrets, runtime variables, schedules, channels, outboxes or logs**; the docs say they are "managed with the CLI and the dashboard" only.
- **Delegated/end-user OAuth** is absent, and explicitly disclaimed for connections.
- **Guardrails as a product feature do not exist.** There is no content filter, no output validation, no tool-approval/human-in-the-loop gate, no policy engine. `useInput()` is explicitly "not a human-in-the-loop prompt" and "does not pause the session to ask a person a question". Untrusted-data handling is developer advice only.
- **`/agents/development` returns 404** despite being in the task list; the equivalent content is split across `/agents/quickstart` and `/agents/mental-model`.
- **`/reference/cli/agent` describes an incompatible surface**: `oc agent invoke`, `agent.toml`, named "Hook URLs" with `--expires-at` and revocation, a "one-hour session client token", and a "Durable Agent Sessions API". None of this reconciles with `/agents/*` (which uses `opencomputer webhooks`, `opencomputer/project.ts` and `x-api-key`). It also links to `/agent-sessions/overview`, which is not in the sitemap. This looks like a stale or parallel product surface; I could not verify which is current.
- **`createFromCheckpoint` is documented as both working and broken** across pages: `/sandboxes/overview` and `/reference/typescript-sdk/secrets` show it in use, while `/sandboxes/checkpoints` says "`createFromCheckpoint` fails on v2." Sandboxes-side only, but it means sandbox pages mix v1 and v2 statements.
- **What "sandbox" means to Serverless Agents.** The failure codes `sandbox_timeout` and `sandbox_failed` exist and memory operations "start no sandbox", implying agent runtimes sometimes run on sandboxes, but the relationship between an agent runtime and a Sandbox product sandbox (including whether the 8-hour ceiling applies) is never stated.
- **`memory.saved` `revision` versus the document `revision`/`ETag`** are presumably the same opaque token, but this is not stated.
- **Event subscription fan-out limits**, retry backoff schedule, and how long a `pending` delivery is retried before `failed` are not documented.

## Sources

Fetched as markdown (`.md` appended) on 2026-09-11:

- https://docs.opencomputer.dev/sitemap.xml
- https://docs.opencomputer.dev/introduction.md
- https://docs.opencomputer.dev/how-it-works.md
- https://docs.opencomputer.dev/agents/overview.md
- https://docs.opencomputer.dev/agents/mental-model.md
- https://docs.opencomputer.dev/agents/quickstart.md
- https://docs.opencomputer.dev/agents/projects.md
- https://docs.opencomputer.dev/agents/deployments.md
- https://docs.opencomputer.dev/agents/reactive-agents.md
- https://docs.opencomputer.dev/agents/hooks.md
- https://docs.opencomputer.dev/agents/sessions.md
- https://docs.opencomputer.dev/agents/api.md
- https://docs.opencomputer.dev/agents/events.md
- https://docs.opencomputer.dev/agents/capabilities.md
- https://docs.opencomputer.dev/agents/secrets.md
- https://docs.opencomputer.dev/agents/inputs.md
- https://docs.opencomputer.dev/agents/models.md
- https://docs.opencomputer.dev/agents/tools.md
- https://docs.opencomputer.dev/agents/mcp.md
- https://docs.opencomputer.dev/agents/skills.md
- https://docs.opencomputer.dev/agents/subagents.md
- https://docs.opencomputer.dev/agents/session-data.md
- https://docs.opencomputer.dev/agents/memory.md
- https://docs.opencomputer.dev/agents/document-memory.md
- https://docs.opencomputer.dev/agents/memory-providers.md
- https://docs.opencomputer.dev/agents/react.md
- https://docs.opencomputer.dev/agents/channels.md
- https://docs.opencomputer.dev/agents/outboxes.md
- https://docs.opencomputer.dev/agents/schedules.md
- https://docs.opencomputer.dev/agents/webhooks.md
- https://docs.opencomputer.dev/agents/byok.md
- https://docs.opencomputer.dev/agents/logs.md
- https://docs.opencomputer.dev/agents/playground.md
- https://docs.opencomputer.dev/guides/agent-skill.md
- https://docs.opencomputer.dev/sandboxes/overview.md
- https://docs.opencomputer.dev/sandboxes/secrets.md
- https://docs.opencomputer.dev/sandboxes/lifetime.md
- https://docs.opencomputer.dev/sandboxes/checkpoints.md
- https://docs.opencomputer.dev/sandboxes/signed-urls.md
- https://docs.opencomputer.dev/cli/secrets.md
- https://docs.opencomputer.dev/reference/cli/agent.md
- https://docs.opencomputer.dev/reference/typescript-sdk/secrets.md

Attempted and returned HTTP 404: https://docs.opencomputer.dev/agents/development.md
