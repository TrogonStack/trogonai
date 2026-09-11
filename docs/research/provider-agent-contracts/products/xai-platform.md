# xAI: Sessions, Agents, Secrets

Part of the [provider agent contracts research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-09-11. Every claim is sourced inline to a
vendor documentation page or machine-readable spec. These documentation sites
are unversioned and publish no commit identifiers, so the authoritative
anchors are the pinned entry points below plus the API version header each
surface requires:

- xAI [platform documentation](https://docs.x.ai/developers), the
  [`grok` CLI documentation](https://docs.x.ai/build), and the
  machine-readable [`openapi.json`](https://docs.x.ai/openapi.json).
- No version pin exists: xAI publishes no beta header and no dated API
  version for the surfaces studied here. The `openapi.json` snapshot is the
  only machine-checkable anchor, and it is incomplete in ways the dossier
  records explicitly.

xAI does **not** have a managed agent platform. There is no server-side agent resource, no agent definition that is stored, versioned, activated or deployed, and no server-side session or conversation resource. What xAI has instead is: (1) a stateful-by-default **Responses API** where the unit of stored state is a single `response` chained by `previous_response_id`, retained for a fixed 30 days; (2) agentic behavior expressed entirely **per request** as `tools` + `max_turns` on `POST /v1/responses`, with a server-side tool loop; (3) a **multi-agent capability that is a model name**, `grok-4.20-multi-agent`, whose sub-agents are internal, non-addressable and encrypted; (4) **hosted Skills** (`/v1/skills`), the closest thing to a stored, reusable behavior artifact, which is uploaded as a zip of `SKILL.md` and is present in the OpenAPI spec but has no documentation page; and (5) a credential model with four distinct types (inference API key, management key, ephemeral realtime token, mTLS certificate) and **no vault**: third-party credentials for MCP servers are passed inline in the request body on every call. A separate client-side coding agent product (`grok` CLI, docs.x.ai/build) does have sessions, subagents, permissions and a sandbox, but all of it lives on the user's disk under `~/.grok/`, not as an API resource.

Caveat on evidence: `https://docs.x.ai/openapi.json` is **not** an exhaustive platform inventory. It covers the `api.x.ai` inference surface only, and even there it omits `/v1/batches` and `/v1/realtime/client_secrets`, both of which are documented in prose. It contains none of the `management-api.x.ai` surface (API keys, collections, audit). Path lists below are labeled by source accordingly.

---

## Resource model

```
                       CREDENTIAL PLANES
  ┌───────────────────────────┐  ┌──────────────────────────────┐
  │ management key            │  │ inference API key (xai-...)   │
  │ management-api.x.ai       │  │ api.x.ai / mtls.api.x.ai      │
  │ Console > Management Keys │  │ ACLs: api-key:endpoint:*      │
  │ (no documented ACLs)      │  │       api-key:model:*         │
  └────────────┬──────────────┘  └───────────┬──────────────────┘
               │                             │
   ┌───────────▼──────────┐      ┌───────────▼───────────────────────────┐
   │ /auth/teams/{teamId} │      │ POST /v1/responses                    │
   │   /api-keys          │      │   input, tools[], max_turns,          │
   │ /auth/api-keys/{id}  │      │   previous_response_id, store=true    │
   │   /rotate            │      │   model="grok-4.20-multi-agent"?      │
   │ /audit/teams/{id}    │      └───────────┬───────────────────────────┘
   │   /events            │                  │ returns id, stored 30 days
   │ /v1/collections      │                  ▼
   │   /{id}/documents    │      ┌───────────────────────────────────────┐
   └───────────┬──────────┘      │ response  (THE ONLY STATE OBJECT)     │
               │                 │  GET    /v1/responses/{response_id}   │
               │                 │  DELETE /v1/responses/{response_id}   │
               │                 │  GET    .../input_items  (paginated)  │
               │                 └───────────┬───────────────────────────┘
               │ collection_ids              │ previous_response_id
               │                             ▼  (linked list, not a session)
               │                 ┌───────────────────────────────────────┐
               └────────────────►│ POST /v1/documents/search (inference) │
                                 └───────────────────────────────────────┘

   NO Agent resource.  NO Session/Conversation resource.  NO Secret/Vault resource.
   NO Connector resource (connector_id explicitly unsupported).

   Adjacent stored artifacts:
     /v1/skills            zip of SKILL.md, default_version always "1"
     /v1/files             uploads, optional public URL
     /v1/batches           queued inference jobs (prose docs only)
     /v1/responses/compact returns an OPAQUE encrypted_content blob the CLIENT stores
```

### Actual path list from `https://docs.x.ai/openapi.json`

Verbatim, all 38 paths, sorted (`openapi: 3.1.0`, `info.title: "xAI's REST API"`, `info.version: "1.0.0"`, single security scheme `bearerAuth` = `http`/`bearer`, no `servers` block):

```
GET         /v1/api-key
POST        /v1/chat/completions
GET         /v1/chat/deferred-completion/{request_id}
POST        /v1/complete
POST        /v1/completions
POST        /v1/documents/search
GET         /v1/embedding-models
GET         /v1/embedding-models/{model_id}
POST        /v1/embeddings
GET,POST    /v1/files
DELETE,GET  /v1/files/{file_id}
GET         /v1/files/{file_id}/content
POST        /v1/files/{file_id}/public-url
POST        /v1/files/{file_id}/public-url/revoke
GET         /v1/image-generation-models
GET         /v1/image-generation-models/{model_id}
POST        /v1/images/edits
POST        /v1/images/generations
GET         /v1/language-models
GET         /v1/language-models/{model_id}
GET         /v1/me
POST        /v1/messages
GET         /v1/models
GET         /v1/models/{model_id}
POST        /v1/responses
POST        /v1/responses/compact
DELETE,GET  /v1/responses/{response_id}
GET         /v1/responses/{response_id}/input_items
GET,POST    /v1/skills
DELETE,GET  /v1/skills/{skill_id}
GET         /v1/skills/{skill_id}/content
POST        /v1/tokenize-text
GET         /v1/video-generation-models
GET         /v1/video-generation-models/{model_id}
POST        /v1/videos/edits
POST        /v1/videos/extensions
POST        /v1/videos/generations
GET         /v1/videos/{request_id}
```

`components.schemas` has **195** entries. Searching the whole spec: zero occurrences of `agent` as a JSON key or schema name (the string appears only inside prose descriptions, as "agentic"); zero occurrences of `session` or `conversation` as a key or schema name; four occurrences of `previous_response_id`. There is no `Agent`, `Session`, `Conversation`, `Thread`, `Secret`, `Vault`, `Connector`, or `Collection` schema.

### Paths documented in prose but absent from `openapi.json`

| Path | Host | Credential | Source |
|---|---|---|---|
| `POST /v1/realtime/client_secrets` | `api.x.ai` | inference API key | ephemeral-tokens |
| `POST /v1/batches`, `/v1/batches/{batch_id}`, `/v1/batches/{batch_id}/requests`, `/v1/batches/{batch_id}/results` | `api.x.ai` | inference API key | batch-api |
| `POST/GET /auth/teams/{teamId}/api-keys` | `management-api.x.ai` | management key | management/auth |
| `PUT /auth/api-keys/{api_key_id}` | `management-api.x.ai` | management key | management/auth |
| `POST /auth/api-keys/{apiKeyId}/rotate` | `management-api.x.ai` | management key | management/auth |
| `DELETE /auth/api-keys/{apiKeyId}` | `management-api.x.ai` | management key | management/auth |
| `GET /auth/api-keys/{apiKeyId}/propagation` | `management-api.x.ai` | management key | management/auth |
| `GET /auth/teams/{teamId}/models`, `/endpoints` | `management-api.x.ai` | management key | management/auth |
| `GET /auth/management-keys/validation` | `management-api.x.ai` | management key | management/auth |
| `GET /audit/teams/{teamId}/events` | `management-api.x.ai` | management key | management/audit |
| `POST/GET /v1/collections`, `GET/PUT/DELETE /v1/collections/{collection_id}`, `POST/GET/PATCH/DELETE /v1/collections/{collection_id}/documents[/{file_id}]`, `GET /v1/collections/{collection_id}/documents:batchGet` | `management-api.x.ai` | management key | collections/collection |

---

## Agents (or the absence of them)

### Verdict

There is no agent resource. Nothing is created, named, stored, versioned, activated, or deployed as an agent. Every agentic capability is a **parameter on a single inference request** and dies with that request.

### How agentic behavior is actually expressed

All of it is request-scoped fields on `POST /v1/responses` (`ModelRequest` schema):

| Field | Type | Verbatim description / behavior | Source |
|---|---|---|---|
| `tools` | array of `ModelTool` | The tool set for this request only. | openapi.json |
| `max_turns` | `integer \| null` (int32) | "Maximum number of agentic tool calling turns allowed for this request. If not set, defaults to the server's global cap. This parameter will be ignored for any non-agentic requests." | openapi.json |
| `parallel_tool_calls` | `boolean \| null`, default `true` | "Whether to allow the model to run parallel tool calls." | openapi.json |
| `tool_choice` | `ModelToolChoice` | `none` / `auto` / `required` / a specific tool. | openapi.json |
| `instructions` | `string \| null` | "An alternate way to specify the system prompt. Note that this cannot be used alongside `previous_response_id`, where the system prompt of the previous message will be used." | openapi.json |
| `reasoning` / `reasoning_effort` | object / `string \| null` | Effort levels; doubles as agent-count control on the multi-agent model. | openapi.json |
| `background` | `boolean \| null`, default `false` | "**(Unsupported)** Whether to process the response asynchronously in the background." | openapi.json |
| `metadata` | (untyped) | "**Not supported.** Only maintained for compatibility reasons." | openapi.json |
| `context_management` | `array \| null` | "Optional context-management directives (e.g. compaction). **Parsed but not yet executed.**" | openapi.json |

Note the last three: OpenAI's background-mode, metadata-tagging, and inline context-management fields are accepted for wire compatibility and **do nothing**. `background` being unsupported means there is no server-side long-running agent run object at all on the Responses API.

`ModelTool` is a `oneOf` over exactly these `type` values (verbatim enums from openapi.json):

`function`, `web_search`, `x_search`, `image_generation`, `file_search`, `code_interpreter`, `mcp`, `shell`, `tool_search`

Server-side execution is confirmed per type via `response.output[].type`:

| `output[].type` | Meaning | Source |
|---|---|---|
| `"function_call"` | "Client-side tool - requires local execution" | tool-usage-details |
| `"web_search_call"` | "Web-search tool - handled by xAI server" | tool-usage-details |
| `"x_search_call"` | "X-search tool - handled by xAI server" | tool-usage-details |
| `"code_interpreter_call"` | "Code-execution tool - handled by xAI server" | tool-usage-details |
| `"file_search_call"` | "Collections-search tool - handled by xAI server" | tool-usage-details |
| `"mcp_call"` | "MCP tool - handled by xAI server" | tool-usage-details |

Billing categories in `server_side_tool_usage` are verbatim: `SERVER_SIDE_TOOL_WEB_SEARCH`, `SERVER_SIDE_TOOL_IMAGE_SEARCH`, `SERVER_SIDE_TOOL_X_SEARCH`, `SERVER_SIDE_TOOL_CODE_EXECUTION`, `SERVER_SIDE_TOOL_VIEW_X_VIDEO`, `SERVER_SIDE_TOOL_VIEW_IMAGE`, `SERVER_SIDE_TOOL_COLLECTIONS_SEARCH`, `SERVER_SIDE_TOOL_MCP`. Only successful executions are billed; `tool_calls` lists every attempt including failures.

### What "multi agent" actually means

It is **a model name**, not an orchestration API. You set `model` to `grok-4.20-multi-agent`.

| Property | Verbatim | Source |
|---|---|---|
| Status | "This feature is currently in **beta**." | multi-agent |
| Supported models | `grok-4.20-multi-agent` (the only one listed) | multi-agent |
| Mechanism | "multiple agents are launched to discuss and collaborate on your query. Each agent contributes its own perspective, reasoning, and findings. A designated **leader agent** is responsible for synthesizing the discussion and presenting the final answer back to you." | multi-agent |
| Agent count | Exactly two setups: **4** or **16**. xAI SDK `agent_count` = `4` or `16`; REST / OpenAI SDK `reasoning.effort` = `"low"`/`"medium"` (4 agents) or `"high"`/`"xhigh"` (16 agents). | multi-agent |
| Addressability | None. "Only the **tool calls** and the **final response** from the leader agent are sent back to the user. All sub-agent state — including their intermediate reasoning, tool calls, and outputs — is encrypted and included in the response only when `use_encrypted_content` is set to `True` in the xAI SDK." | multi-agent |
| Cannot do | "**No client-side or custom tools:** Client-side tools (function calling) and custom tools are not currently supported by the multi-agent model variant." | multi-agent |
| Cannot do | "**Chat Completions API not supported**" | multi-agent |
| Cannot do | "`max_tokens` is not supported" | multi-agent |

So: sub-agents have no identity, no configuration, no per-agent tool assignment, no per-agent model, no per-agent prompt. The only knob is the count, and it takes exactly two values. This is a decoding/ensembling strategy sold as multi-agent, not an orchestration primitive.

### The closest thing to a stored agent definition: hosted Skills

`/v1/skills` is present in `openapi.json` with five operations. **No documentation page exists for it.** I probed five plausible URLs (`/developers/tools/skills.md`, `/developers/model-capabilities/text/skills.md`, `/developers/advanced-api-usage/skills.md`, `/developers/rest-api-reference/inference/skills.md`, `/developers/skills.md`); all returned `404 — Page not found`. Everything below is from the schema alone.

`Skill` schema, verbatim, required = `id, created_at, default_version, description, latest_version, name, object`:

| Field | Type | Verbatim description |
|---|---|---|
| `id` | string | "Unique identifier for the skill." |
| `object` | string | "The object type, which is always `\"skill\"`." |
| `name` | string | "Skill name extracted from `SKILL.md` frontmatter." |
| `description` | string | "Description extracted from `SKILL.md` frontmatter." |
| `created_at` | integer int64 | "Unix timestamp (seconds) for when the skill was created." |
| `default_version` | string | "Default version for the skill. **Currently always `\"1\"`.**" |
| `latest_version` | string | "Latest version for the skill. **Currently always `\"1\"`.**" |

`UploadSkillMultipartRequest`: one required field `files`, array of binary. "Skill zip file or one file from a directory upload. Clients may send this field once with a zip file, or repeatedly with directory files."

`DeletedSkill`: `{id, deleted, object}` where `object` is always `"skill.deleted"`.

`LocalShellSkill` (used inside the `shell` tool's `ShellEnvironment.skills[]`, a different thing): required `name, description, path`; `path` is "The path to the directory containing the skill (with a SKILL.md file)": i.e. it names a directory on the **caller's** machine, not on xAI's.

`ShellEnvironment`: required `type`; "The type of the environment. Currently only `local` is supported." Plus the optional `skills` array. There is **no** field for environment variables, secrets, or credentials on the shell environment.

Assessment: Skills carry a version field that is hardcoded to `"1"`, so there is no working versioning. There is no activation, no revision, no draft/published distinction, no link from a skill to a response or to a tool policy documented anywhere. A skill is an uploaded zip with a name and a description. It is a behavior artifact, not an agent.

---

## Session and conversation state

### Verdict

There is no session resource and no conversation resource. The stored unit is an individual **response**. Continuity is a client-supplied backward pointer, `previous_response_id`, forming a linked list of responses. Conversation identity is emergent, not modeled: there is no ID that names the conversation as a whole, no way to list the responses in one, and no way to delete one as a unit.

### Server-side storage: yes, on by default, 30 days

| Fact | Verbatim | Source |
|---|---|---|
| Storage default | `store`: "Whether to store the input message(s) and model response for later retrieval." `"default": true` | openapi.json `ModelRequest` |
| Retention | "The response ID can be used to retrieve the response later or to continue the conversation without repeating prior context. **New responses will be stored for 30 days and then permanently deleted.**" | responses |
| Chaining | `previous_response_id`: "The ID of the previous response from the model." | openapi.json, responses |
| Conflict rule | `instructions` "cannot be used alongside `previous_response_id`, where the system prompt of the previous message will be used." | openapi.json |
| Read back | `GET /v1/responses/{response_id}`: "Retrieve a previously generated response." | openapi.json |
| Delete | `DELETE /v1/responses/{response_id}` → `{id, object: "response", deleted: true}` | responses |
| Enumerate inputs | `GET /v1/responses/{response_id}/input_items`: "List input items for a previously generated response." | responses |

### The only cursor: `input_items` pagination

This is the single paginated read of stored state, and it is scoped to **one response**, not to a conversation:

| Parameter | Verbatim |
|---|---|
| `limit` (query) | "Maximum number of items to return (1-100, default 20)." |
| `order` (query) | `"asc" \| "desc"`: "Sort order: asc or desc. Default asc." |
| `after` (query) | "Cursor for pagination. Returns items after this item ID." |

Response body: `data` (array), `first_id` (`string \| null`), `has_more` (boolean), `last_id` (`string \| null`), `object` (always `"list"`). Note the docs page shows the response example as literally `{}`, so I could not verify the shape of an individual `data[]` entry from the published examples.

This is **not** a durable event log. It does not stream, does not replay across the conversation, and offers no sequence number or version that would let a consumer resume from a known point. There is no webhook, no SSE replay, and no listing endpoint for responses (`GET /v1/responses` does not exist).

### Conversation identity

| Candidate | What it actually is | Verdict as conversation ID |
|---|---|---|
| `response.id` | e.g. `"ad5663da-63e6-86c6-e0be-ff15effa8357"`; identifies one turn | No, it is a turn ID |
| `previous_response_id` | backward pointer only | No, it is an edge, not a node |
| `prompt_cache_key` | "Plumbed to `x-grok-conv-id` for Open Responses compatibility, **used for routing**." | No. It is a cache/routing hint. The name of the header it maps to (`x-grok-conv-id`) is the only place the word "conv" appears, and the documented purpose is routing, not identity |
| `cmp_<uuid>` compaction ID | identifies one compaction artifact | No |

`prompt_cache_key` is worth calling out because it is the nearest miss: xAI internally has a conversation-id header concept, but the public field is explicitly described as a routing lever, not a resource key, and nothing can be fetched by it.

### Context compaction: client owns the result

| Fact | Verbatim | Source |
|---|---|---|
| Endpoint | `POST /v1/responses/compact`: "Compacts a full Responses API input window into a shorter canonical window." | responses |
| Object | `"object": "response.compaction"` | context-compaction |
| ID format | "Stable ID for this compaction (`cmp_<uuid>`). Also echoed on the inner compaction item." Example: `"cmp_01HZ9P0V8M2YQK3F7C4G6N5R2A"` | context-compaction |
| Output | "An array containing a **single** compaction item. Pass it verbatim into your next request." `output[].type` always `"compaction"` | context-compaction |
| Ownership | "Treat `encrypted_content` as **opaque** — do not parse or modify it. **You can store the blob in your own database** and pass it back unchanged in later requests; it is only meaningful when sent back to xAI's API." | context-compaction |
| Usage field | `usage.dropped_message_count`: "Number of input messages folded into the compaction." | context-compaction |
| Constraint | "The conversation you compact must already fit in context." / "At most one compaction per call." / "Re-compacting is fine." | context-compaction |
| Constraint | "**Do not prune the compaction output.** Treat the returned compaction item as the new 'start' of the conversation — append new user turns after it, never before. Removing or reordering items inside the compacted output breaks the chain." | context-compaction |

This is the sharpest signal on xAI's stance: compaction hands the caller an encrypted blob and tells them to put it in their own database. Ownership of long-lived conversation state is deliberately pushed to the client. The inline `context_management` field that would have let the server do it is "parsed but not yet executed."

### Deferred completions and Batch: neither is a session

**Deferred chat completions** (Chat Completions API only, not Responses):

| Fact | Verbatim | Source |
|---|---|---|
| Trigger | `"deferred": true` on `POST /v1/chat/completions`; returns `{"request_id": "..."}` | deferred-chat-completions |
| Retrieve | `GET /v1/chat/deferred-completion/{request_id}` | deferred-chat-completions |
| Not-ready status | "When the completion result is not ready, the request will return **`202 Accepted`** with an empty response body." | deferred-chat-completions |
| Lifetime | "The result would be available to be requested **exactly once within 24 hours**, after which it would be discarded." | deferred-chat-completions |

Read-exactly-once, 24-hour TTL: this is a one-shot mailbox, the opposite of a durable log.

**Batch API** (prose docs only, not in `openapi.json`): `/v1/batches`, `/v1/batches/{batch_id}`, `/v1/batches/{batch_id}/requests`, `/v1/batches/{batch_id}/results`. Batch-level counters `num_requests`, `num_pending`, `num_success`, `num_error`, `num_cancelled`. Individual request states, verbatim: `pending`, `succeeded`, `failed`, `cancelled`. "Batches have an expiration time after which results are no longer accessible—check the `expires_at` field." Server-side tools work in batch; client-side function tools return `tool_calls` and "Multi-turn tool calling requires submitting a new batch request with the tool result messages included in the conversation." Limits: 2 batch creations per second per team, 25MB max per request payload, 1000 add-batch-request calls per 30 seconds per team, media signed URLs expire after 1 hour.

**Async** (`/developers/advanced-api-usage/async`) is purely client-side concurrency: `AsyncClient` from `xai_sdk` or `AsyncOpenAI`, an `asyncio.Semaphore`, and `max_concurrent`. It implies nothing about server-side lifecycle.

### The client-side product, for contrast

The `grok` CLI (docs.x.ai/build) has everything the API lacks, on disk:

| Feature | Verbatim | Source |
|---|---|---|
| Session storage | "Grok saves every conversation to disk automatically — prompts, responses, tool calls, and file snapshots — **under `~/.grok/sessions/`, keyed by working directory**." | build/features/sessions |
| Resume | `grok --resume <session-id>`, `grok --resume`, `grok -c`; `-s, --session-id` "names a new session with a UUID you supply; it does not resume existing ones" | build/features/sessions |
| Fork | `/fork [directive]` "branches the current session into a peer that starts from a copy of the conversation"; `--fork-session`, `--worktree`/`--no-worktree` | build/features/sessions |
| Rewind | "`/rewind` (or `Esc Esc` while idle) lists a rewind point per prompt. Selecting one restores all files to their state at that point and truncates the conversation to match." | build/features/sessions |
| Compact | `/compact [context]`; "Grok also auto-compacts as the context window fills" | build/features/sessions |
| Todos | statuses `pending`, `in progress`, `completed`, `cancelled`; "The list is part of the session" | build/features/sessions |
| Lifecycle | `/sessions`, `/rename <title>`, `grok sessions list`, `grok sessions search <query>`, `grok sessions delete <id>`, `grok export <id> [file]` | build/features/sessions |
| Subagents | "independent child sessions with their own context. They return a summary to the parent when finished." Built-in types: `general-purpose`, `explore`, `plan`. "Add or override types under `.grok/agents/` or `~/.grok/agents/`." Personas under `.grok/personas/*.toml` are "behavioral overlays only (tone, focus, contracts)" | build/features/subagents |

This is a real session model with fork, rewind and resume, and a real agent-definition model (`.grok/agents/` directory). **None of it is exposed as an API resource.** It is files on the developer's laptop. The `grok` CLI's subagent types are also completely disjoint from the API's `grok-4.20-multi-agent` sub-agents.

---

## Secrets and credentials

### Verdict

Four credential types, all for authenticating **to** xAI. Zero mechanisms for storing a credential that xAI uses on your behalf to reach a third party. There is no vault, no connector, no secret reference. MCP authorization tokens are transmitted in the request body on every single call.

### The four credential types

| Type | Minted by | Scope | Lifetime | Can do |
|---|---|---|---|---|
| **Inference API key** (`xai-...`) | `POST /auth/teams/{teamId}/api-keys` (management key required), or Console | Team-bound, user-attributed. Access is via ACL strings `api-key:endpoint:[endpoint name]` and `api-key:model:[model name]`, wildcards `api-key:endpoint:*` / `api-key:model:*`. "By default API keys don't have access to anything." | `expireTime` optional. "If set and in the past, the key is rejected." No default expiry documented. | Call `https://api.x.ai` as HTTP Bearer. Per-key throttles `qps`, `qpm`, `tpm` |
| **Management key** | Console only: "Settings -> Management Keys" | Team management on `https://management-api.x.ai`. **No ACL system documented for management keys themselves** | Not documented | Create/list/update/rotate/delete API keys, list team models and endpoints, read audit events, manage Collections |
| **Ephemeral token** | `POST https://api.x.ai/v1/realtime/client_secrets` using an inference API key | "The ephemeral token gives the holder **scoped access to resources**." Documented use is Speech-to-Speech WebSocket only. Actual scope not enumerated | Caller-set: `{"expires_after": {"seconds": 300}}` in the example. "Does not support `session` or `expires_after.anchor` fields" | Authenticate `wss://api.x.ai/v1/realtime`. "can be used in the same fashion as an API key." Browser variant: prefix `xai-client-secret.` on the `sec-websocket-protocol` header |
| **mTLS client certificate** | Out of band: email support@x.ai with team ID, CA cert in PEM, and the client cert CN | **Team level.** "All API keys in your team share the same mTLS configuration" | Cert-native. Renewing same CA + same CN requires no action; changing CA requires contacting support | Nothing on its own. "You still need a valid API key on every request. mTLS is an **additional** layer of security, not a replacement" |

mTLS specifics: endpoint is `https://mtls.api.x.ai` instead of `https://api.x.ai`; all paths identical. Two checks, both must pass: certificate verification failure → **`403 Forbidden`**; API key failure → **`401 Unauthorized`**. X.509 PEM only. Global endpoint only; regional mTLS requires contacting support.

API key rotation is the one genuinely well-designed piece:

| Field on `POST /auth/api-keys/{apiKeyId}/rotate` | Verbatim |
|---|---|
| (summary) | "!!CAUTION!! Rotates the secret of an existing API key, permanently invalidating the old one." |
| `expireTime` | "If set, updates the expiration time of the new API key secret. If not set, the new secret inherits the old secret's expiration time." |
| `oldSecretExpireTime` | "The time at which the old secret stops being accepted. **Defaults to 24 hours from now** if not set for non-expired keys. **Must not be more than 7 days from now.**" |

The key ID is stable across rotation, so the overlap window is on the secret, not the identity. There is also `GET /auth/api-keys/{apiKeyId}/propagation`: "There could be a slight delay between creating an API key, and the API key being available for use across all clusters."

`ApiKey` schema (inference-side, `GET /v1/api-key`) required fields, verbatim: `redacted_api_key`, `user_id`, `name`, `create_time`, `modify_time`, `modified_by`, `team_id`, `acls`, `api_key_id`, `team_blocked`, `api_key_blocked`, `api_key_disabled`. Note three separate block/disable flags: team-level block, key-level block (by xAI), and key-level disable (by the user).

`GET /v1/me` returns `user_id`, `team_id`, `zdr_status`, `team_blocked`, plus exactly one of `api_key` (`MeApiKeyInfo`: `api_key_id`, `redacted_api_key`, `blocked`, `disabled`) or `oauth` (`MeOAuthInfo`: `client_id` only). "Works with both API keys and OAuth tokens."

**OAuth is a fifth, barely-documented credential type.** `MeOAuthInfo` proves xAI accepts OAuth bearer tokens on the inference API and can identify the issuing `client_id`. I found no documentation page describing how to register an OAuth client, what scopes exist, or how tokens are obtained. Treat as: exists, undocumented.

`ZdrStatus` enum, verbatim: `no_zdr`, `zdr`, `pii_scrubbing`. `GetMeResponse` notes: "Historical `\"pii_scrubbing\"` is accepted on read but no longer emitted."

### The vault problem: xAI has no answer

This is the decisive finding. For a remote MCP server, the token travels **in the request body, on every request**:

| `mcp` tool parameter | Required | Verbatim description | Source |
|---|---|---|---|
| `server_url` | Yes | "The URL of the MCP server to connect to. **Only Streaming HTTP and SSE transports are supported.**" | remote-mcp |
| `server_label` | Yes | "A label to identify the server (used for tool call prefixing)" | remote-mcp |
| `server_description` | No | "A description of what the server provides" | remote-mcp |
| `allowed_tools` | No | "List of specific tool names to allow (empty allows all). The xAI native SDK uses the parameter name `allowed_tool_names`." | remote-mcp |
| **`authorization`** | No | **"A token that will be set in the Authorization header on requests to the MCP server"** | remote-mcp |
| **`headers`** | No | **"Additional headers to include in requests. The xAI native SDK uses the parameter name `extra_headers`."** (openapi type: `object \| null` with `additionalProperties: string`) | remote-mcp, openapi.json |
| `require_approval` | n/a | Present in openapi.json as `string \| null`. **"The `require_approval` and `connector_id` parameters in the OpenAI Responses API are not currently supported."** | remote-mcp |
| `connector_id` | n/a | Present in openapi.json as `string \| null`. **Not supported** (same note) | remote-mcp |
| `defer_loading` | No | "When true, this server's tool definitions are hidden from the model's prompt but stay callable, loaded via a `tool_search` step." | openapi.json |

So, answering the key question directly: **the token comes from the caller and the caller holds it.** There is no place to register an MCP server, no stored server definition, no credential reference, and no OAuth-on-your-behalf flow. `connector_id`: the field that in OpenAI's design points at a stored, provider-held connection: exists in xAI's schema purely for wire compatibility and is explicitly rejected. The same is true of `require_approval`: xAI has no human-in-the-loop approval gate for MCP calls.

Consequences worth stating plainly:
- The third-party secret is sent to xAI on every request, and sits in whatever request logging exists on both sides.
- Because `store` defaults to `true` and responses are retained 30 days, a request carrying an MCP `authorization` token is stored server-side for 30 days unless the caller sets `store: false`. Whether the stored copy redacts `tools[].authorization` is **not documented**. I could not verify this.
- Rotating a third-party token means changing the caller's own code or config, not a platform operation.
- There is no scoping between MCP servers: `allowed_tools` limits which tools the model may call, but every configured server gets exactly the headers the caller attached to it.

By contrast, the client-side `grok` CLI does have credential handling, again on disk: "Grok expands `${VAR}` (and `${VAR:-default}`) in `url`, `command`, `args`, `env`, and `headers`, so secrets can stay in the environment. Servers that require OAuth trigger a browser flow on first use; **tokens are stored under `~/.grok/mcp_credentials.json`**." That is a local credential file, not a service.

The CLI also has the permission and isolation model the API lacks. Permission modes: Ask (default), Auto, Always-approve; rules like `{ action = "allow", tool = "bash", pattern = "git *" }` and `{ action = "deny", tool = "bash", pattern = "rm -rf *" }`, with "`deny` always wins over `allow`". Sandbox profiles `off`, `workspace`, `devbox`, `read-only`, `strict` (Landlock on Linux, Seatbelt on macOS), off by default, with the explicit caveat "Built-ins do not permanently protect paths such as `~/.ssh`; use a custom `deny` list." None of this exists on the API side.

---

## Collections: the one durable, server-side state primitive

Collections are the only place xAI stores caller data as a named, mutable, server-side resource with its own lifecycle. Notably they live on the **management** host, not the inference host, and read and write use different credentials.

| Operation | Host | Credential |
|---|---|---|
| `POST /v1/collections`, `GET /v1/collections`, `GET/PUT/DELETE /v1/collections/{collection_id}` | `management-api.x.ai` | `$XAI_MANAGEMENT_API_KEY` |
| `POST /v1/collections/{collection_id}/documents/{file_id}`, `GET /v1/collections/{collection_id}/documents`, `GET/PATCH/DELETE /v1/collections/{collection_id}/documents/{file_id}`, `GET /v1/collections/{collection_id}/documents:batchGet` | `management-api.x.ai` | `$XAI_MANAGEMENT_API_KEY` |
| `POST /v1/documents/search` | `api.x.ai` | `$XAI_API_KEY` |

Core concepts, verbatim: "**File** — A single entity of a user-uploaded file." / "**Collection** — A group of files linked together, with an embedding index for efficient retrieval." / "A single file can belong to multiple collections."

`field_definitions` options, verbatim:

| Option | Verbatim description |
|---|---|
| `required` | "Document uploads must include this field. Defaults to `false`." |
| `unique` | "Only one document in the collection can have a given value for this field. Defaults to `false`." |
| `inject_into_chunk` | "Prepends this field's value to every embedding chunk, improving retrieval by providing context. Defaults to `false`." |

Search filtering uses **AIP-160** syntax with operators `=`, `!=`, `<`, `>`, `<=`, `>=`, `AND`, `OR`. "`AND` has higher precedence than `OR`." "Wildcard matching (e.g., `author=\"E*\"`) is not supported. All string comparisons are exact matches." Max file size 100MB. "We do not use user data stored on Collections for model training purposes."

Collections reach the model through the `file_search` tool, whose parameter is named `vector_store_ids` (OpenAI's name) but whose example values are `["collection_id_1", "collection_id_2"]`, max 10. The tool's `filters` and `ranking_options` are "For OpenAI API compatibility ONLY. Request will be rejected if this field is set."

### Audit log

`GET /audit/teams/{teamId}/events` on `management-api.x.ai`. "Audit events track changes to team settings, API keys, team membership, and other administrative actions."

Event shape, verbatim: `eventTime`, `eventId` ("Identifier to reference this log. **Not bound to anything else in the system.**"), `description` ("Free form description of the event **in English**"), `user` (`userId`, `email` "May not always populated", `profileImage`, `givenName`, `familyName`, `profileImageUrl`), plus top-level `nextPageToken`. Query params: `pageSize`, `pageToken`, `eventFilter.userId`, `eventFilter.query` (full-text over descriptions), `eventFilter.eventId`, `eventTimeFrom`, `eventTimeTo`, `orderBy` (`TIME_ASCENDING` | `TIME_DESCENDING`).

This is a **control-plane** audit log only. It is human-prose, has no structured event type, no typed payload, and explicitly no foreign key into the rest of the system. It records "API key 'Production Key' was created". It does not record inference, tool invocations, MCP calls, or data access. There is no data-plane audit trail of what an agentic request did.

---

## Identity and versioning

| Thing | ID form | Stability | Versioned? |
|---|---|---|---|
| Response | UUID, e.g. `"ad5663da-63e6-86c6-e0be-ff15effa8357"` | 30 days, then permanently deleted | No |
| Output message inside a response | `"msg_" + <response uuid>` | Tied to response | No |
| Compaction | `cmp_<uuid>`, e.g. `"cmp_01HZ9P0V8M2YQK3F7C4G6N5R2A"` (ULID-shaped) | "Stable ID for this compaction." Blob stored by the client, so lifetime is the client's | No |
| Deferred completion | `request_id` UUID | 24 hours, readable exactly once | No |
| Skill | opaque `id` | Not documented | `default_version` and `latest_version` both **always `"1"`** |
| API key | `apiKeyId` UUID | Stable **across secret rotation** | Secret rotates; identity does not |
| Batch | `batch_id` | Until `expires_at` | No |
| Collection | `collection_id` | Until deleted | No |
| Team / User | UUID | Stable | n/a |
| Audit event | `eventId` UUID, "Not bound to anything else in the system" | n/a | No |

There is no versioning anywhere on the platform in the sense of an immutable, reviewable, activatable revision. The one field that gestures at it, `Skill.default_version` / `latest_version`, is documented as a constant. Nothing is content-addressed, nothing has a digest, and nothing has a draft-versus-active distinction.

Model identity is worth noting as the one real version handle: `model` accepts `"latest"` (it is the schema's `example`, and appears in a real response body as `"model": "latest"`) alongside pinned names like `grok-4.6`, `grok-4.20`, `grok-4.20-multi-agent`. `system_fingerprint` (e.g. `"fp_44e53da025"`) appears on chat completion responses but not on the Responses API example.

---

## Notable design decisions

1. **OpenAI wire compatibility is the organizing principle, and xAI is unusually honest about where it is a facade.** The schema is littered with fields that exist only to accept OpenAI-shaped requests: `metadata` ("Not supported. Only maintained for compatibility reasons"), `background` ("(Unsupported)"), `context_management` ("Parsed but not yet executed"), `logprobs` ("silently ignored" on grok-4.20+), and a whole class that **reject the request** if set: `external_web_access`, `search_context_size`, `user_location`, `file_search.filters`, `file_search.ranking_options`, `code_interpreter.container`. Three distinct failure modes (ignored, ignored-with-note, rejected) for compatibility fields is a real interop hazard.

2. **State is stored but not modeled.** Responses persist by default for 30 days, yet there is no list endpoint, no conversation object, no way to enumerate a chain, and no retention control. You can only walk backward from an ID you already kept. Storage without an index is closer to a cache than to a system of record.

3. **Long-lived context ownership is explicitly pushed to the client.** Compaction returns an opaque encrypted blob with the instruction to store it in your own database. The server-side alternative (`context_management`) is stubbed. This is a deliberate choice: xAI holds the ciphertext key, the customer holds the ciphertext.

4. **"Multi-agent" is a model, not an architecture.** One model name, two agent counts, no per-agent configuration, sub-agents encrypted and non-addressable, and client-side function calling disabled entirely on that variant. Framing an ensemble as a multi-agent system sets an expectation the API cannot meet.

5. **The real agent product is a CLI, and it is deliberately local.** Sessions keyed by working directory under `~/.grok/sessions/`, agent definitions in `.grok/agents/`, MCP OAuth tokens in `~/.grok/mcp_credentials.json`, sandbox profiles enforced by Landlock/Seatbelt. Everything a managed agent platform would centralize, xAI keeps on the developer's machine. There is no stated intent to promote any of it to a server-side resource.

6. **No vault, and the compatibility shim makes it look like there might be.** `connector_id` sits in the published schema and is explicitly unsupported. A reader skimming the OpenAPI would reasonably conclude xAI has stored connections. It does not. Third-party secrets ride in the request body on every call.

7. **Security controls are strong at the edge, absent in the middle.** mTLS, per-key ACLs, `qps`/`qpm`/`tpm` throttles, rotation with a bounded overlap window (24h default, 7d max), cross-cluster propagation checks, ZDR status. All of it governs who may call xAI. Nothing governs what an agentic request may then do: no tool-level permissions, no MCP approval gate (`require_approval` unsupported), no egress policy, and no data-plane audit.

8. **Two hosts, two credentials, one logical feature.** Collections are written on `management-api.x.ai` with a management key and read on `api.x.ai` with an inference key. A workload that ingests and queries its own knowledge base must hold the more powerful credential, which is the same credential that can mint and delete API keys.

9. **Hosted Skills shipped without documentation.** Five operations in the public OpenAPI, a `SKILL.md` frontmatter convention, and no prose page. Either very new or not yet a supported product.

---

## Gaps and open questions

Things I could **not** verify from the fetched documentation:

1. **Whether stored responses redact `tools[].authorization`.** This is the most consequential unknown. `store` defaults to `true` and retention is 30 days, so MCP bearer tokens plausibly persist server-side. Not documented either way.
2. **Hosted Skills semantics entirely.** How a skill is attached to a request, whether the model can invoke one, what `GET /v1/skills/{skill_id}/content` returns, the `id` format, and any relationship to the `shell` tool's `LocalShellSkill`. No documentation page exists (five URLs probed, all 404).
3. **Ephemeral token scope.** "gives the holder scoped access to resources" is the entirety of the scope documentation. Whether it is limited to `/v1/realtime`, whether it inherits the parent key's ACLs, and the max `expires_after.seconds` are all undocumented.
4. **OAuth.** `MeOAuthInfo.client_id` proves OAuth bearer tokens are accepted on the inference API. No client registration, scope model, or token flow is documented anywhere I fetched.
5. **Management key permissions.** API keys have a full ACL system; management keys appear to have none. Whether a management key can be scoped to, say, audit-read-only is not documented. The Console mentions a "`Management Keys` Read + Write permission" at the user level, which is a different thing.
6. **`input_items[]` element shape.** The docs page prints the response example as literally `{}`.
7. **`GET /v1/responses/{response_id}` response fields.** Listed as a path with a summary in `openapi.json`; the prose page's example is `{}` for several endpoints.
8. **Whether `store: false` disables `previous_response_id` chaining.** Logically it must, but this is not stated.
9. **`defer_loading` / `tool_search` semantics.** Only the one-line schema description; no prose page found for the `tool_search` tool type.
10. **gRPC surface.** `https://docs.x.ai/developers/grpc-api-reference/auth.md` returned 200 but the body is 22 bytes: `#### gRPC API` / `# Auth`, with no content. The batch page links to `/developers/grpc-api-reference/batches`, so a gRPC API exists, but its auth model is undocumented at that URL.
11. **`/v1/messages`** appears in `openapi.json` with `MessageRequest`/`MessageResponse`/`MessageTools` schemas (an Anthropic-Messages-compatible surface). I did not fetch documentation for it, and it is out of scope for the three axes, but it is a third compatibility layer worth noting.
12. **Regional endpoints** are referenced in the mTLS FAQ ("Can I use regional endpoints with mTLS?") but I did not fetch the page describing them, so data-residency implications for stored responses are unverified.

---

## Sources

Every URL fetched, all via `curl -sS -A 'Mozilla/5.0'` with `.md` appended. Status noted where not 200.

Machine-readable spec:
- https://docs.x.ai/openapi.json

Inference, conversation state, agentic behavior:
- https://docs.x.ai/developers/rest-api-reference/inference/responses.md
- https://docs.x.ai/developers/model-capabilities/text/multi-agent.md
- https://docs.x.ai/developers/model-capabilities/text/comparison.md
- https://docs.x.ai/developers/model-capabilities/text/generate-text.md
- https://docs.x.ai/developers/advanced-api-usage/context-compaction.md
- https://docs.x.ai/developers/advanced-api-usage/deferred-chat-completions.md
- https://docs.x.ai/developers/advanced-api-usage/async.md
- https://docs.x.ai/developers/advanced-api-usage/batch-api.md (fetched additionally, referenced from async.md)

Tools:
- https://docs.x.ai/developers/tools/overview.md
- https://docs.x.ai/developers/tools/remote-mcp.md
- https://docs.x.ai/developers/tools/tool-usage-details.md

Auth, secrets, identity:
- https://docs.x.ai/developers/rest-api-reference/management/auth.md
- https://docs.x.ai/developers/management-api-guide.md
- https://docs.x.ai/developers/rest-api-reference/management.md
- https://docs.x.ai/developers/rest-api-reference/management/audit.md
- https://docs.x.ai/developers/model-capabilities/audio/ephemeral-tokens.md
- https://docs.x.ai/developers/advanced-api-usage/mtls.md
- https://docs.x.ai/developers/grpc-api-reference/auth.md (200, but 22-byte empty stub)

State-adjacent resources:
- https://docs.x.ai/developers/files/collections.md
- https://docs.x.ai/developers/files/collections/api.md
- https://docs.x.ai/developers/files/collections/metadata.md
- https://docs.x.ai/developers/rest-api-reference/collections/collection.md

Client-side coding agent product, for contrast:
- https://docs.x.ai/build/features/sessions.md
- https://docs.x.ai/build/features/subagents.md
- https://docs.x.ai/build/features/mcp-servers.md
- https://docs.x.ai/build/features/permissions.md
- https://docs.x.ai/build/features/sandbox.md

Probed and confirmed non-existent (all returned `404 — Page not found`), establishing that hosted Skills is undocumented:
- https://docs.x.ai/developers/tools/skills.md
- https://docs.x.ai/developers/model-capabilities/text/skills.md
- https://docs.x.ai/developers/advanced-api-usage/skills.md
- https://docs.x.ai/developers/rest-api-reference/inference/skills.md
- https://docs.x.ai/developers/skills.md
