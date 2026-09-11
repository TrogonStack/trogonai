# OpenAI: Sessions, Agents, Vaults

Part of the [provider agent contracts research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-09-11. Every claim is sourced inline to a
vendor documentation page or machine-readable spec. These documentation sites
are unversioned and publish no commit identifiers, so the authoritative
anchors are the pinned entry points below plus the API version header each
surface requires:

- OpenAI Agents API,
  [guide](https://developers.openai.com/api/docs/guides/agents-api) and
  [API reference](https://developers.openai.com/api/reference), indexed from
  [`/api/llms.txt`](https://developers.openai.com/api/llms.txt).
- Version pin: the `OpenAI-Beta: agents=v1` header against base
  `https://api.openai.com/v1`.

The OpenAI Agents API is a beta server-side API (`OpenAI-Beta: agents=v1`, base `https://api.openai.com/v1`) that exposes what the docs call "the Codex harness through an OpenAI-managed API". OpenAI runs the model and tool loop; the caller supplies configuration, input, function-tool results, and optionally the compute. The docs name four core concepts: "**Agent:** The model, instructions, tools, and MCP servers available to the agent. **Environment:** An optional sandbox or computer where the agent accesses files, loads skills, and runs commands. **Session:** A durable instance of an agent that works on tasks and responds to input. **Events and items:** The inputs sent to an agent and the output produced during a session." Those four map onto five actual server-side resources: `agent`, `agent.session`, `agent.session.turn`, `agent.session.artifact`, and `vault` plus `vault.credential`. Secrets are a separate concern handled by three unrelated mechanisms: vaults (stored, write-only, bound to MCP server URLs), workload identity federation (no stored secret at all, token exchange against an external issuer), and Realtime client secrets (`ek_*` ephemeral tokens, a different product surface entirely). Source: <https://developers.openai.com/api/docs/guides/agents-api.md>, <https://developers.openai.com/api/docs/guides/agents-api/architecture.md>.

## Resource model

```
 project (scoping boundary for agents and vaults)
   |
   +-- agent                       POST /agents            object: "agent"
   |     model, instructions, tools, reasoning, text,
   |     service_tier, multi_agent, metadata, name
   |     NO version / revision / digest field
   |
   +-- vault                       POST /vaults            object: "vault"
   |     |
   |     +-- vault.credential      POST /vaults/{vault_id}/credentials
   |           auth.type: "static_bearer" | "mcp_oauth"
   |           bound to auth.mcp_server_url   (secrets write-only)
   |
   +-- environment template        POST /agents/environments/templates
   |     (openai_hosted only)
   |
   +-- agent.session               POST /agents/sessions   object: "agent.session"
         |
         |  binds at creation:
         |    agent_id  (reference)  and/or  agent {...} (inline override)
         |    environment  {none | openai_hosted | self_hosted}
         |    vault_ids[]
         |    metadata
         |  copies agent.name as a snapshot; "Later changes to the
         |  agent's name do not affect this value."
         |
         +-- agent.session.turn    GET /agents/sessions/{id}/turns
         |     status: queued|in_progress|waiting|completed|failed|cancelled
         |     error.code: 17-value enum
         |
         +-- items                 GET /agents/sessions/{id}/items
         |     message, reasoning, function_call, function_call_output,
         |     mcp_call, command_execution, web_search_call,
         |     create_subagent_call, send_subagent_input_call,
         |     interrupt_subagent_call, resume_subagent_call,
         |     close_subagent_call, wait_for_subagents_call
         |
         +-- subagents             GET /agents/sessions/{id}/subagents/.../items
         |
         +-- agent.session.artifact GET /agents/sessions/{id}/artifacts
         |     "An immutable file published by a completed hosted session turn."
         |
         +-- events (SSE)          GET  /agents/sessions/{id}/events   (read, no replay)
               input events        POST /agents/sessions/{id}/events   (write)

 webhooks (5 coarse events) ---> your handler, out of band from the SSE stream

 NOT connected to any of the above:
   /conversations  (Conversations resource, Responses API family)
   /containers     (Code Interpreter containers)
   /realtime/client_secrets  (ek_* ephemeral tokens)
   /oauth/token    (workload identity federation token exchange)
```

## Agents

### Is the agent a server-side resource?

Yes. `POST /agents` creates one and the returned object is described verbatim as "A reusable agent scoped to the caller's project" with `object: "agent"`. Full CRUD exists: `POST /agents`, `GET /agents`, `GET /agents/{agent_id}`, `POST /agents/{agent_id}` (update), `DELETE /agents/{agent_id}`. The create endpoint description is "Creates a reusable agent without storing credentials."

The agent resource is optional. A session can be created with an inline `agent` object and no `agent_id`; in that case `model` is required. The guide says: "An agent configuration defines how the agent behaves. You can supply it when creating a session or save it for reuse. The session holds the conversation and work, while the saved agent holds reusable settings."

This is distinct from the Agents SDK (a client-side library) and from Agent Builder. The Agents API docs never cross-reference either. The only mention of "Agent Builder" anywhere in the fetched corpus is in the RBAC permission table and in a separate `agent-builder-safety` doc listed in `docs_llms.txt` (not fetched). Treat the Agents API as a wholly server-side product surface.

### `POST /agents` body parameters (verbatim)

| Field | Type | Required | Meaning (verbatim) |
| --- | --- | --- | --- |
| `model` | `string` | yes | "The model to use for the agent. The requested model name is preserved." |
| `instructions` | `string` or `null` | optional | "Additional instructions appended to the agent's default base instructions. Omit or set to null to add no custom instructions." |
| `metadata` | `map[string]` or `null` | optional | "Up to 16 string key-value pairs, with keys up to 64 and values up to 512 characters. Omission or null defaults to an empty map." |
| `multi_agent` | `MultiAgentConfigParam { enabled, max_concurrent_subagents }` or `null` | optional | "Explicit configuration for creating and coordinating subagents." |
| `multi_agent.enabled` | `boolean` | yes (within object) | "Whether subagent tools are enabled." |
| `multi_agent.max_concurrent_subagents` | `number` (int64, min 1, max 4294967295) | optional | "Maximum number of subagents that may run concurrently. Defaults to 6." |
| `name` | `string` or `null`, maxLength 128 | optional | "A human-readable name for the agent. Omission or null leaves the agent unnamed." |
| `reasoning` | `AgentReasoningParam { effort, summary }` or `null` | optional | "Reasoning configuration for the agent." |
| `reasoning.effort` | `"none"`, `"minimal"`, `"low"`, `"medium"`, `"high"`, `"xhigh"`, `"max"` or `null` | optional | "The amount of reasoning effort the model should use." |
| `reasoning.summary` | `"concise"`, `"detailed"`, `"auto"` or `null` | optional | "The reasoning summary format requested from the model." |
| `service_tier` | `"auto"`, `"default"`, `"flex"`, `"priority"`, `"fast"` or `null` | optional | "The service tier used for model requests." |
| `text` | `AgentTextParam { format, verbosity }` or `null` | optional | "Configuration for text generated by the agent." |
| `text.format` | `TextFormatParam` = `Text { type: "text" }` or `JSONSchema { schema, type: "json_schema" }` or `null` | optional | "The output format for generated text." |
| `text.verbosity` | `"low"`, `"medium"`, `"high"` or `null` | optional | "The amount of text the model should produce." |
| `tools` | array of `PersistedAgentToolParam` or `null` | optional | "Tools available to the agent. Defaults to an empty list." |

`PersistedAgentToolParam` is a discriminated union on `type`:

| Variant | `type` | Fields (verbatim) |
| --- | --- | --- |
| Function | `"function"` | `description: string` (required), `name: string` (required), `parameters: map[unknown]` "A JSON Schema object describing the function's arguments." (required), `defer_loading: optional boolean` "Whether this function is deferred and discovered through tool search. Defaults to false." |
| ToolSearch | `"tool_search"` | none. "Discovers deferred function tools and loads them into the model context." |
| ProgrammaticToolCalling | `"programmatic_tool_calling"` | `enabled: optional boolean` "Whether tools can be called from model-generated code. Defaults to true." |
| Mcp | `"mcp"` | `server_label: string` (required), `transport: PersistedMcpTransportParam` (required), `allowed_tools: optional array of string or null`, `connection_origin: optional "service" or "environment" or null`, `credential_id: optional string or null`, `request_metadata: optional map[unknown] or null`, `required: optional boolean` "Whether this MCP server must initialize before the first turn. Defaults to false." |
| WebSearch | `"web_search"` | `allowed_domains: optional array of string or null`, `context_size: optional "low"/"medium"/"high" or null`, `location: optional object { city, country, region, timezone } or null`, `mode: optional "disabled"/"cached"/"live" or null` |

`PersistedMcpTransportParam` is described as "The credential-free transport used to connect to the MCP server" and is one of:

| Variant | `type` | Fields (verbatim) |
| --- | --- | --- |
| HTTP | `"http"` | `server_url: string` (required), `headers: optional map[string] or null` "Non-secret HTTP headers sent to the MCP server." |
| Stdio | `"stdio"` | `command: string` (required), `cwd: string` "The working directory used to start the MCP server." (required), `args: optional array of string or null`, `env_vars: optional array of string or null` "Environment variable names to inherit from the selected execution environment." |

`connection_origin` values: `"service"` = "Uses the Managed Agents service network"; `"environment"` = "Uses the session's execution environment". `credential_id` = "The vault credential selected for this MCP server. Optional when exactly one attached credential matches the server URL."

Note that the persisted agent transport carries no secret fields at all. Session-level MCP transports additionally accept `transport.authorization` and `transport.headers` (the MCP guide says "The Agents API encrypts these values and omits them from the returned session resource"), which is why the agent create endpoint is described as "without storing credentials".

### `Agent` return object (verbatim)

| Field | Type | Meaning (verbatim) |
| --- | --- | --- |
| `id` | `string` | "The ID of the reusable agent." |
| `created_at` | `number` (int64) | "The Unix timestamp, in seconds, when the agent was created." |
| `instructions` | `string` or `null` | "Custom instructions appended to the agent's default base instructions." |
| `metadata` | `map[string]` | "Custom string key-value pairs attached to the agent." |
| `model` | `string` | "The requested model name used for inference." |
| `multi_agent` | `MultiAgentConfig { enabled, max_concurrent_subagents }` | "The resolved configuration for creating and coordinating subagents." |
| `name` | `string` or `null` | "A human-readable name for the agent, or null if it is unnamed." |
| `object` | `"agent"` | "The object type. Always agent." |
| `reasoning` | `AgentReasoning { effort, summary }` | "The resolved reasoning configuration, including the model default for an omitted effort." |
| `service_tier` | `"auto"`/`"default"`/`"flex"`/`"priority"`/`"fast"` | "The resolved service-tier policy used for model requests." |
| `text` | `AgentText { format, verbosity }` | "The resolved configuration for text generated by the agent." |
| `tools` | array of `PersistedAgentTool` | "Tools available to the agent." |

There is no `updated_at` field documented on `Agent`, even though `POST /agents/{agent_id}` exists. There is no `status`, no `version`, no `revision`, no `digest`, and no `project_id` field.

### Agent lifecycle

Create with `POST /agents`. Update with `POST /agents/{agent_id}`; the update body fields say "Omit to leave unchanged" (the create body says "Omission or null defaults to an empty map", so create and update have different omission semantics). List with `GET /agents` using `after` / `limit` / `order` (`"asc"` or `"desc"`, "Defaults to desc"). Delete with `DELETE /agents/{agent_id}`.

Reuse: "Save an agent to reuse its configuration across sessions. Create it once, then pass its ID as `agent_id` when starting each session." Per-session override: "Include both `agent_id` and `agent` to customize a session that uses a saved agent. The session inherits omitted settings, including the model. ... Overrides apply only to that session. They do not change the saved agent or other sessions. **Supplied objects and arrays replace the entire field rather than merging with the saved value.** For example, supplying `tools` replaces the saved tool list."

What happens to a running session when its saved agent is mutated or deleted is **not documented**.

`timeout` is not an agent field. It appears in SDK examples as a client HTTP request timeout only, and the `POST /agents` body parameter list has no such field.

## Sessions

### What a session is and what it binds

"**Session:** A durable instance of an agent that works on tasks and responds to input." `POST /agents/sessions` is described as "Creates a managed agent session, optionally submits initial input, and returns the session or streams its events when stream is true."

### `POST /agents/sessions` body parameters (verbatim)

| Field | Type | Required | Meaning (verbatim) |
| --- | --- | --- | --- |
| `environment` | `EnvironmentParam` | yes | "An inline execution environment or a reference to an environment template." |
| `agent` | `object { instructions, model, multi_agent, ... }` | optional | "Agent configuration. With `agent_id`, supplied fields override the saved agent for this session. Without `agent_id`, `model` is required." |
| `agent_id` | `string`, maxLength 64 | optional | "The ID of a saved reusable agent. Omit `agent` to use its configuration unchanged." |
| `input` | `string` or array of `AgentSessionInputMessageParam { content, role, type }` or `null` | optional | "Initial input submitted when creating a session." |
| `metadata` | `map[string]` or `null` | optional | "Up to 16 string key-value pairs, with keys up to 64 and values up to 512 characters." |
| `stream` | `boolean` | optional | "Whether to stream session events as server-sent events. Defaults to false." |
| `vault_ids` | array of `string` or `null` | optional | "The IDs of vaults made available to the session." |

`AgentSessionInputMessageParam`: `content` is an array of `InputContentParam`, one of `InputText { text, type: "input_text" }` or `InputImage { image_url, type: "input_image" }`; `role` is `"user"` ("The role of the message author. Always user."); `type` is optional `"message"`.

`EnvironmentParam` is a union on `type`:

| Variant | `type` | Fields (verbatim) |
| --- | --- | --- |
| None | `"none"` | none. "Runs the agent without an execution environment." |
| OpenAIHosted | `"openai_hosted"` | `capability_directories`, `env` "Environment variables made available to the agent.", `environment_template_id` (maxLength 64) "A reusable hosted template applied before inline session configuration. Omitted fields inherit the template; network overrides cannot broaden its policy.", `files` (`FileID { file_id, path, type }` or `Inline { data, path, type }`; `path` is "The absolute destination path inside /workspace"), `network { access: "enabled"/"disabled"/"restricted", allowed_domains }`, `packages { npm, python, system }`, `plugins` (inline base64 ZIP, name and description "declared in .codex-plugin/plugin.json"), `setup_commands` (`{ command, cwd }`, "Ordered, confidential setup commands. Command bodies are never returned."), `skills` (`SkillReference { skill_id, type, version }` where `skill_id` is "The ID of the skill created through /v1/skills" and `version` is "a positive integer or latest", or `Inline { description, name, source, type }`) |
| SelfHosted | `"self_hosted"` | `workspace_directory: string` "Absolute project directory inside the self-hosted environment.", `capability_directories: optional array of string or null` |

### `AgentSession` return object (verbatim)

| Field | Type | Meaning (verbatim) |
| --- | --- | --- |
| `id` | `string` | "The ID of the session." |
| `agent` | `object { id, instructions, model, multi_agent, name, reasoning, service_tier, text, tools }` | "The agent running in the session." |
| `agent.name` | `string` or `null` | "The reusable agent's name when the session was created, or null if no name was saved. **Later changes to the agent's name do not affect this value.**" |
| `created_at` | `number` (int64) | Unix timestamp, seconds. |
| `environment` | `None`/`OpenAIHosted`/`SelfHosted` | For `SelfHosted`: `id` "The public ID of the environment.", `remote_url` "Pass this URL unchanged to `codex exec-server --remote` when connecting this environment.", `capability_directories`, `workspace_directory` "The absolute project directory inside the environment. Defaults to /workspace." For `OpenAIHosted`, installed `plugins` and `skills` are echoed back ("excluding their archive contents"), and `HostedSkillReference` carries `version` = "The concrete skill version installed for this session." |
| `error` | `string` or `null` | "The error that caused the session to fail, if any." |
| `last_active_at` | `number` (int64) | "The Unix timestamp, in seconds, when the session was last active." |
| `metadata` | `map[string]` | "Custom string key-value pairs attached to the session." |
| `object` | `"agent.session"` | "The object type. Always agent.session." |
| `required_actions` | array of `FunctionCall` or `EnvironmentConnection` | "Actions that must be completed before the session can continue." |
| `status` | `"idle"`/`"in_progress"`/`"requires_action"`/`"failed"` | "The current status of the session." |
| `usage` | `TokenUsage { input_tokens, input_tokens_details, output_tokens, output_tokens_details, total_tokens }` or `null` | "Recorded token usage for a session or turn. Usage is best effort and may change." |
| `vault_ids` | array of `string` | "The IDs of vaults made available to the session." |

`required_actions` union members, verbatim:

- `FunctionCall` "Run a function tool and submit its result." with `arguments: unknown` "The arguments supplied by the model.", `call_id: string` "The ID to include when submitting the function result.", `name: string`, `turn_id: string` "The ID of the turn that requested the function call.", `type: "function_call"`.
- `EnvironmentConnection` "Reconnect a session environment." with `environment_id: string`, `type: "environment_connection"`.

Session status values, verbatim: `"idle"` = "The session has no turn in progress and is ready for input. A hosted environment may still be provisioning."; `"in_progress"` = "The session is processing a turn."; `"requires_action"` = "The session is waiting for one or more required actions."; `"failed"` = "The session failed."

### Is the binding immutable?

There is no session update endpoint in the fetched reference. The documented session methods are create, retrieve, list, delete, plus the subresources (events, items, turns, artifacts, subagents). `vault_ids`, `environment`, `agent_id` and the inline `agent` override are therefore set only at creation and there is no documented way to change them afterwards. The docs do not state "immutable" in so many words, so this is an inference from the absence of a mutating endpoint, not a quoted claim.

### Session and `conversation`

**Not documented.** There is no relationship stated anywhere between an Agents API session and the separate Conversations resource (`POST /conversations`, `object: "conversation"`). Grepping every fetched Agents API reference schema (`ref_sessions_create`, all `sess_*`, all `agent_*`, all `vault_*`) for "conversation" returns zero hits. In the Agents API guides the word appears only as ordinary English: "The session holds the conversation and work", "Each session has its own conversation and work", "Store `session.id` with your application's conversation state". The Conversation state guide never mentions the Agents API or agent sessions. Conclusion: within the Agents API, "conversation" is the durable item history owned by the session itself, and the Conversations resource is an unrelated Responses API construct.

### Turns

`Turn` object, `object: "agent.session.turn"`, "The canonical public representation of a session turn."

| Field | Type | Meaning (verbatim) |
| --- | --- | --- |
| `id` | `string` | "The ID of the turn." |
| `agent_id` | `string` | "The ID of the agent that ran the turn." |
| `completed_at` | `number` or `null` | "The Unix timestamp, in seconds, when the turn reached a terminal state." |
| `created_at` | `number` | "The Unix timestamp, in seconds, used to order the turn by creation time. Subagent turns use their start time, falling back to completion time or the subagent opening time when the preceding timestamps are unavailable." |
| `error` | `SessionTurnError { code, message }` or `null` | "A customer-safe error describing why a session request failed." |
| `object` | `"agent.session.turn"` | "The object type. Always agent.session.turn." |
| `session_id` | `string` | "The ID of the session that owns the turn." |
| `started_at` | `number` or `null` | "The Unix timestamp, in seconds, when the turn started." |
| `status` | `"queued"`/`"in_progress"`/`"waiting"`/`"completed"`/`"failed"`/`"cancelled"` | "The current status of the turn." |
| `subagent_id` | `string` or `null` | "The ID of the subagent that ran the turn, if applicable." |
| `usage` | `TokenUsage` or `null` | "Recorded token usage for a session or turn. Usage is best effort and may change." |

`SessionTurnError.code` is "A stable, machine-readable failure category" with 17 values, verbatim: `"context_length_exceeded"`, `"session_budget_exceeded"`, `"usage_limit_exceeded"`, `"credit_balance_exhausted"`, `"rate_limit_exceeded"`, `"server_overloaded"`, `"cyber_policy"` ("The request was rejected by a safety policy."), `"connection_failed"`, `"server_error"`, `"authentication_error"`, `"invalid_request"`, `"resource_not_found"`, `"sandbox_error"`, `"executor_version_incompatible"` ("The executor must be upgraded before it can run this turn."), `"active_turn_not_steerable"` ("The session cannot accept additional input while a request is running."), `"request_timeout"`, `"internal_error"`.

Turn status values: `"queued"` = "The turn is waiting to start."; `"in_progress"`; `"waiting"` = "The turn is waiting for external input."; `"completed"`; `"failed"`; `"cancelled"`.

### Delegation and subagents

Delegation is a per-session capability switched on by the agent's `multi_agent` config: "Set `multi_agent.enabled` to `true`" (<https://developers.openai.com/api/docs/guides/agents-api/multi-agent.md>). The harness owns the delegation tools: "The harness supplies tools to create, message, wait for, and interrupt subagents. You do not declare these tools yourself."

Binding time is session creation, verbatim: "These settings apply at session creation. Changes to a stored agent apply to new sessions."

The only documented limit is a concurrency limit, not a depth limit: "`max_concurrent_subagents` limits how many subagents can run at once. The default is `6`, excluding the coordinator. Set a positive integer when delegation is enabled." The schema gives `number` (int64, min 1, max 4294967295). **No nesting depth cap is documented anywhere**, and the guide never states whether a subagent may itself create a subagent.

Subagents do not get their own environment: "The coordinator and subagents share its filesystem. Creating a subagent does not create another environment."

Inheritance is partial and explicitly enumerated: "Subagents inherit configured MCP tools, their credentials and allowed tools, and web search settings. They can also use the environment's files and command-line tools. **Subagents do not support function tools**."

Coordination is visible as ordinary items and events. Item types: `create_subagent_call`, `send_subagent_input_call`, `wait_for_subagents_call`, `interrupt_subagent_call`, plus `resume_subagent_call` and `close_subagent_call` in the items schema. Events: `agent.session.subagent.created` ("provides the new subagent's ID"), `agent.session.subagent.active`, `agent.session.subagent.closed`. Caveat, verbatim: "A completed create or wait action does not mean the subagent finished its task."

Attribution runs through two fields: `Turn.subagent_id` is "The ID of the subagent that ran the turn, if applicable" and is "`null` for the main agent", and "On a create item, `agent_id` identifies the agent that requested the subagent." Each subagent has its own item history, addressed at `/agents/sessions/{session_id}/subagents/{subagent_id}/items` and at a per-turn variant under `/subagents/{subagent_id}/turns/{turn_id}/items`.

The inter-agent transcript is not fully exposed: "Coordination items can omit message content. An `agent_message` item contains inter-agent text when available, but the stream does not provide a full conversation transcript."

### Submitting work, steering, interrupting

Work is submitted by `POST /agents/sessions/{session_id}/events` ("Submits message, cancellation, or tool-result events to a managed agent session"), which accepts an optional `Idempotency-Key` header (minLength 1, maxLength 256) and a body field `events: array of AgentSessionInputParam`. Three input event types, verbatim:

| `type` | Meaning (verbatim) | Fields |
| --- | --- | --- |
| `"agent.session.input.message"` | "Adds one or more user messages and starts a turn." | `input: array of AgentSessionInputMessageParam` |
| `"agent.session.input.cancel"` | "Cancels the session's active turn." | none |
| `"agent.session.input.tool_result"` | "Submits the result of a function call." | `call_id: string`, `success: boolean`, `turn_id: string`, `error: optional string or null` "The error message when the call failed.", `output: optional AgentFunctionCallOutputParam or null` (string or array of `InputContentParam`) |

Steering is implicit, not a separate verb: "A turn is one cycle of work within a session. A message sent to an idle session starts a new turn. **A message sent during an active turn steers that turn.**" And: "Send another `agent.session.input.message` to the same session. If the agent is working, the message steers the active turn. If the session is idle, it starts a new turn with the existing conversation." Interrupting is `agent.session.input.cancel`: "Cancel the current turn when you want the agent to stop. The session and its previous work remain available."

This is a different mechanism from the Responses API `response.steer` WebSocket protocol documented at `/api/docs/guides/steering.md`, which has its own `response.steer.pending`, `required_input`, and errors such as `too_many_pending_steers`. The steering guide does not mention the Agents API. Do not conflate the two.

### Event log shape, ordering, durability

Reading the stream: `GET /agents/sessions/{session_id}/events`, "Streams live events for an agent session." The only parameter is the `session_id` path parameter. **There is no cursor, no `after`, no `last_event_id`, and no resume parameter on this endpoint.**

Events carry `event_id`, `session_id`, and `type` (for example `AgentSessionErrorEvent { error, event_id, session_id, type }`). Item-scoped events carry `item_id`.

Documented event type names collected verbatim from the streaming-events reference and the sessions guides:

- Session lifecycle: `agent.session.created`, `agent.session.in_progress`, `agent.session.idle`, `agent.session.failed`, `agent.session.requires_action`, `agent.session.action_required`
- Environment: `agent.session.environment.pending`, `agent.session.environment.connected`, `agent.session.environment.ready`, `agent.session.environment.disconnected`, `agent.session.environment.failed`
- Turn lifecycle: `agent.session.turn.created`, `agent.session.turn.in_progress`, `agent.session.turn.completed`, `agent.session.turn.failed`, `agent.session.turn.cancelled`
- Turn content: `agent.session.turn.item.added`, `agent.session.turn.item.done`, `agent.session.turn.content_part.added`, `agent.session.turn.content_part.done`, `agent.session.turn.output_text.delta`, `agent.session.turn.output_text.done`, `agent.session.turn.reasoning_summary_part.added`, `agent.session.turn.reasoning_summary_part.done`, `agent.session.turn.reasoning_summary_text.delta`, `agent.session.turn.reasoning_summary_text.done`
- Subagents: `agent.session.subagent.created`, `agent.session.subagent.active`, `agent.session.subagent.closed`
- Input echo: `agent.session.input.message`

Durability and resumability, verbatim: "**Streams do not replay missed events.** To restore your application's view: 1. Open a new stream and buffer incoming events. 2. Retrieve the session and its saved items while the stream stays connected. 3. Restore your local state from those items, keyed by item ID. 4. Apply buffered item updates using `item_id`. Discard updates for items that already reached their final state in the retrieved history. 5. Resume handling live events." And: "An `output_text.done` event can replace a temporary text buffer with the complete text. **Saved items let you recover completed work, but not every intermediate event you missed.**"

Durability therefore lives in the item store, not the event log. Pagination over items is ID-based: `GET /agents/sessions/{session_id}/items` takes `after` ("Return resources after this resource ID in the selected order"), `limit` ("between 1 and 100. Defaults to 20"), and `order` (`"asc"`/`"desc"`, defaults to `desc`). The guide says "List endpoints return one page at a time. Use SDK pagination helpers or the `after` cursor to retrieve more results. A single page may not contain every item for a turn. Use `order: \"asc\"` to read items from oldest to newest."

Item `type` values observed verbatim in the items list schema: `message`, `reasoning`, `function_call`, `function_call_output`, `mcp_call`, `command_execution`, `web_search_call`, `create_subagent_call`, `send_subagent_input_call`, `interrupt_subagent_call`, `resume_subagent_call`, `close_subagent_call`, `wait_for_subagents_call`. Items carry `turn_id`, `id`, `object`, `status` (`in_progress`, `completed`, `incomplete`, `failed`), and role-specific content.

Caution: the events guide says "For a root-agent turn, filter session items by `turn_id`", but `turn_id` is a **response** field on items and is **not** among the documented query parameters of `GET /agents/sessions/{session_id}/items` (which are only `after`, `limit`, `order`). Filtering by turn appears to be client side. This is a documentation inconsistency, not a verified server capability.

### Webhooks versus the event stream

Webhooks are a deliberately coarser channel. Exactly five webhook event types are documented, verbatim from the table:

| Webhook event | Meaning (verbatim) |
| --- | --- |
| `agent.session.created` | "A session is created." |
| `agent.session.action_required` | "The session needs a function result, initial environment connection, or reconnection." |
| `agent.session.in_progress` | "The session starts processing a turn." |
| `agent.session.idle` | "The session is idle and ready for more input." |
| `agent.session.failed` | "The session enters a failed state." |

The docs are explicit about the semantic trap: "`agent.session.idle` means the session is ready for more input, not that its last turn succeeded. Inspect that turn's status or observe `agent.session.turn.completed`, `agent.session.turn.failed`, or `agent.session.turn.cancelled` on the session stream. A completed turn can still contain failed tool calls." And: "`agent.session.failed` reports a failed session, not every failed turn. **Session deletion has no corresponding webhook and does not stop provider compute.**"

The same required action surfaces under two different names depending on channel: "The session stream reports the same request as `agent.session.requires_action`" while the webhook is `agent.session.action_required`. For self-hosted sessions the `agent.session.created` webhook carries `data.environment_id` and `data.connect.remote_url`, but the `agent.session.action_required` webhook does not: "This webhook does not include `connect.remote_url`."

### Session lifecycle

Create (optionally with `input` and `stream`). Turns run. `GET /agents/sessions` lists with `after`, `agent_id` ("Only return sessions whose root agent has this ID. Omit to return sessions for all agents."), `limit`, `order`. `GET /agents/sessions/{session_id}` retrieves. `DELETE /agents/sessions/{session_id}` "Removes a managed agent session from the public API and returns a deletion confirmation. Physical cleanup may continue asynchronously." It returns `AgentSessionDeleted { id, deleted, object }`.

Session lifetime, TTL, maximum turn count, and maximum age are **not documented**.

## Vaults and secrets

### What a vault is

"A vault stores credentials for MCP connections from OpenAI. Attach it to a session so the agent can use authenticated tools without receiving the secret values." The resource description is "A collection of credentials that agent tools can use to authenticate to MCP servers." Credential create says: "Secret values are write-only and are never returned."

### `Vault` object (verbatim)

| Field | Type | Required | Meaning (verbatim) |
| --- | --- | --- | --- |
| `id` | `string` | returned | "The ID of the vault." |
| `created_at` | `number` (int64) | returned | "The Unix timestamp, in seconds, when the vault was created." |
| `metadata` | `map[string]` | returned | "Key-value pairs associated with the vault, such as an application or team identifier." |
| `name` | `string` or `null` | returned | "The human-readable name of the vault, if set." |
| `object` | `"vault"` | returned | "The object type. Always vault." |

`POST /vaults` body: `metadata` (optional) and `name` (optional, "The name is trimmed before storage. It must contain 1 to 256 UTF-8 bytes after trimming."). No `updated_at`, no `status`, no `project_id` on the returned object.

### `vault.credential` object (verbatim)

Create is `POST /vaults/{vault_id}/credentials` with body `auth: CredentialAuthCreateParam` and `name: string`.

| Field | Type | Meaning (verbatim) |
| --- | --- | --- |
| `id` | `string` | "The ID of the credential." |
| `auth` | `CredentialAuth` | "The authentication method and non-secret configuration for the MCP server." |
| `created_at` | `number` (int64) | Unix timestamp. |
| `name` | `string` | "The human-readable name of the credential." |
| `object` | `"vault.credential"` | "The object type. Always vault.credential." |
| `updated_at` | `number` (int64) | "The Unix timestamp, in seconds, when the credential was last updated." |
| `vault_id` | `string` | "The ID of the vault containing this credential." |

Exactly two credential types exist.

`StaticBearer` ("A bearer token for an MCP server, without automatic OAuth refresh"), `type: "static_bearer"`:

| Field | Write | Read back | Meaning (verbatim) |
| --- | --- | --- | --- |
| `token` | required | never | "The bearer token to store. This secret is never returned in credential resources." |
| `mcp_server_url` | required | yes | "The HTTPS MCP server URL authorized by this credential." |

`McpOauth` ("An OAuth credential for an HTTPS MCP destination"), `type: "mcp_oauth"`:

| Field | Write | Read back | Meaning (verbatim) |
| --- | --- | --- | --- |
| `access_token` | required | never | "A write-only OAuth access token; never returned by credential resources." |
| `mcp_server_url` | required | yes | "The HTTPS MCP server URL authorized by this credential." |
| `expires_at` | optional | yes | "When the OAuth access token expires, as an RFC 3339 timestamp, if known." |
| `refresh.client_id` | required in `refresh` | yes | "The OAuth client ID used when requesting a new access token." |
| `refresh.refresh_token` | required in `refresh` | never | "The refresh token to store. This secret is never returned in credential resources." |
| `refresh.token_endpoint` | required in `refresh` | yes | "The HTTPS OAuth token endpoint used to exchange the refresh token for a new access token." |
| `refresh.token_endpoint_auth` | required in `refresh` | yes, minus the secret | `None { type: "none" }`, `ClientSecretBasic { client_secret, type: "client_secret_basic" }`, or `ClientSecretPost { client_secret, type: "client_secret_post" }`. Read-back variants carry only `type`: "How the OAuth client authenticates to the token endpoint, excluding its client secret." |
| `refresh.resource` | optional | yes | "The resource URI to send to the OAuth token endpoint during refresh, if required." |
| `refresh.scope` | optional | yes | "Space-separated OAuth scopes to request during refresh, if required." |

Write-only is enforced structurally: the request union (`CredentialAuthCreateParam`) and the response union (`CredentialAuth`) are different shapes. The response variants simply do not have the secret fields. There is no redaction placeholder and no "last four" hint.

### How a secret binds to a tool

Binding is by URL matching, not by explicit reference. The chain, verbatim:

1. "`mcp_server_url` binds the credential to that server."
2. "Pass the saved ID in `vault_ids` when creating a session. Use the same server URL in the MCP configuration."
3. "The Agents API selects a credential that matches the server URL. If several attached credentials match, set the MCP tool's `credential_id` to select one."
4. "Vaults apply only to connections from OpenAI." And: "**Environment-origin HTTP does not use vault credentials**; use inline authentication or a trusted proxy."
5. "Use one source for `Authorization`: inline configuration or a matching vault credential. Other headers can accompany vault authentication."

So the scoping model is: vault -> project; credential -> one `mcp_server_url`; session -> `vault_ids[]` at creation; tool -> matched by URL, optionally disambiguated by `credential_id`; and the whole mechanism is inert for `connection_origin: "environment"` and for stdio transports.

### How the secret reaches running code

It does not. That is the point of the design. The secret never enters the sandbox. The service-side MCP client attaches it on the outbound request from "the Managed Agents service network". For anything that must run inside the environment, the docs explicitly refuse to provide a secure path: "Stdio credentials: Supply values in the environment and list their names in `transport.env_vars`. **These values can be read by code running in the environment.** Self-hosted sessions do not accept inline values in `transport.env`." And: "Keep secrets out of reusable agent definitions, plugin archives, and logs. To keep credentials inaccessible to agent-generated code, use a trusted proxy or server that supplies them outside the environment."

### Vault lifecycle

Create, list (`GET /vaults`), retrieve, delete. Credentials: create, list, retrieve, update, delete. Update semantics, verbatim: "Update a credential to replace its token without changing its ID, authentication type, or server URL." And "Include `expires_at` when the replacement token expires. Supplying a new access token without an expiry clears the stored expiry; an explicit `null` also clears it." Deletion semantics, verbatim: "Delete a vault to remove the vault and all its credentials." And the important non-guarantee: "**Deleting stored credentials does not revoke the original tokens with their providers or stop a running session.** Your application handles provider-side revocation and session cancellation." Also: "If an expired token cannot be refreshed, supply a valid replacement. Token expiry does not delete the credential or its vault."

`GET /vaults` query parameters: `after`, `limit` ("Defaults to 20. Values are clamped between 1 and 100"), `order` (defaults to `desc`), and `status`: "Filter by one status or a list, such as `status=active` or `status[]=active&status[]=archived`. Both statuses are included by default." `VaultStatus` = `"active"` or `"archived"`, "Whether a vault or credential is active or archived."

### Workload identity federation (a different problem)

WIF solves the problem of not storing an OpenAI credential at all: "Workload identity federation lets a trusted workload use an identity it already has instead of storing an OpenAI API key or ChatGPT credential. The workload presents a short-lived token from your identity provider, and OpenAI exchanges it for a short-lived OpenAI access token."

Three configured pieces, verbatim: "1. An **identity provider** tells OpenAI which external issuer to trust and how to verify its signed tokens or certificate identities. 2. An **access rule** describes which token attributes OpenAI accepts and which OpenAI identity the workload may act as. OpenAI API configuration calls this a service account mapping. Codex configuration calls it a federation rule. 3. An **OpenAI principal** receives the resulting access."

Token exchange is OAuth 2.0 token exchange at `POST /oauth/token`:

| Parameter | Required | Description (verbatim) |
| --- | --- | --- |
| `grant_type` | Yes | "Must be `urn:ietf:params:oauth:grant-type:token-exchange`." |
| `subject_token_type` | Yes | "Supports `urn:ietf:params:oauth:token-type:jwt` and `urn:ietf:params:oauth:token-type:id_token`." For X.509: "Must be `urn:openai:params:oauth:token-type:x509`." |
| `subject_token` | Yes (JWT), No (X.509) | "The externally issued OIDC JWT or SPIFFE JWT-SVID from your Workload Identity Provider." For X.509: "Omit this parameter. OpenAI obtains certificate identity only from the authenticated TLS connection." |
| `identity_provider_id` | Yes | "The OpenAI Workload Identity Provider ID configured for the external issuer." |
| `service_account_id` | Yes | "The OpenAI service account ID to resolve against the matching service account mapping." |

Response fields: `access_token`, `expires_in` ("Access-token lifetime in seconds, measured from issuance (`iat`)"), `expires_at` ("Absolute expiration as a Unix timestamp in seconds ... Equals the issued access token's `exp` claim"), and `scope` ("returned only when the resolved mapping has permissions"). Constraints, verbatim: "Access tokens expire after at most one hour. A JWT exchange token never outlives its external subject token, and an X.509 exchange token never outlives the verified client certificate. **Token exchange doesn't return a refresh token.**" And "The X.509 endpoint accepts only exact `POST /oauth/token` requests on `mtls.auth.openai.com`. Other methods and paths return HTTP `403`."

Mapping configuration options, verbatim: `Name`, `Key` ("The attribute key to match. Use a raw token claim, such as `sub`, `aud`, or `iss`, or a derived attribute like `openai.subject`."), `Value`, `Description`, `Project`, `Service account`, `Permissions` ("Optional API permissions that further narrow access tokens minted from this mapping. These permissions can't grant access beyond the mapped service account."). Resolution: "OpenAI looks up mappings for the requested `identity_provider_id` and `service_account_id`, skips mappings that aren't enabled, evaluates only the attributes needed by each mapping, and **issues a token only if exactly one enabled mapping matches every configured attribute**." And "OpenAI enforces a unique mapping for each `(provider, service account)` pair and doesn't combine permissions from different mappings."

Value matching supports "one trailing wildcard with a non-empty prefix, such as `repo:example/*`. A wildcard by itself or in the middle of a value isn't supported." Valid: `repo:openai/*`, `repository:my-org/*`. Unsupported: `*`, `repo:*:prod`, `repo/*/main`.

Attribute transformations use CEL: "OpenAI supports the standard CEL operators specified in langdef.md and doesn't add custom workload identity federation functions. Each expression receives one root object: `assertion`: The verified JWT claim set." Results "must be scalar values: strings, `true` or `false` values, integers, or finite numbers. Arrays, objects, null values, and evaluation errors fail mapping resolution. OpenAI converts scalar transformation results to strings before comparing them to mapping values." And "Mapping keys that start with `openai.` resolve only from attribute transformations."

JWKS caching: "OpenAI caches discovery documents and remote JWKS payloads for 600 seconds" with "Key refresh on miss".

Token exchange error categories, verbatim: "Missing JWT request parameter", "Unsupported token request", "Provider resolution error", "JWT subject token verification", "X.509 certificate verification", "Mapping resolution". For X.509 specifically, `invalid_subject_token` is returned for "Malformed or missing certificate material, an invalid chain, a root mismatch, a certificate outside its validity period, or rejection by a Mutual TLS certificate-admission rule", while "Rejection by the provider's Attribute conditions expression returns `invalid_grant`."

Note that WIF is about authenticating **to** OpenAI. It is not a mechanism for giving an agent access to third-party services. Nothing in the fetched Agents API docs connects WIF to vaults, sessions, or MCP credentials.

### Realtime client secrets (a third secret shape)

`POST /realtime/client_secrets`, "Create a Realtime client secret with an associated session configuration." Verbatim: "Client secrets are short-lived tokens that can be passed to a client app, such as a web frontend or mobile client, which grants access to the Realtime API without leaking your main API key. You can configure a custom TTL for each client secret." "The client secret is a string that looks like `ek_1234`."

| Field | Type | Meaning (verbatim) |
| --- | --- | --- |
| `expires_after.anchor` | optional `"created_at"` | "Only `created_at` is currently supported." |
| `expires_after.seconds` | optional `number` | "Select a value between `10` and `7200` (2 hours). This default to 600 seconds (10 minutes) if not specified." |
| `session` | optional `RealtimeSessionCreateRequest` or `RealtimeTranscriptionSessionCreateRequest` | "Session configuration to use for the client secret." |

Unlike a vault credential, this token is meant to be handed to an untrusted client, it carries attached session configuration, and "A secret can be used to create multiple sessions until it expires." Session configuration attached to it "can also be overridden by the client connection". This is a bearer-token-with-policy shape, not a stored-secret-injection shape. It belongs to the Realtime API, not the Agents API.

## Identity and versioning

**Naming.** Every resource is addressed by an opaque server-issued `id`. `name` is a non-unique human label: on an agent it is optional and nullable (maxLength 128); on a vault and a credential it is "trimmed before storage. It must contain 1 to 256 UTF-8 bytes after trimming." Nothing in the docs claims uniqueness for any `name`. There is no namespace, no slug, and no user-chosen ID anywhere in the Agents API.

**Addressing.** Paths are flat and ID-based: `/agents/{agent_id}`, `/agents/sessions/{session_id}`, `/agents/sessions/{session_id}/turns/{turn_id}`, `/agents/sessions/{session_id}/artifacts/{artifact_id}`, `/vaults/{vault_id}`, `/vaults/{vault_id}/credentials/{credential_id}`. `agent_id` on session create has `maxLength 64`; `skill_id` and `environment_template_id` also have `maxLength 64`.

**Pagination.** Uniformly ID-based, not timestamp-based or offset-based: `after` = "Return resources after this resource ID in the selected order", plus `limit` and `order` (`"asc"`/`"desc"`, defaults to `desc`). The docs call `after` a "cursor" in prose but the parameter is a resource ID.

**Versioning of the API surface.** One header, `OpenAI-Beta: agents=v1`, and the reference tree lives under `resources/beta/subresources/agents`. There is no dated version pin and no per-request version override documented.

**Versioning of an agent.** There is none. The `Agent` object has no `version`, `revision`, `etag`, or `digest`. `POST /agents/{agent_id}` mutates in place. The only pinning of any kind is a single snapshot copy at session creation, and only of the display name: `session.agent.name` is "The reusable agent's name when the session was created, or null if no name was saved. **Later changes to the agent's name do not affect this value.**" Nothing says the same about `model`, `instructions`, or `tools`. Whether a long-lived session picks up later edits to its saved agent's model or tools is **not documented**.

**The only real version pin in the system** is on skills: `SkillReference.version` = "The skill version, a positive integer or latest; omission selects the default", and the session echoes back `HostedSkillReference.version` = "The concrete skill version installed for this session." That is a genuine resolve-and-pin, and it stands in sharp contrast to the agent, which has no equivalent.

**Optimistic concurrency.** Not documented. No `If-Match`, no `etag`, no `previous_version` anywhere.

**Idempotency.** Documented in exactly one place: the optional `Idempotency-Key` header (minLength 1, maxLength 256) on `POST /agents/sessions/{session_id}/events`. It is not documented on session create, agent create, or vault credential create.

## Notable design decisions

**1. The agent is a resource, but a thin and unversioned one.** Making the agent server-side removes config drift between callers and keeps the definition out of every request body. But the design stops short: mutation is in-place with no revision history, and the session copies only the `name`. A team that edits a production agent has no documented way to tell which configuration a past session actually ran with, and no way to pin one. The docs give no reasoning for this; the closest they come is the framing "The session holds the conversation and work, while the saved agent holds reusable settings", which treats the agent as configuration rather than as an audited artifact.

**2. Override replaces, never merges.** "Supplied objects and arrays replace the entire field rather than merging with the saved value. For example, supplying `tools` replaces the saved tool list." This is the right call for predictability (deep merge of a tool list has no sane semantics) but it means a session that wants to add one tool must restate the entire list, which reintroduces exactly the drift the saved agent was supposed to prevent.

**3. Streams are lossy by design, items are the source of truth.** "Streams do not replay missed events." There is no cursor on the event endpoint. Instead the docs prescribe a five-step reconciliation: open a new stream, buffer, fetch saved items, key by item ID, apply buffered updates by `item_id`, discard updates for already-final items. This is a deliberate split between a best-effort real-time channel and a durable queryable store. The cost is that every serious client must implement the reconciliation, and the docs admit the loss outright: "Saved items let you recover completed work, but not every intermediate event you missed."

**4. Two channels with different names for the same state.** The same required action is `agent.session.action_required` on webhooks and `agent.session.requires_action` on the stream. The webhook set is only five events. The docs flag the resulting semantic hazards explicitly: idle does not mean success, session failure is not turn failure, a completed turn can contain failed tool calls, and session deletion emits nothing.

**5. Vault credentials bind by URL, not by name.** "The Agents API selects a credential that matches the server URL." This makes attachment declarative (add a vault ID, keep the URL consistent, credentials appear) but it is implicit: nothing in the session request states which credential will be used, ambiguity is resolved only by optionally setting `credential_id`, and a URL typo silently produces an unauthenticated call rather than an error.

**6. Vaults are scoped to the network origin, not to the agent.** "Vaults apply only to connections from OpenAI" and "Environment-origin HTTP does not use vault credentials." The trust boundary is drawn at the sandbox wall, not around the tool. This is coherent given decision 7 but it means the vault mechanism is unavailable for exactly the case where an agent needs to reach a private network.

**7. The sandbox is assumed hostile, and the docs say so plainly.** "Agent-generated code can access the files, credentials, and network available to its environment." "Agent-generated code can read the executor key." The mitigation is to make the executor key nearly powerless: "the key only permits connecting environments. It cannot authorize any other API action." For third-party secrets the recommendation is architectural rather than a product feature: "Keep third-party credentials outside the environment. Where possible, route requests through a credential broker. The broker injects secrets into approved outbound requests without placing them in the agent's environment." And bluntly: "**Injecting a stored secret into the environment still exposes it to agent-generated code.**" This is a refreshing refusal to pretend a sandbox secret store is safe, but it also means OpenAI ships no in-sandbox secret primitive at all.

**8. Environments are the caller's problem when self-hosted, including the hard parts.** "An agent session can outlive its environment." The API waits "up to five minutes for an input-time connection". Non-guarantees are stated directly: "The API does not guarantee recovery of pending input after a process crash." "A late connection does not replay input that timed out." "Reusing the environment ID does not restore files in replacement compute." "Deleting a session neither stops its environment nor emits a deletion webhook." "An idle event alone is not a safe shutdown signal." The provider table names nine third-party sandbox providers (Modal, Cloudflare, Vercel, Daytona, Blaxel, E2B, Runloop, DigitalOcean, Oracle Cloud Infrastructure), but the integration contract is uniform and provider-agnostic: run `codex exec-server --remote <remote_url> --environment-id <id>` with a restricted `CODEX_API_KEY`, outbound only, to `https://api.openai.com` and `wss://codex-cloud-environments.chatgpt.com`. There is no provider plugin interface; every provider guide is just a recipe for starting that one process.

**9. Steering is a message, not a verb.** "A message sent during an active turn steers that turn." No separate endpoint, no `steer` event type, no priority flag. The turn error enum carries `active_turn_not_steerable` for the case where it is refused. Compare this to the Responses API, which has an entire WebSocket steering protocol with `response.steer`, `response.steer.pending`, `required_input`, and `too_many_pending_steers`. Two products, two answers to the same question, and the docs never reconcile them.

**10. Write-only secrets are enforced by schema shape, not by redaction.** The create union and the read union are different types, and the read types simply lack the secret fields. No masked placeholder exists, so there is no way for a client to detect whether a secret was ever set beyond the credential existing. The same technique is applied to hosted environment `setup_commands`: "Ordered, confidential setup commands. Command bodies are never returned."

**11. Skills get real version pinning; agents do not.** `version` accepts "a positive integer or latest", and the session records "The concrete skill version installed for this session". The platform clearly understands resolve-and-pin. It just did not apply it to the agent resource.

## Gaps and open questions

The following are **not documented** in any page fetched. These are absences, not inferences about behavior.

1. **Agent versioning.** No `version`, `revision`, `etag`, or `digest` on `Agent`. No way to pin a session to a specific agent configuration.
2. **Effect of mutating a saved agent on live sessions.** Only `session.agent.name` is stated to be snapshotted. Whether `model`, `instructions`, `tools`, `reasoning` are snapshotted or read live is unstated.
3. **Effect of deleting a saved agent.** Not documented. `Turn.agent_id` and the `GET /agents/sessions?agent_id=` filter imply a durable reference, but the deletion behavior is unstated.
4. **The `agent.id` of a session created with inline config only.** The session response requires `agent.id: string`, but the docs never say what that ID is when no `agent_id` was supplied, whether it is stable, or whether it is retrievable via `GET /agents/{agent_id}`.
5. **Session mutability.** No update endpoint exists, so `vault_ids`, `environment`, and the agent binding appear fixed at creation, but the docs never state this as a guarantee.
6. **Session-to-Conversation relationship.** Zero cross-references between the Agents API and the Conversations resource. Zero hits for "conversation" across every Agents API reference schema. The conversation-state guide never mentions agent sessions.
7. **Session TTL, retention, and limits.** No documented session lifetime, maximum turn count, item retention window, maximum concurrent sessions, or maximum session size. The only retention statement is the general one: "data residency only in the United States" and "does not support Zero Data Retention (ZDR)".
8. **Event ordering guarantees.** `event_id` exists but the docs never say it is monotonic, never define a total order, and never say whether events for different turns interleave.
9. **Event stream resumption.** No `Last-Event-ID`, no `after`, no cursor on `GET /agents/sessions/{id}/events`. The documented answer is reconciliation, not resumption.
10. **`turn_id` as an items filter.** The guide says to filter items by `turn_id` but the endpoint documents no such query parameter.
11. **Guardrails in the Agents API.** **Not documented.** The word "guardrail" appears nowhere in any Agents API page. It appears once in `docs_llms.txt` as the description of a separate Agent Builder safety page, which is a different product.
12. **Approvals in the Agents API.** **Not documented** as a first-class mechanism. The Agents API has `required_actions` with exactly two types, `function_call` and `environment_connection`, neither of which is a human approval gate. The `require_approval` / `McpToolApprovalFilter` / `McpToolApprovalSetting` (`"always"` / `"never"`) fields exist only on the Conversations and Responses MCP tool config, and are absent from the entire Agents API reference. An approval workflow in the Agents API must be built by the application, presumably out of a function tool. The only hint is a showcase blurb: "investigate alerts and request approval for recovery actions."
13. **Treating tool output as untrusted.** **Not documented** for the Agents API. There is no prompt-injection guidance, no untrusted-content marking, no provenance field on `mcp_call` or `function_call_output` items. The security guidance runs the other direction only: it treats the *agent* as the untrusted party relative to *your* credentials ("Agent-generated code can access the files, credentials, and network available to its environment"). The only prompt-injection page in the corpus index belongs to Agent Builder and was not fetched.
14. **Vault `status`.** `GET /vaults` accepts `status=active` / `status=archived` and defines `VaultStatus` as applying to "a vault or credential", yet neither the `Vault` object nor the `vault.credential` object documents a `status` field, and no endpoint for archiving is documented. Either the field is undocumented on the resource or the archive operation is undocumented.
15. **Vault scoping granularity.** Vaults are "for the current project". There is no documented way to restrict a vault to specific agents, specific sessions beyond the per-session `vault_ids` list, or specific principals.
16. **Credential usage audit.** No documented event, item, or field records which credential was selected for a given `mcp_call`.
17. **RBAC coverage.** The RBAC guide's permission table lists "Agent Builder" but documents no Agents API or Vaults permissions, while the agents guides reference `api.agents.read`, `api.agents.write`, `api.vaults.read`, `api.vaults.write`, and `api.responses.write`. The two documents disagree about what permissions exist.
18. **Rate limits and quotas.** `rate_limit_exceeded`, `session_budget_exceeded`, and `usage_limit_exceeded` exist as turn error codes, but no numeric limits, headers, or budget configuration are documented. What sets a "session budget" is unstated.
19. **HTTP status codes.** None of the fetched Agents API reference pages document response status codes. All error information comes from the `SessionTurnError.code` enum, which describes turn outcomes rather than HTTP responses.
20. **Agent object `updated_at`.** An update endpoint exists; no corresponding timestamp field is documented on `Agent` (unlike `vault.credential`, which has one).

21. **Subagent nesting depth.** `max_concurrent_subagents` caps concurrency only. No maximum delegation depth is documented, and whether a subagent can itself create subagents is unstated. Nothing documents a per-subagent budget, timeout, or independent failure policy.
22. **Subagent event durability.** Subagent items are retrievable under `/agents/sessions/{session_id}/subagents/{subagent_id}/items`, but the inter-agent transcript is explicitly incomplete: "the stream does not provide a full conversation transcript."

## Sources

Fetched as markdown by appending `.md`:

- <https://developers.openai.com/api/docs/guides/agents-api.md>
- <https://developers.openai.com/api/docs/guides/agents-api/architecture.md>
- <https://developers.openai.com/api/docs/guides/agents-api/configuration.md>
- <https://developers.openai.com/api/docs/guides/agents-api/quickstart.md>
- <https://developers.openai.com/api/docs/guides/agents-api/sessions.md>
- <https://developers.openai.com/api/docs/guides/agents-api/sessions/manage.md>
- <https://developers.openai.com/api/docs/guides/agents-api/sessions/events.md>
- <https://developers.openai.com/api/docs/guides/agents-api/sessions/webhooks.md>
- <https://developers.openai.com/api/docs/guides/agents-api/tools/vaults.md>
- <https://developers.openai.com/api/docs/guides/agents-api/tools/mcp.md>
- <https://developers.openai.com/api/docs/guides/agents-api/tools/functions.md>
- <https://developers.openai.com/api/docs/guides/agents-api/tools/plugins.md>
- <https://developers.openai.com/api/docs/guides/agents-api/tools/web-search.md>
- <https://developers.openai.com/api/docs/guides/agents-api/environments/lifecycle.md>
- <https://developers.openai.com/api/docs/guides/agents-api/environments/security.md>
- <https://developers.openai.com/api/docs/guides/agents-api/environments/openai-hosted.md>
- <https://developers.openai.com/api/docs/guides/agents-api/environments/self-hosted.md>
- <https://developers.openai.com/api/docs/guides/agents-api/environments/files.md>
- <https://developers.openai.com/api/docs/guides/agents-api/multi-agent.md>
- <https://developers.openai.com/api/docs/guides/agents-api/observability.md>
- <https://developers.openai.com/api/docs/guides/agents-api/tracing.md>
- <https://developers.openai.com/api/docs/guides/conversation-state.md>
- <https://developers.openai.com/api/docs/guides/steering.md>
- <https://developers.openai.com/api/docs/guides/rbac.md>
- <https://developers.openai.com/api/docs/guides/secure-mcp-tunnels.md>
- <https://developers.openai.com/api/docs/guides/workload-identity-federation.md>
- <https://developers.openai.com/api/docs/guides/workload-identity-federation/federation-rules.md>
- <https://developers.openai.com/api/reference/workload-identity-federation.md>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/streaming-events.md>
- <https://developers.openai.com/api/reference/resources/conversations.md>
- <https://developers.openai.com/api/reference/resources/conversations/methods/create.md>
- <https://developers.openai.com/api/reference/resources/containers.md>
- <https://developers.openai.com/api/reference/resources/realtime/subresources/client_secrets/methods/create.md>
- <https://developers.openai.com/llms.txt>, <https://developers.openai.com/api/llms.txt>, <https://developers.openai.com/api/reference/llms.txt>, <https://developers.openai.com/api/docs/llms.txt>, <https://developers.openai.com/api/reference/llms-full.txt>

Fetched as HTML and converted locally, because the `.md` variant returns HTTP 404 for these deep reference method pages:

- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/methods/create>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/methods/retrieve>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/methods/update>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/methods/delete>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/methods/create>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/methods/retrieve>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/methods/delete>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/events/methods/create>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/events/methods/stream>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/items/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/turns/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/turns/methods/retrieve>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/artifacts/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/artifacts/methods/retrieve>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/artifacts/methods/content>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/artifacts/methods/delete>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/sessions/subresources/subagents/subresources/items/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/methods/create>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/methods/retrieve>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/methods/delete>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/subresources/credentials/methods/create>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/subresources/credentials/methods/list>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/subresources/credentials/methods/retrieve>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/subresources/credentials/methods/update>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/vaults/subresources/credentials/methods/delete>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/environments/subresources/templates/methods/create>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/environments/subresources/files/methods/create>

URLs probed that returned HTTP 404 and were abandoned:

- <https://developers.openai.com/api/reference/resources.md>
- <https://developers.openai.com/api/reference/resources/sessions.md>
- <https://developers.openai.com/api/reference/resources/agents.md>
- <https://developers.openai.com/api/reference/resources/beta/subresources/agents/subresources/agents/methods/create> (and `list`, `retrieve`, `update`, `delete`)
- The `.md` variant of every reference method page listed in the HTML section above.
