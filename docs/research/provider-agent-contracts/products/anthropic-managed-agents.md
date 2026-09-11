# Anthropic: Sessions, Agents, Vaults

Part of the [provider agent contracts research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-09-11. Every claim is sourced inline to a
vendor documentation page or machine-readable spec. These documentation sites
are unversioned and publish no commit identifiers, so the authoritative
anchors are the pinned entry points below plus the API version header each
surface requires:

- Claude Managed Agents,
  [overview](https://platform.claude.com/docs/en/managed-agents/overview) and
  [API reference](https://platform.claude.com/docs/en/managed-agents/reference).
- Version pin: the `managed-agents-2026-04-01` beta header on every endpoint,
  except the memory store endpoints, which take `agent-memory-2026-07-22`.

Claude Managed Agents is Anthropic's server-hosted agent harness: instead of writing your own agent loop, you declare an **agent** (model, system prompt, tools, MCP servers, skills), declare an **environment** (where the sandbox runs), and then create a **session**, which is one running agent instance inside a sandbox. Work is driven entirely by an append-only **event** log: you post `user.*` events, the platform emits `agent.*`, `session.*`, `span.*` events back, over SSE or by polling. All endpoints are beta and require the `managed-agents-2026-04-01` beta header, except memory store endpoints, which use `agent-memory-2026-07-22` (source: https://platform.claude.com/docs/en/managed-agents/reference). The whole design is built around a small number of separately-owned, independently-versioned resources that a session **snapshots** at creation time, plus two sidecar resources for state (memory stores) and secrets (vaults) whose contents deliberately stay outside the agent definition.

The four core concepts, verbatim from the overview table: **Agent** ("The model, system prompt, tools, MCP servers, and skills"), **Environment** ("Configuration for where sessions run: an Anthropic-managed cloud sandbox, or a self-hosted sandbox on your own infrastructure"), **Session** ("A running agent instance within an environment, performing a specific task and generating outputs"), **Events** ("Messages exchanged between your application and the agent (user turns, tool results, status updates)"). Source: https://platform.claude.com/docs/en/managed-agents/overview

## Resource model

```
                         ORGANIZATION / WORKSPACE
  ┌──────────────────────────────────────────────────────────────────────────┐
  │                                                                          │
  │  agent (agent_...)            environment (env_...)                      │
  │  ├─ version: int32 1,2,3...   ├─ config: cloud | self_hosted             │
  │  ├─ model / system            ├─ networking: unrestricted | limited      │
  │  ├─ tools[] (+permission_     ├─ packages: apt/cargo/gem/go/npm/pip      │
  │  │   policy per tool)         ├─ scope: organization | account           │
  │  ├─ mcp_servers[]             └─ NOT VERSIONED                           │
  │  ├─ skills[] (pinned ver)                                                │
  │  ├─ multiagent.agents[]  ── pins other agents by {id, version} ──┐       │
  │  └─ archived_at                                                  │       │
  │        │                                                         │       │
  │        │ referenced by                          ┌────────────────┘       │
  │        ▼                                        ▼                        │
  │  deployment (dep_...)                     (roster agent versions)        │
  │  ├─ agent (id or {id,version})                                           │
  │  ├─ environment_id                                                       │
  │  ├─ initial_events[] (1..50, required)                                   │
  │  ├─ schedule: {type:"cron", expression, timezone}                        │
  │  ├─ budget, resources[], vault_ids[]                                     │
  │  └─ creates sessions ──────────┐                                         │
  │                                │                                         │
  │  vault (vlt_...)               │      memory_store (memstore_...)        │
  │  └─ credential (vcrd_...)      │      └─ memory  ──▶ memory version      │
  │     ├─ mcp_oauth               │         (immutable, auditable)          │
  │     ├─ static_bearer           │                                         │
  │     └─ environment_variable    │                                         │
  │              │                 │                  │                      │
  └──────────────┼─────────────────┼──────────────────┼──────────────────────┘
                 │ vault_ids[]     │                  │ resources[]
                 ▼                 ▼                  ▼
        ┌───────────────────────────────────────────────────────────┐
        │  session (sesn_...)                                       │
        │  ├─ agent      : SNAPSHOT of agent at creation time       │
        │  ├─ environment_id                                        │
        │  ├─ vault_ids[]: fixed at creation ("attached at creation")│
        │  ├─ resources[]: github_repository | file | memory_store   │
        │  ├─ budget     : only attachable at creation              │
        │  ├─ status     : running|idle|rescheduling|terminated     │
        │  ├─ usage / stats / outcome_evaluations[]                 │
        │  ├─ deployment_id (null unless created from a deployment) │
        │  │                                                        │
        │  ├── session_thread (sth_...) PRIMARY  ─── event stream ──┼──▶ SSE
        │  │     └─ parent_thread_id: null                          │
        │  └── session_thread (sth_...) child (multiagent, max 25)  │
        │        ├─ agent: snapshot at THREAD creation time         │
        │        └─ own event stream + own conversation history     │
        │                                                           │
        │  events: user.* / system.* in, agent.*/session.*/span.*   │
        │          out; each persisted event has id + processed_at  │
        └───────────────────────────────────────────────────────────┘
                 shared per session: sandbox, filesystem, vault credentials
                 NOT shared across threads: tools, MCP servers, context
```

## Agents

An agent is "a reusable, versioned configuration that defines persona and capabilities. It bundles the model, system prompt, tools, MCP servers, and skills that shape how Claude behaves during a session." (https://platform.claude.com/docs/en/managed-agents/agent-setup)

`POST /v1/agents`, `POST /v1/agents/{agent_id}` (update, note: POST not PATCH), `GET /v1/agents/{agent_id}/versions`, `POST /v1/agents/{agent_id}/archive`.

### Create body (POST /v1/agents)

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `model` | `BetaManagedAgentsModel` (string) or `BetaManagedAgentsModelConfigParams` | yes | "Model identifier. Accepts the model string, e.g. `claude-opus-5`, or a `model_config` object for additional configuration control" |
| `model.id` | `BetaManagedAgentsModel` | yes (object form) | Enum includes `"claude-fable-5-1"`, `"claude-sonnet-5"`, `"claude-fable-5"`, `"claude-opus-5"`, `"claude-opus-4-8"`, `"claude-opus-4-7"`, `"claude-opus-4-6"`, `"claude-sonnet-4-6"`, `"claude-haiku-4-5"`, `"claude-haiku-4-5-20251001"`, `"claude-opus-4-5"`, `"claude-opus-4-5-20251101"`, `"claude-sonnet-4-5"`, `"claude-sonnet-4-5-20250929"`, plus free-form `string` |
| `model.effort` | `"low"`/`"medium"`/`"high"`/`"xhigh"`/`"max"`, or `{"type": ...}` object | no | "How hard Claude works on each inference call... On create, omitting it resolves the per-model default; on update, omitting it leaves the stored value unchanged." |
| `model.inference_geo` | string or null | no | "Geographic region for model inference. When unset, requests fall through to the workspace's default_inference_geo. On update, `model` is whole-object replacement, omitting inference_geo clears it." |
| `model.speed` | `"standard"` or `"fast"` or null | no | "`fast` provides significantly faster output token generation at premium pricing. Not all models support `fast`; invalid combinations are rejected at create time." |
| `name` | string | yes | "Human-readable name for the agent." minLength 1, maxLength 256 |
| `description` | string or null | no | maxLength 2048 |
| `mcp_servers` | array of `BetaManagedAgentsURLMCPServerParams` | no | "Maximum 20. Names must be unique within the array. Every server must be referenced by an `mcp_toolset` in `tools`; unreferenced servers are rejected." Entry: `type: "url"`, `name` (1-255), `url` (maxLength 2048) |
| `metadata` | map[string] | no | "Maximum 16 pairs, keys up to 64 chars, values up to 512 chars." |
| `multiagent` | `BetaManagedAgentsMultiagentParams` or null | no | `type: "coordinator"` plus `agents` roster |
| `multiagent.agents` | array | yes (if multiagent) | "1-20 entries. Each entry is an agent ID string, a versioned `{"type":"agent","id","version"}` reference, or `{"type":"self"}`... Referenced agents must exist, must not be archived, and must not themselves have `multiagent` set (depth limit 1)." A fourth variant is `{"type":"advisor","model":...}`: "At most one per roster; the entry occupies the roster name `anthropic.advisor`." |
| `skills` | array of `BetaManagedAgentsAnthropicSkillParams` or `BetaManagedAgentsCustomSkillParams` | no | `type: "anthropic"` with `skill_id` (e.g. `"xlsx"`) or `type: "custom"` with `skill_id` (e.g. `"skill_01XJ5..."`); both take optional `version` string, "Defaults to latest if omitted" |
| `system` | string or null | no | "System prompt for the agent." maxLength 100000 |
| `tools` | array of `BetaManagedAgentsAgentToolset20260401Params` / `BetaManagedAgentsMCPToolsetParams` / `BetaManagedAgentsCustomToolParams` | no | "Maximum of 128 tools across all toolsets allowed." |

Toolset variants:

- `type: "agent_toolset_20260401"` with `default_config` (`enabled`, `permission_policy`) and `configs[]` per-tool overrides. Tool names, verbatim: `bash`, `read`, `write`, `edit`, `glob`, `grep`, `web_fetch`, `web_search`. `web_fetch` additionally takes `allowed_domains` / `blocked_domains` (at most 64 entries, mutually exclusive, empty list rejected) and `max_content_tokens`. `web_search` takes the same domain lists plus `user_location` (`type: "approximate"`, `city`, `country`, `region`, `timezone`).
- `type: "mcp_toolset"` with `mcp_server_name` (must match an `mcp_servers` entry), `configs[]`, `default_config`.
- `type: "custom"` with `description`, `input_schema` (`type: "object"`, `properties`, `required`), `name` (1-128, "letters, digits, underscores, and hyphens"). "A custom tool that is executed by the API client rather than the agent. When the agent calls this tool, an `agent.custom_tool_use` event is emitted and the session goes idle, waiting for the client to provide the result via a `user.custom_tool_result` event."

### Agent resource (response)

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | `"agent"` | Discriminator |
| `id` | string | e.g. `"agent_011CZkYpogX7uDKUyvBTophP"` |
| `version` | number (int32) | "The agent's current version. Starts at 1 and increments when the agent is modified." |
| `archived_at` | string (RFC 3339) or null | Archive timestamp |
| `created_at` / `updated_at` | string (RFC 3339) | |
| `name`, `description`, `system`, `model`, `tools`, `mcp_servers`, `skills`, `multiagent`, `metadata` | | Resolved configuration. In `multiagent.agents`, roster entries come back "resolved to a specific version" with a concrete `version: number` |

### Lifecycle

From the agent lifecycle table (https://platform.claude.com/docs/en/managed-agents/agent-setup):

| Operation | Behavior (verbatim) |
| --- | --- |
| **Update** | "Generates a new agent version when the configuration changes." |
| **List versions** | "Returns the full version history so you can track changes over time." |
| **Archive** | "Makes the agent read-only. New sessions cannot reference it, but existing sessions continue to run." |

Archive is `POST /v1/agents/{agent_id}/archive`; "Archiving makes the agent read-only and cannot be undone... The response sets `archived_at` to the archive timestamp." There is **no documented agent delete endpoint** in the pages fetched.

Update semantics, verbatim highlights:

- `version` on update is optimistic concurrency: "When supplied, the request returns a 409 if it doesn't match the agent's current version, even when the fields you send already match the stored values; re-read the agent and retry. When omitted, the update applies unconditionally and the most recent update silently replaces any concurrent one, with no error to either caller."
- "Omitted fields are preserved."
- Scalars (`model`, `system`, `name`, `description`) are replaced. `system` and `description` clear with `null`; `model` and `name` "are mandatory and cannot be cleared."
- Arrays (`tools`, `mcp_servers`, `skills`) are "fully replaced"; clear with `null` or `[]`.
- `multiagent` "is replaced as a whole, including its `agents` roster."
- `metadata` "is merged at the key level... To delete a specific key, set its value to `null`."
- "**No-op detection.** If the update produces no change relative to the current version, no new version is created and the existing version is returned."
- "**Coordinator rosters are not updated.** Coordinators that reference this agent in their `multiagent.agents` roster keep the version that was pinned when the coordinator was created or last updated, even if the reference omits `version`."

## Sessions

"A session is an agent instance within an environment. Each session references an agent and an environment (both created separately), and maintains conversation history across multiple interactions. Sessions follow a two-step lifecycle: first create the session, then send a user event to start work." (https://platform.claude.com/docs/en/managed-agents/sessions)

`POST /v1/sessions`, `GET /v1/sessions/{session_id}`, `POST /v1/sessions/{session_id}` (update), `POST /v1/sessions/{session_id}/archive`, `DELETE /v1/sessions/{session_id}`, `POST /v1/sessions/{session_id}/resources`, `GET /v1/sessions/{session_id}/threads`, `GET /v1/sessions/{session_id}/events`, `POST /v1/sessions/{session_id}/events`, `GET /v1/sessions/{session_id}/events/stream`, `GET /v1/sessions/{session_id}/threads/{thread_id}/stream`.

### Create body (POST /v1/sessions)

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `agent` | string, `BetaManagedAgentsAgentParams`, or `BetaManagedAgentsAgentWithOverridesParams` | yes | "Accepts the `agent` ID string, which pins the latest version for the session, or an `agent` object with both id and version specified." |
| `environment_id` | string | yes | "ID of the `environment` defining the container configuration for this session." 1-128 chars |
| `budget` | `BetaManagedAgentsBudgetLimit` | no | `type: "limit"`, `max_list_cost: {amount: string, currency: "USD"}`. "A hard spend ceiling." |
| `initial_events` | array | no | "Initial events to send to the `session` at creation, processed in order. Supports `user.message` and `user.define_outcome` events. Maximum 50 events." A non-empty list "starts the agent loop in the same call: the session is created directly in the `running` status" |
| `metadata` | map[string] | no | "Maximum 16 pairs, keys up to 64 chars, values up to 512 chars." |
| `resources` | array | no | "Resources (e.g. repositories, files) to mount into the session's container." |
| `title` | string or null | no | "Human-readable session title." maxLength 500 |
| `vault_ids` | array of string | no | "Vault IDs for stored credentials the agent can use during the session." |

`resources[]` variants, verbatim:

- `type: "github_repository"`: `url` (required, max 2048), `authorization_token` (optional, "Required for private repositories; optional for public ones", max 4096), `checkout` (`{"type":"branch","name"}` or `{"type":"commit","sha"}`, 7-64 chars), `mount_path` ("Defaults to `/workspace/<repo-name>`").
- `type: "file"`: `file_id` (Files API), `mount_path` ("Defaults to `/mnt/session/uploads/<file_id>`").
- `type: "memory_store"`: `memory_store_id` ("The memory store ID (memstore_...). Must belong to the caller's organization and workspace."), `access` (`"read_write"` | `"read_only"`, defaults `read_write`), `instructions` ("Per-attachment guidance... Rendered into the memory section of the system prompt. Max 4096 chars.").

The response resource form adds output-only `mount_path` ("e.g. /mnt/memory/user-preferences. Derived from the store's name. Output-only.") and snapshotted `name` / `description` ("Later edits to the store's name do not propagate to this resource.").

### Session resource (response)

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | `"session"` | |
| `id` | string | e.g. `"sesn_011CZkZAtmR3yMPDzynEDxu7"` |
| `agent` | `BetaManagedAgentsSessionAgent` | "Resolved `agent` definition for a `session`. **Snapshot of the `agent` at `session` creation time.**" |
| `archived_at` | string or null | |
| `budget` | `BetaManagedAgentsBudgetLimit` or null | |
| `created_at` / `updated_at` | string (RFC 3339) | |
| `environment_id` | string | |
| `metadata` | map[string] | |
| `outcome_evaluations` | array of `BetaManagedAgentsOutcomeEvaluationResource` | "One entry per `define_outcome` event sent to the session." Fields: `outcome_id` (`outc_` prefix), `description`, `iteration` (0-indexed), `result`, `explanation`, `completed_at`. `result` values: `pending`, `running`, `evaluating`, `satisfied`, `max_iterations_reached`, `failed`, `interrupted` (last four terminal) |
| `resources` | array of `BetaManagedAgentsSessionResource` | Each gets a server `id` (e.g. `"sesrsc_011CZkZBJq5dWxk9fVLNcPht"`), `mount_path`, `created_at`, `updated_at` |
| `stats` | `BetaManagedAgentsSessionStats` | `active_seconds` ("Cumulative time in seconds the session spent in `running` status. Excludes idle time."), `duration_seconds` |
| `status` | `"rescheduling"` \| `"running"` \| `"idle"` \| `"terminated"` | `SessionStatus` enum |
| `title` | string or null | |
| `usage` | `BetaManagedAgentsSessionUsage` | `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation.{ephemeral_1h_input_tokens, ephemeral_5m_input_tokens}`, `list_cost` (`BetaMonetaryAmount` or null), `server_tool_use.{web_fetch_requests, web_search_requests}`, `active_seconds` ("Overlapping activity from concurrent threads is counted once, unlike `stats.active_seconds`... This is the duration the session's runtime cost is priced on.") |
| `vault_ids` | array of string | "Vault IDs **attached to the session at creation**. Empty when no vaults were supplied." |
| `deployment_id` | string or null | "Deployment ID when the session was created from a deployment reference. Null otherwise." |

### What is fixed for the session's life

Verbatim from https://platform.claude.com/docs/en/managed-agents/session-operations:

- "Only the agent's `tools` and `mcp_servers` can change after a session is created." Updates are "session-local and do not propagate back to the underlying agent." Semantics are "full replacement: the provided array is the new value. To preserve existing entries, `GET` the session, modify the array, and `POST` it back."
- "The session must be `idle` to update the agent. To update the agent while the session is running, send a `user.interrupt` event by itself and wait for the session to become `idle`."
- "To run a session with `model`, `system`, or `skills` values other than the agent's, use agent configuration overrides when you create the session." "The agent's configured `system` field is fixed for the session's lifetime." `inference_geo` also "can't change mid-session."
- Budget: "can only be attached at creation: you can change or remove it later, but you can't add one to a session created without it." Removal "is one-way."
- Memory stores: "can only be attached at session creation time; adding or removing one from a running session is not supported." (https://platform.claude.com/docs/en/managed-agents/memory)
- `vault_ids`: "attached to the session at creation." (No documented endpoint to add or remove vaults from a live session.)
- `POST /v1/sessions/{session_id}/resources` accepts **only** `type: "file"` with `file_id` and optional `mount_path`. So files can be added mid-session; repositories and memory stores cannot.

### Per-session overrides

`agent` accepts three forms: ID string, pinned `{"type":"agent","id","version"}`, or `{"type":"agent_with_overrides","id","version",...}`. Overridable fields: `model`, `system`, `tools`, `mcp_servers`, `skills`. Rules, verbatim:

- Omit -> inherit from the agent version.
- `null` / `[]` -> cleared, with three exceptions: `model: null` returns a 400 `agent_model_required`; clearing `tools` returns 400 "when the session's effective `skills` is non-empty, because skills require the `read` tool"; clearing `mcp_servers` returns 400 "when the session's effective `tools` still contains an `mcp_toolset` that references one of the agent's servers."
- Set -> "The value replaces the agent's value in full. Overrides never merge with the agent's configuration."
- "An `effort` level inside a per-session `model` override isn't applied... a session created with a `model` override runs at the model's default effort level."
- "Overrides apply only to the session you create. They do not modify the agent resource or create a new agent version."
- "In the response, the `agent` object reflects the configuration the session runs with after the overrides are applied. Its `id` and `version` still identify the agent and version the overrides are applied to. This lets you trace a session back to its base agent."

### Statuses

| Status | Description (verbatim) |
| --- | --- |
| `idle` | "Agent is waiting for input, including user messages or tool confirmations. Sessions created without `initial_events` start in `idle`." |
| `running` | "Agent is actively executing." |
| `rescheduling` | "Transient error occurred, retrying automatically." |
| `terminated` | "Session has ended, either because of an unrecoverable error or because it was archived. A session that finishes its work goes `idle`, not `terminated`." |

Session thread status uses the same four values (`SessionThreadStatus`: `"running"`, `"idle"`, `"rescheduling"`, `"terminated"`). "The session `status` is an aggregation of all agent activity; if at least one thread is `running`, then the overall session status is `running` as well."

`stop_reason` on `session.status_idle` / `session.thread_status_idle` is a tagged object, one of:

| `stop_reason.type` | Meaning (verbatim) |
| --- | --- |
| `"end_turn"` | "The agent completed its turn naturally and is ready for the next user message." |
| `"requires_action"` | "The agent is idle waiting on one or more blocking user-input events (tool confirmation, custom tool result, etc.)." Carries `event_ids: array of string`; "Resolving fewer than all re-emits `session.status_idle` with the remainder." |
| `"retries_exhausted"` | "The turn ended because repeated errors exhausted the retry budget or an error escalated to `retry_status: 'exhausted'`." |
| `"budget_reached"` | "The agent stopped because the session's tracked list cost reached its budget, or because its usage includes a model with no list price." |

### Archive vs delete

- **Archive** (`POST /v1/sessions/{id}/archive`): "Archive a session to prevent new events from being sent while preserving its history. A `running` session cannot be archived."
- **Delete** (`DELETE /v1/sessions/{id}`): "Delete a session to permanently remove its record, events, and associated sandbox. A `running` session cannot be deleted... Memory stores, vaults, skills, environments, and agents are independent resources and are not affected by session deletion. Files you uploaded through the Files API are also unaffected, but files the session itself produced are scoped to it and are permanently deleted along with its filesystem."

Both require an interrupt-then-wait dance if the session is running.

### Threads

"All agents share the same sandbox, filesystem, and vault credentials, but each agent runs in its own **session thread**, a context-isolated event stream with its own conversation history... Tools, MCP servers, and context are not shared." "Threads are persistent: the coordinator can send a follow-up to an agent it called earlier, and that agent retains everything from its previous turns." (https://platform.claude.com/docs/en/managed-agents/multiagent-orchestration)

`GET /v1/sessions/{session_id}/threads` returns `data` "ordered... primary first then children in spawn order", each with:

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | `"session_thread"` | |
| `id` | string | e.g. `sth_01DEF...` |
| `agent` | `BetaManagedAgentsSessionThreadAgent` or `BetaManagedAgentsAdvisor` | "Snapshot of the agent at **thread** creation time. The multiagent roster is not repeated here; read it from `Session.agent`." |
| `parent_thread_id` | string or null | "Parent thread that spawned this thread. Null for the primary thread." |
| `session_id` | string | |
| `status` | `SessionThreadStatus` | |
| `stats`, `usage` | objects or null | |
| `archived_at`, `created_at`, `updated_at` | | |

"A maximum of 25 concurrent threads is supported... Advisor consultation threads are exempt from this limit."

### Event log

Persisted event types follow "a `{domain}.{action}` naming convention." The full catalog (https://platform.claude.com/docs/en/managed-agents/reference):

**User events (you send):** `user.message`, `user.interrupt`, `user.custom_tool_result`, `user.tool_confirmation`, `user.define_outcome`, `user.tool_result` ("For sessions with `self_hosted` environments only").

**System events (you send):** `system.message` ("Append privileged system-level context that applies to the accompanying turn and all subsequent turns." Supported on "Claude Fable 5.1, Claude Mythos 5.1, Claude Fable 5, Claude Mythos 5, Claude Opus 5, and Claude Opus 4.8. On an unsupported primary model the event is rejected with `model_does_not_support_mid_conversation_system`.")

**Agent events:** `agent.message`, `agent.thinking` ("a progress signal only and does not carry the thinking content"), `agent.tool_use`, `agent.tool_result`, `agent.mcp_tool_use`, `agent.mcp_tool_result`, `agent.custom_tool_use`, `agent.thread_context_compacted`, `agent.thread_message_received`, `agent.thread_message_sent`.

**Session events:** `session.status_running`, `session.status_idle`, `session.status_rescheduled`, `session.status_terminated`, `session.deleted`, `session.updated`, `session.error`, `session.usage`, `session.thread_created`, `session.thread_status_running`, `session.thread_status_idle`, `session.thread_status_rescheduled`, `session.thread_status_terminated`.

**Span events:** `span.model_request_start`, `span.model_request_end`, `span.outcome_evaluation_start`, `span.outcome_evaluation_ongoing`, `span.outcome_evaluation_end`.

**Stream-only deltas (never persisted):** `event_start`, `event_delta`.

Ordering and cursoring, from `GET /v1/sessions/{session_id}/events`:

| Query param | Meaning (verbatim) |
| --- | --- |
| `created_at[gt]` / `[gte]` / `[lt]` / `[lte]` | "Compared against the event's `processed_at` value." |
| `limit` | int32 |
| `order` | `"asc"` or `"desc"`, "ordered by the event's `processed_at`. Defaults to `asc` (chronological)." |
| `page` | "Opaque pagination cursor from a previous response's `next_page`." |
| `types` | "Filter by event type. Values match the `type` field on returned events" |

"Every persisted event includes a `processed_at` timestamp set when the event finishes processing. On events you send, `processed_at` is null while the event is still queued behind earlier events. The exceptions are `user.define_outcome`, `user.custom_tool_result`, and `user.tool_result`, which are processed on receipt and echoed back with `processed_at` already populated."

Streaming is SSE over `GET /v1/sessions/{session_id}/events/stream` (thread-level: `GET /v1/sessions/{session_id}/threads/{thread_id}/stream`, "not `/events/stream`... and there is no `/threads/{thread_id}/events/stream` endpoint"). Critically: **"Only events emitted after the stream is opened are delivered, so open the stream before sending events to avoid a race condition."** The documented reconnect procedure is not a cursor replay but a dedupe:

1. "Open a new stream."
2. "List the full event history to seed a set of seen event IDs."
3. "Tail the live stream, skipping any events already returned by the history list."

Event deltas are opt-in per connection via the `event_deltas[]` query parameter, values restricted to `agent.message` and `agent.thinking`, "any other value returns a 400 error, as does a request with more than 100 values." They have "No replay on reconnect" and are "Never persisted." Their wire format deliberately differs from the Messages API streaming format: "A previewed `agent.message` gets a single `event_start` followed only by `event_delta` events. There are no per-content-block start or stop events... The delta type is `content_delta`, not `content_block_delta`. Accumulator code written for the Messages API does not carry over unchanged."

### Steering and interrupting

`POST /v1/sessions/{session_id}/events` with `events: array`. "The call returns as soon as the events are queued, and the interrupt's `processed_at` stays null until the agent applies it. A model response in progress stops immediately. The interrupt can take longer to apply while tool calls are running, and the session stays `running` until it does... Its `stop_reason` is `end_turn`, the same value as a turn that finishes on its own; **there is no stop reason specific to interruption**. The agent starts its next turn with the `user.message` you sent after the interrupt."

Session persistence: "Sessions persist between interactions. Conversation history is preserved unless the session is explicitly deleted. When a session goes idle, its sandbox is checkpointed, preserving the full sandbox state, including the filesystem, installed packages, and any files the agent created." But: "While session history is persisted until deleted, **sandbox state is only preserved for 30 days after the sandbox is created. Activity does not extend this window**: after 30 days the sandbox state (files, installed tools, and so on) is unrecoverable, and a resumed session starts from a fresh sandbox."

## Vaults and secrets

"Vaults and credentials are authentication primitives that let you register credentials for third-party services once and reference them by ID at session creation. This means you don't need to run your own secret store, transmit tokens on every call, or lose track of which end user an agent acted on behalf of." "A vault is the collection of `credentials` associated with an end user." (https://platform.claude.com/docs/en/managed-agents/vaults)

The design intent, verbatim: "The vault reference is a per-session parameter, so you can manage your product at the `agent` resource granularity and your users at the `session` resource granularity."

### Scope

**Workspace-scoped**, flagged as a warning in the docs: "Vaults and credentials are workspace-scoped, meaning **any API key with workspace access can reference them when creating a session**. To revoke access, delete the vault or credential." Vaults are not per-agent and not per-session; the binding to a session is by reference at creation.

### Vault resource

`POST /v1/vaults`. Body: `display_name` (string, required, 1-255), `metadata` (optional map, max 16 pairs).

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | `"vault"` | |
| `id` | string | `vlt_011CZkZDLs7fYzm1hXNPeRjv` |
| `display_name` | string | |
| `metadata` | map[string] | |
| `archived_at` | string or null | |
| `created_at` / `updated_at` | string (RFC 3339) | |

### Credential types (exact enum values)

`POST /v1/vaults/{vault_id}/credentials`, discriminated by `auth.type`. Three values: `"mcp_oauth"`, `"static_bearer"`, `"environment_variable"`.

"Two credential categories are supported: **MCP credentials** (`mcp_oauth`, `static_bearer`): each credential is keyed by an `mcp_server_url`. When the agent connects to a server at that URL at session runtime, the token is injected automatically. **Environment variables** (`environment_variable`): each credential is keyed by a `secret_name` (the environment variable name) and stored in the sandbox as an opaque placeholder. When the agent initiates an outbound request, the opaque placeholder is substituted with the real secret at egress. **The agent never sees the secret value.**"

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| **`type: "mcp_oauth"`** | | | |
| `access_token` | string | yes | "OAuth access token." 1-8192 |
| `mcp_server_url` | string | yes | "URL of the MCP server this credential authenticates against." 1-2047 |
| `expires_at` | RFC 3339 or null | no | |
| `refresh` | object or null | no | "OAuth refresh token parameters" |
| `refresh.client_id` | string | yes | 1-1024 |
| `refresh.refresh_token` | string | yes | 1-8192 |
| `refresh.token_endpoint` | string | yes | 1-2047 |
| `refresh.token_endpoint_auth` | union | yes | `{"type":"none"}` ("Token endpoint requires no client authentication" / docs prose: "public client"), `{"type":"client_secret_basic","client_secret"}`, `{"type":"client_secret_post","client_secret"}`; `client_secret` 1-512 |
| `refresh.resource` | string or null | no | "OAuth resource indicator." |
| `refresh.scope` | string or null | no | |
| **`type: "static_bearer"`** | | | |
| `token` | string | yes | "Static bearer token value." 1-8192 |
| `mcp_server_url` | string | yes | 1-2047 |
| **`type: "environment_variable"`** | | | |
| `secret_name` | string | yes | "Name of the environment variable. **Immutable after create.**" 1-255 |
| `secret_value` | string | yes | "Secret value. **Write-only; never returned in responses.**" 1-4096 |
| `networking` | union | yes | `{"type":"unrestricted"}` ("Substitute the secret on any host the session's Environment network policy permits egress to. The Environment's network policy is the only boundary on where the secret can reach.") or `{"type":"limited","allowed_hosts":[...]}` ("Each entry is a bare hostname (`api.example.com`), an IPv4 address (`192.0.2.1`), or a `*.`-prefixed wildcard (`*.example.com`). URLs, ports, paths, and IPv6 addresses are not accepted. At most 16 entries.") |
| `injection_location` | object | no | `{header: boolean, body: boolean}`. "`allowed_hosts` scopes which hosts the secret is substituted for, and `injection_location` scopes which parts of the request it is substituted into." "A credential must have at least one location enabled, so a create or update that would disable both locations returns a 400 error." |
| **Common** | | | |
| `display_name` | string or null | no | max 255 |
| `metadata` | map[string] | no | max 16 pairs |

Credential response object: `type: "vault_credential"`, `id` (`vcrd_...`), `vault_id`, `auth` (with sensitive fields stripped: the OAuth response keeps `mcp_server_url`, `expires_at`, and the `refresh` shape minus tokens and secrets), `display_name`, `metadata`, `archived_at`, `created_at`, `updated_at`.

**Write-only is explicit:** "The actual credential values you supply (`token`, `access_token`, `refresh_token`, `client_secret`, `secret_value`) are treated as sensitive, write-only fields and never returned in API responses." There is no read-back path documented, for any caller.

### How a secret reaches a running tool

Two distinct paths, and they differ in trust model:

1. **MCP credentials** (`mcp_oauth`, `static_bearer`) are keyed by `mcp_server_url` and injected into the MCP connection by the platform when the agent connects to that URL. The agent's model context never carries the token.
2. **Environment variable credentials** are placed in the sandbox as an *opaque placeholder* under `secret_name`. The real value is substituted "at egress" by the platform proxy, restricted by `networking` (which hosts) and `injection_location` (header and/or body). "The agent never sees the secret value." Not supported on self-hosted sandboxes: "Environment variable credentials (`environment_variable`) are not yet supported with self-hosted sandboxes."

### Rotation, revocation, lifecycle

"Secret values, `display_name`, and (on environment variable credentials) `injection_location` can be updated... **Structural fields (`mcp_server_url`, `secret_name`, `token_endpoint`, `client_id`) are locked after creation.** To change them, archive the credential and create a new one."

"Credentials are re-resolved periodically, both during a session and during the vault lifecycle. This ensures that credential rotation, archival, or deletion propagates to running sessions **without a restart**. For `mcp_oauth` credentials, re-resolution also refreshes the access token if it has expired."

Lifecycle webhooks (verbatim table): `vault.archived`, `vault.deleted`, `vault_credential.archived`, `vault_credential.deleted`, `vault_credential.refresh_failed` ("An `mcp_oauth` credential cannot be refreshed (invalid refresh token, or irrecoverable error from the OAuth server)").

Operations:

- "**Archive a vault:** `POST /v1/vaults/{id}/archive`. Cascades to all credentials. **Secrets are purged; records are retained for auditing.** Future sessions referencing this vault fail; running sessions continue."
- "**Archive a credential:** `POST /v1/vaults/{id}/credentials/{cred_id}/archive`. Purges the secret payload; the credential key (`mcp_server_url` or `secret_name`) remains visible and is freed for a replacement credential."
- "**Delete a vault or credential:** Hard delete. The record is not retained. Use archive if you need an audit trail."
- List: "Paginated, newest first. Archived records are excluded by default (pass `include_archived=true` to include them)."

Note the asymmetry with agent archive: archiving a **vault** makes *future* sessions fail, but running sessions continue; archiving an **agent** makes new sessions unable to reference it, and existing sessions continue.

### `mcp_oauth_validate`

`POST /v1/vaults/{vault_id}/credentials/{credential_id}/mcp_oauth_validate`. "Result of live-probing a credential against its configured MCP server." It is a diagnostic, not a repair.

| Field | Type | Meaning |
| --- | --- | --- |
| `type` | `"vault_credential_validation"` | |
| `credential_id`, `vault_id` | string | |
| `has_refresh_token` | boolean | |
| `status` | `"valid"` \| `"invalid"` \| `"unknown"` | "`valid`: the token works; no action needed. `invalid`: the grant is gone or the OAuth server rejected the refresh with a 4xx. Prompt the end user to re-authorize. `unknown`: a transient error (5xx, 429, or network failure). Wait and retry." |
| `refresh` | object or null | `status`: `"succeeded"` \| `"failed"` \| `"connect_error"` \| `"no_refresh_token"`, plus `http_response` |
| `mcp_probe` | object or null | "The failing step of an MCP validation probe": `method` ("The MCP method that failed (for example `initialize` or `tools/list`)") and `http_response` |
| `http_response` | object | `status_code` (int32), `body` ("May be truncated and has sensitive values scrubbed"), `body_truncated`, `content_type` |
| `validated_at` | RFC 3339 | |

### Egress and allowlist controls

Three layers, all separate from the credential itself:

1. **Environment network policy** (`environment.config.networking`): `{"type":"unrestricted"}` or `{"type":"limited", allowed_hosts, allow_mcp_servers, allow_package_managers}`. `allow_mcp_servers` "Permits outbound access to MCP server endpoints configured on the agent, beyond those listed in the `allowed_hosts` array. Defaults to `false`." `allow_package_managers` "Defaults to `false` on creation. Must be `true` when `packages` are specified."
2. **Credential networking** (`environment_variable` only): `allowed_hosts`, at most 16 entries.
3. **Tool-level domain filters** on `web_fetch` / `web_search`: `allowed_domains` or `blocked_domains`, at most 64 entries, mutually exclusive.

## Identity and versioning

**Tagged IDs.** Every resource carries a typed prefix: `agent_`, `sesn_`, `sth_` (session thread), `sevt_` (session event), `sesrsc_` (session resource), `env_`, `vlt_`, `vcrd_`, `memstore_`, `outc_` (outcome), `skill_`, `file_`. Deployments are addressed as `dep_...` per the docs prose. Every ID is opaque and server-generated; nothing is content-addressed.

**Agent versioning is implicit, monotonic, and integer-valued.** `version` "Starts at 1 and increments when the agent is modified." There is no explicit "publish a version" call: any update that changes configuration mints a version, and any update that changes nothing does not ("no-op detection"). A version is addressed as `{"type":"agent","id":"agent_...","version":N}`, or by ID string alone, which "pins the latest version." There is **no content digest, hash, or content-addressed identity anywhere in the documented surface**, and no way to ask "which version has this exact config?" other than listing versions and diffing yourself.

**Pinning happens at three separate moments, each a snapshot:**

| Pin point | What it captures |
| --- | --- |
| Session creation | The session's `agent` field is a full snapshot of the resolved configuration, with overrides applied. `id` and `version` still name the base agent "so you can trace a session back to its base agent." |
| Thread creation (multiagent) | The thread's `agent` is a "Snapshot of the agent at thread creation time" |
| Coordinator create/update | Roster entries are resolved to concrete versions and frozen: "Coordinators... keep the version that was pinned when the coordinator was created or last updated, even if the reference omits `version`." |

**Skills version separately** and are pinned by string `version`, defaulting to `latest` when omitted, for both `anthropic` and `custom` skills.

**Environments are deliberately not versioned:** "Environments are not versioned. If you update an environment frequently, keep your own record of the changes so you can tell which configuration each session used." This is the one obvious hole in the pinning story and the docs say so out loud.

**Optimistic concurrency is opt-in per call.** `version` in the update body is an If-Match, returning 409 on mismatch. Omitting it is last-write-wins with no error to either writer. The docs recommend supplying it "for interactive callers" and omitting it for "declarative apply loops, such as a CI job that syncs checked-in agent definitions, where the loop owns the agent."

**Deployments** bind an agent version to an environment, credentials, initial events, and an optional cron schedule: "a configured instance of an agent, it binds the agent to everything needed to run it autonomously: an environment, credentials, initial events, and an optional schedule." `initial_events` is *required* on a deployment (1-50), unlike on a session. `schedule` is `{"type":"cron","expression","timezone"}` with a "5-field POSIX cron expression"; extended syntax (seconds, year, `L`, `W`, `#`, `?`, `@daily`) is explicitly not supported. Sessions created from a deployment carry `deployment_id`.

## Notable design decisions

1. **The session snapshots the agent rather than referencing it live.** Editing an agent never perturbs a running session, and the session response carries the full resolved config plus `{id, version}` back-pointers. The cost is that a session's config can drift arbitrarily far from the agent's current version, and there is no digest to prove two sessions ran identical config.

2. **Per-session overrides are an escape hatch that does not version.** "Use it to try a different model or grant an extra tool in one session without versioning the agent." This is deliberate: the agent resource stays the product-level abstraction and the session stays the per-user/per-task abstraction. The tradeoff is that override-created configuration is never named, never listed, and never reusable; it exists only inside one session's snapshot.

3. **Overrides replace, never merge.** "Overrides never merge with the agent's configuration, so a `tools` override must list every tool the session should have." Same rule for agent updates on array fields. This avoids the deep-merge ambiguity that plagues config systems, at the cost of verbose callers. The one carve-out is `metadata`, which merges per-key, and `effort`, which is preserved if the model `id` is unchanged. Those two exceptions are exactly the places where the docs have to spend the most words explaining themselves.

4. **Very few things are mutable mid-session, and the mutable set is chosen for safety response.** Only `tools` and `mcp_servers` can change. The reasoning is implicit but visible: those are the levers you need to *tighten* a running agent (revoke a tool, add a domain filter, flip a policy to `always_ask`). Everything identity-shaped, `model`, `system`, `skills`, `inference_geo`, budget-add, memory attach, vault attach, is frozen.

5. **The event log is durable and listable, but the live stream is not replayable.** "Only events emitted after the stream is opened are delivered." There is no `Last-Event-ID` or cursor-resume on the SSE endpoint. The prescribed reconnect is open-stream-then-list-history-then-dedupe-by-`id`. This pushes the dedupe burden onto every client, but it means the platform never has to keep per-connection replay buffers.

6. **Cursoring is by `processed_at` with an opaque `next_page`, not by event sequence number.** Filters are time-range (`created_at[gt]` etc., "Compared against the event's `processed_at` value"). A queued event you sent has `processed_at: null` until applied, so your own event is invisible to a time-range query until the server processes it. Ordering is total on `processed_at` but the API exposes no monotonic offset.

7. **Interruption is not a first-class terminal state.** A `user.interrupt` ends the turn with `stop_reason: end_turn`, "the same value as a turn that finishes on its own; there is no stop reason specific to interruption." That is a real information loss: a client reading only `stop_reason` cannot distinguish "the agent finished" from "I stopped it." Worth arguing about.

8. **Budgets are one-way and creation-only.** You can lower, raise, or remove a budget, but never add one after the fact, and never re-add after removal. Enforcement is between model requests, so "a session capped at `"50"` (50 cents) can pause with a `list_cost` of `"53"`. This is expected, not a billing error." Amounts are whole cents as a *string*, "the API takes a string rather than a number so no floating-point rounding is ever applied." List cost is priced at public list rates regardless of your contract, so "your billed spend might be lower than the cap."

9. **Secrets are write-only by construction, and the `environment_variable` type puts the substitution boundary at network egress, not in the sandbox.** The agent holds a placeholder; the platform swaps it at the proxy, scoped by host (`allowed_hosts`, max 16) and by request part (`injection_location.header` / `.body`). This is the single most interesting decision in the whole surface: it means a compromised agent process cannot exfiltrate the secret, only *use* it against hosts you allowlisted. `injection_location` is independent of `allowed_hosts` precisely so "where it can go" and "where in the request it lands" are separate axes.

10. **Vaults are workspace-scoped, and the docs flag that as a hazard rather than a feature.** "any API key with workspace access can reference them when creating a session. To revoke access, delete the vault or credential." There is no per-agent or per-session ACL on a vault. The only revocation primitive is destroying the credential.

11. **Archive purges secrets but keeps records; delete keeps nothing.** "Secrets are purged; records are retained for auditing" versus "Hard delete. The record is not retained. Use archive if you need an audit trail." The same archive/delete split recurs on sessions, agents, memory stores, and environments, and it consistently means "keep the audit trail, drop the payload" versus "keep nothing."

12. **Structural credential fields are immutable; rotation replaces only the payload.** `mcp_server_url`, `secret_name`, `token_endpoint`, `client_id` are "locked after creation." That makes a credential's *identity* stable across rotations, which is what lets "credentials are re-resolved periodically... without a restart" work safely.

13. **`auto` permission policy reads intent from `user.message` only, and the docs are explicit that tool output is not intent.** Verbatim: "The server does not read intent from a tool result, a fetched webpage, an MCP server's response, or a message between session threads. It assesses that content but does not take instructions from it." And the corresponding warning: "If you relay untrusted end-user input in `user.message` events, the server reads that input as your intent too, and it can get a call allowed. Configure `always_ask` on the tools you would not let that end user run without review." In multiagent: "Nothing in a subagent's thread counts as your intent: your client posts no messages there, and the coordinator's messages to the subagent do not count."

14. **Denials under `auto` are not overridable by the client.** "The agent receives an error tool result with the content `Permission to use {tool_name} has been denied.` and `is_error: true`. The session keeps running, and your client cannot override the denial." Contrast with `always_ask`, where you decide. `auto` deliberately reserves a class of calls the platform will refuse "no matter who asks."

15. **Every tool-use event carries its own audit record.** `evaluated_permission` (`"allow"` / `"ask"` / `"deny"`) plus an `evaluation` object naming the policy that produced it, with `reason_code` (`"high_risk"`, `"indeterminate"`) under `auto`. The docs even specify forward-compatibility: "Write your client to tolerate an `evaluation.type` or `reason_code` it does not recognize," and specify how to read events predating the field.

16. **Memory stores are a filesystem, not an API the agent calls.** They mount under `/mnt/memory/<slug>`, the agent uses ordinary `read`/`write`/`edit`, "writes to any other path under `/mnt/memory/` fail, because the sandbox mounts that parent directory read-only," and `access` is "enforced at the filesystem level." Every write produces an immutable memory version. The security guidance is blunt: "If the agent processes untrusted input... a successful prompt injection could write malicious content into the store. Later sessions then read that content as trusted memory. Use `read_only` for reference material."

17. **Multiagent is capability-limited to depth 1.** Roster agents "must not themselves have `multiagent` set (depth limit 1)," roster entries must be distinct, at most one `self`, at most one `advisor`, 1-20 entries, 25 concurrent threads. Blocking events from subagents are cross-posted to the primary thread with `session_thread_id` so the client only ever has to answer on one stream, and "the server routes the response to the correct thread automatically."

18. **The declared-versus-supplied split.** Declared **on the agent**: model (+effort, speed, inference_geo), system, tools, mcp_servers, skills, multiagent roster, metadata. Supplied **per session**: environment_id, vault_ids, resources (repos, files, memory stores), budget, title, metadata, initial_events, plus overrides of the agent's five overridable fields. The line is: the agent is *behavior*, the session is *context, credentials, placement, and money*. MCP **server URLs** live on the agent, MCP **credentials** live on the vault, which is the cleanest expression of that split in the whole design.

## Gaps and open questions

- **No content digest or content-addressed identity for agents.** Versions are sequence numbers only. Nothing in the fetched docs lets you assert "this session ran exactly this configuration" other than by comparing snapshots field-by-field.
- **No agent delete.** Only archive, and "cannot be undone." Whether agents are ever hard-deletable is not documented.
- **No documented way to revert an agent to a prior version.** `GET /versions` reads history; nothing documented writes an old version back other than re-POSTing the fields yourself.
- **Environments are unversioned by design**, and the docs tell you to keep your own change log. There is no `environment_version` on a session, so a session's placement configuration is unreconstructible after the fact.
- **No cursor-resume on the SSE stream.** No `Last-Event-ID`, no `after_event_id` parameter on the stream endpoint in any page fetched. Event deltas explicitly have "no way to re-request missed deltas."
- **No monotonic event sequence number.** Only `id` (opaque) and `processed_at` (a timestamp). Whether `processed_at` is unique per event, or how ties are broken, is not documented.
- **`user.interrupt` is indistinguishable from `end_turn` in `stop_reason`.** Documented as intentional, but there is no documented alternative signal.
- **Session update endpoint fields are not fully enumerated in the pages fetched.** The concept doc names `agent.tools`, `agent.mcp_servers`, and `budget` as updatable; I did not fetch `/docs/en/api/beta/sessions/update.md`, so the complete update body is **not verified**.
- **Session create response versus retrieve response.** I read the full field list from `sessions/retrieve`. The create response is documented as the same `BetaManagedAgentsSession` object, but I did not diff them field by field.
- **Vault ACLs.** There is no documented per-agent, per-session, or per-key scoping on a vault beyond the workspace boundary. Whether a session can be prevented from reading a vault it was handed is not documented.
- **What happens to a running session when its vault is deleted (not archived) is not documented.** Archive says "running sessions continue"; delete says only "Hard delete. The record is not retained." Re-resolution presumably fails, but the failure mode, error code, and event are **not documented**.
- **Credential re-resolution interval is not documented** ("re-resolved periodically"). Only the self-hosted memory-store sync interval is given (15 seconds by default).
- **No documented limit on `vault_ids` per session.** Deployments cap it at 50 ("Maximum 50"); the session create doc gives no maximum.
- **Conflict behavior when two vaults hold credentials for the same `mcp_server_url` is not documented.**
- **Whether `metadata` on any resource is visible to the model is only documented for memory stores** ("Not visible to the agent"). Agent, session, vault, and credential metadata visibility is **not documented**.
- **Egress proxy details for `environment_variable` substitution are not documented**: TLS interception, what happens on a host outside `allowed_hosts` (silent pass-through of the placeholder, or a blocked request), and whether the placeholder is stable or per-session. The Console note implies pass-through ("the placeholder passes through literally and the service rejects it with its own authentication error") but that is stated only for the `injection_location` mismatch case, not for a host mismatch.
- **Rate limits are per organization only**: "Create endpoints... 300 requests per minute", "Read endpoints... 1,200 requests per minute". No per-session or per-agent limits are documented.
- **`skills` version strings**: the API schema says `version: optional string or null` "Defaults to latest if omitted", while the concept doc says "Pin to a specific version or use `latest`." Whether the literal string `"latest"` is an accepted value, versus omitting the field, is **not clearly documented**; the example response shows numeric-looking strings `"1"` and `"2"`.
- **`archive` on a session versus `session.status_terminated`**: the status table says a session terminates "because it was archived", and the event table says `session.status_terminated` fires when "Session ended, either because of an unrecoverable error or because it was archived." Whether an archived session's status reads `terminated` on retrieve, in addition to `archived_at` being set, is implied but not stated.
- **Compliance**: "Managed Agents is not currently eligible for Zero Data Retention (ZDR) or HIPAA Business Associate Agreement (BAA) coverage."

## Sources

Concept pages:
- https://platform.claude.com/docs/en/managed-agents/overview
- https://platform.claude.com/docs/en/managed-agents/sessions
- https://platform.claude.com/docs/en/managed-agents/session-operations
- https://platform.claude.com/docs/en/managed-agents/agent-setup
- https://platform.claude.com/docs/en/managed-agents/vaults
- https://platform.claude.com/docs/en/managed-agents/environments
- https://platform.claude.com/docs/en/managed-agents/memory
- https://platform.claude.com/docs/en/managed-agents/permission-policies
- https://platform.claude.com/docs/en/managed-agents/skills
- https://platform.claude.com/docs/en/managed-agents/tools
- https://platform.claude.com/docs/en/managed-agents/multiagent-orchestration
- https://platform.claude.com/docs/en/managed-agents/events-and-streaming
- https://platform.claude.com/docs/en/managed-agents/budgets
- https://platform.claude.com/docs/en/managed-agents/reference

API reference pages:
- https://platform.claude.com/docs/en/api/beta/agents/create
- https://platform.claude.com/docs/en/api/beta/agents/update
- https://platform.claude.com/docs/en/api/beta/agents/versions/list
- https://platform.claude.com/docs/en/api/beta/sessions/create
- https://platform.claude.com/docs/en/api/beta/sessions/retrieve
- https://platform.claude.com/docs/en/api/beta/sessions/resources/add
- https://platform.claude.com/docs/en/api/beta/sessions/threads/list
- https://platform.claude.com/docs/en/api/beta/sessions/events/list
- https://platform.claude.com/docs/en/api/beta/sessions/events/send
- https://platform.claude.com/docs/en/api/beta/vaults/create
- https://platform.claude.com/docs/en/api/beta/vaults/credentials/create
- https://platform.claude.com/docs/en/api/beta/vaults/credentials/update
- https://platform.claude.com/docs/en/api/beta/vaults/credentials/mcp_oauth_validate
- https://platform.claude.com/docs/en/api/beta/environments/create
- https://platform.claude.com/docs/en/api/beta/deployments/create
- https://platform.claude.com/docs/en/api/beta/memory_stores/create

All pages fetched as markdown by appending `.md` to the URL, on 2026-09-11.
