# LiteLLM: Sessions, Agents, Secrets

Part of the [provider agent contracts research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-10-13. Every claim is sourced inline to a
vendor documentation page. LiteLLM is a different kind of subject from the
other dossiers in this corpus: it is not a model provider and not an agent
host but an open-source Python SDK plus a self-hosted gateway (the "proxy")
that puts one contract in front of many providers. This dossier therefore
answers the RESEARCH_PROMPT questions and also records that normalised
provider contract, because the normalisation is the product. The
documentation is versioned only weakly, so the authoritative anchors are the
pinned entry points below:

- LiteLLM [documentation](https://docs.litellm.ai/docs/), its machine index
  [`llms.txt`](https://docs.litellm.ai/llms.txt), and the markdown twin of
  each page, which the index describes as: "Every docs page is available as
  markdown by appending `.md` to its URL". All quotes below are from those
  markdown twins.
- Documentation source: the repository
  [`BerriAI/litellm-docs`](https://github.com/BerriAI/litellm-docs) at commit
  `799a4065c6799679da1fdcf5186cd62354106f86`, fetched on the retrieval date
  and spot-checked against the site twins.
- Version pin: weak. Every page twin carries `last_updated: "2026-10-12"` in
  its frontmatter, and pages name the proxy releases a behaviour arrived in
  (for example v1.85.0, v1.92.0, v1.97.0, v1.104.0), but no page states which
  release the documentation as a whole describes. The
  [API Stability Policy](https://docs.litellm.ai/docs/api_stability_policy)
  makes the running proxy's own `/openapi.json` the public contract, and that
  spec is generated per deployment rather than published at a docs URL.

LiteLLM does **not** have a managed agent platform in the sense the other
dossiers use. Its durable server-side nouns are **deployments** (entries of
`model_list`, each identified by a `model_id` that is by default a
deterministic hash of the deployment's `litellm_params`), **model groups**
(deployments sharing a `model_name`), **named provider credentials**
(`credential_name`, permanent, with rotatable values that carry the
destination `api_base` as well as the `api_key`), **virtual keys**, **teams**
and **projects**, and a registry of **A2A agents** that are endpoints LiteLLM
proxies to rather than agents it runs. Its contract is the OpenAI Chat
Completions shape: the index states "Responses always use the OpenAI Chat
Completions shape." On top of that it serves `/responses` and Anthropic
`/v1/messages`, bridging each to whatever the target provider natively speaks.
There is **no server-side session resource**: conversation continuity is
either the provider's own (`previous_response_id` passed through and pinned to
one deployment), or reconstructed from spend logs when the operator turns
prompt storage on, or a client-side `State` blob in the beta Python agent
harness. There is **no vault resource of its own**: provider secrets live in
config, environment variables, a credentials table encrypted with
`LITELLM_SALT_KEY`, or an external secret manager (an Enterprise feature).

Caveat on evidence: LiteLLM's public HTTP contract is defined by a spec that
each running proxy generates for itself (`/openapi.json`), not by a published
document, so path lists below come from prose pages and are labelled
accordingly. The documentation is also prose-heavy and contradicts itself in
places; contradictions are recorded with both pages cited and are not
resolved here.

---

## Resource model

```
                          CONTROL PLANE (admin key, Postgres)
  ┌──────────────────────────┐  ┌──────────────────────────┐  ┌───────────────────┐
  │ credential               │  │ deployment               │  │ virtual key       │
  │ credential_name (fixed)  │◄─┤ model_name (group)       │  │ team / project    │
  │ credential_values        │  │ litellm_params           │  │ model_config      │
  │  {api_base, api_key,..}  │  │  model: provider/model   │──┤  (per-team or     │
  │ POST/PATCH /credentials  │  │  litellm_credential_name │  │   per-project     │
  └──────────────────────────┘  │ model_info.id = model_id │  │   credential      │
                                │  (hash of litellm_params)│  │   routing)        │
                                └────────────┬─────────────┘  └─────────┬─────────┘
                                             │ model group              │
                                             ▼                          ▼
                          DATA PLANE (virtual key, OpenAI-shaped)
  ┌────────────────────────────────────────────────────────────────────────────┐
  │ POST /v1/chat/completions   POST /v1/responses   POST /v1/messages         │
  │   router: pre-call checks (session_affinity, encrypted_content_affinity,   │
  │   responses_api_deployment_check) -> strategy -> retries -> fallbacks      │
  │   -> translate to provider wire format -> translate back                   │
  └────────────────────────────────────────────────────────────────────────────┘
       │                                   │
       ▼                                   ▼
  response object (OpenAI shape)      response id (encrypted with the caller's
  + _hidden_params (SDK only)          identity; GET / DELETE / cancel)

  AGENT SURFACES (proxied, not hosted)
     /v1/agents          registry of A2A agents (UUID agent_id, agent_name)
     /a2a/{agent_id}     JSON-RPC 2.0 to the registered agent URL
     /v1beta/agents      Google AI Studio managed agents, passthrough only

  NO Session resource.  NO Vault resource of its own.  NO stored agent definition
  that LiteLLM executes.
```

### Paths documented in prose

There is no published spec to list paths from. The
[API Stability Policy](https://docs.litellm.ai/docs/api_stability_policy)
says "Every endpoint that appears in the proxy's OpenAPI spec is public" and
"Any endpoint not in the OpenAPI spec is private", and the spec "is generated
from the running proxy, so it reflects exactly the version you deploy." The
paths below are therefore taken from prose and examples, by page:

| Path | Purpose | Source |
|---|---|---|
| `POST /v1/chat/completions` | Normalised completion | [agent resources](https://docs.litellm.ai/docs/agent_resources) |
| `POST /v1/responses`, `GET` and `DELETE /v1/responses/{response_id}`, `POST /v1/responses/{response_id}/cancel` | OpenAI Responses shape, bridged where the provider has none | [/responses](https://docs.litellm.ai/docs/response_api) |
| `/responses/compact` | Compaction, `openai` only | [/responses/compact](https://docs.litellm.ai/docs/response_api_compact) |
| `POST /v1/messages` | Anthropic Messages shape for every provider | [/v1/messages](https://docs.litellm.ai/docs/anthropic_unified/) |
| `GET /v1/models` | Model groups the key can reach | [agent resources](https://docs.litellm.ai/docs/agent_resources) |
| `POST /model/new`, `GET /model/info`, `POST /model/update`, `POST /model/delete` | Deployment management | [model management](https://docs.litellm.ai/docs/proxy/model_management) |
| `POST /credentials`, `PATCH /credentials/{credential_name}`, `GET /credentials` | Named provider credentials | [configs](https://docs.litellm.ai/docs/proxy/configs), [credential routing](https://docs.litellm.ai/docs/proxy/credential_routing) |
| `POST /config/update`, `GET /router/settings` | Runtime router settings | [routing](https://docs.litellm.ai/docs/routing) |
| `POST` and `GET /v1/agents` | A2A agent registry and search | [A2A](https://docs.litellm.ai/docs/a2a) |
| `POST /a2a/{agent_id}`, `/a2a/{agent_id}/message/send`, `/v1/a2a/{agent_id}/message/send`, `GET /a2a/{agent_id}/.well-known/agent-card.json` | A2A invocation and discovery | [A2A](https://docs.litellm.ai/docs/a2a) |
| `/v1beta/agents`, `/v1beta/agents/{name}/versions`, `/v1beta/interactions` | Google managed agents, passthrough | [managed agents](https://docs.litellm.ai/docs/managed_agents) |

The model management page states the deletion route is not RESTful: "This is
a POST; there is no DELETE-verb route".

### Containment

Our inference from the pages above: a **model group** contains
**deployments**; a deployment references at most one **credential** by name
through `litellm_credential_name`; **teams** and **projects** can override
which credential a deployment uses for their traffic; a **virtual key**
belongs to a user, team or project and is the data-plane identity. A
**response** is owned by the caller whose identity encrypted its id. A
registered **agent** is a pointer to an external endpoint plus
`litellm_params`. Nothing contains a conversation.

---

## Agents (or the absence of them)

### Verdict

LiteLLM uses the word agent for several things, and none of them is an agent
definition that LiteLLM stores and executes the way the hosted platforms in
this corpus do.

1. **Registered A2A agents** (`/v1/agents`, `/a2a/{agent_id}`). The
   [A2A page](https://docs.litellm.ai/docs/a2a) describes the feature as: "Add
   A2A Agents on LiteLLM AI Gateway, Invoke agents in A2A Protocol, track
   request/response logs in LiteLLM Logs." Registration records a name, the
   agent card parameters and the URL of an agent that runs elsewhere; the
   supported back ends listed are "A2A, Vertex AI Agent Engine, LangGraph,
   Azure AI Foundry, Bedrock AgentCore, Pydantic AI". LiteLLM proxies,
   authorises, logs and budgets the calls.
2. **Google AI Studio managed agents** passed through at `/v1beta/agents`.
   The [managed agents page](https://docs.litellm.ai/docs/managed_agents) is
   explicit: "LiteLLM does **not** store the agent in its own database. The
   agent lives entirely on Google's side. LiteLLM is just the auth + routing
   layer."
3. **`litellm.agent()`**, a beta Python SDK harness. The
   [harness page](https://docs.litellm.ai/docs/harness) says
   "`litellm.agent()` is in beta. The API may change between releases." and
   that it drives "Claude Code, Codex, OpenCode, Deep Agents or Tool Loop from
   Python with one API." It is a client-side library, not a server resource.

### What a registered agent is

| Field | Verbatim | Source |
|---|---|---|
| `agent_id` | "`{agent_id}` may be the agent UUID or the registered agent name." | [A2A](https://docs.litellm.ai/docs/a2a) |
| `agent_name`, `agent_card_params` | "`agent_name` and `agent_card_params` are both required; entries missing either one are skipped at startup." | [A2A](https://docs.litellm.ai/docs/a2a) |
| `litellm_params` | "An optional `litellm_params` block carries per-agent settings such as `api_key`, `headers`, `agent_card_path`, and Microsoft Entra credentials" | [A2A](https://docs.litellm.ai/docs/a2a) |
| `agent_card_params.protocolVersion` | "LiteLLM serves that version on the proxied agent card and converts upstream responses to match." | [A2A](https://docs.litellm.ai/docs/a2a) |

An agent can be declared in `config.yaml` or created through the API or UI,
and the two sources collide by name with a stated rule: "the database record
wins and the config entry with that name is skipped, so the name always
resolves to the record you can edit." Config-declared agents are immutable at
runtime: "Agents declared in `config.yaml` are not stored in the database, so
they cannot be edited or deleted from the Admin UI."

### Stored, versioned, activated?

- **Stored**: yes for registered A2A agents (database or config), as a
  pointer plus parameters. No for managed agents (Google stores them).
- **Versioned**: a gap for registered A2A agents. No page documents a
  version, revision or history for an entry in `/v1/agents`. The only agent
  versions documented are Google's, read through the passthrough at
  `/v1beta/agents/{name}/versions`.
- **Activated or deployed**: a gap. No activation or deployment state is
  documented for a registered agent.

### Delegation

Delegation is call chaining across the gateway, not a parent and child
resource. When an agent calls back into LiteLLM, the A2A page says "LiteLLM
forwards two headers to maintain trace continuity", namely
`X-LiteLLM-Trace-Id` and `X-LiteLLM-Agent-Id`. Identity does not flow by
default: "The caller's **virtual key** and **end-user ID** are not
automatically forwarded." An agent can demand a trace id on inbound calls:
"Set `require_trace_id_on_calls_to_agent: true` in the agent's
`litellm_params`. When set, requests missing `x-litellm-trace-id` (or
`x-litellm-session-id`) are rejected with HTTP 400."

### Harness sessions are client-side

The [harness sessions page](https://docs.litellm.ai/docs/harness/sessions)
defines a session as something that "keeps its working directory and
conversation history alive between turns." Persistence across processes is a
blob the caller stores: "`State` holds the harness, its native session id, the
working directory and the model. It never holds credentials. `state.dumps()`
gives you bytes to store anywhere". We infer that the durable session lives in
the underlying harness (Claude Code, Codex and so on) and in whatever store
the caller picks, not in LiteLLM.

---

## Session and conversation state

### Verdict

No session resource exists in the proxy's documented contract. Probes for
`https://docs.litellm.ai/docs/sessions.md` and
`https://docs.litellm.ai/docs/proxy/sessions.md` returned 404 on the retrieval
date. What LiteLLM documents instead are several mechanisms that each carry part
of what a session would carry. The word "session" is used for all of them.

### Provider-held state passed through, pinned to one deployment

On `/v1/responses`, multi-turn state is the provider's own
`previous_response_id` chain. Because a model group can span deployments that
do not share state, LiteLLM pins follow-ups: the
[/responses page](https://docs.litellm.ai/docs/response_api) says session
continuity "ensures that follow-up requests using a `previous_response_id`
are routed to the same deployment that generated the original response."
This is a routing rule, enabled per proxy through `optional_pre_call_checks`
(`responses_api_deployment_check`), not a stored object.

Response ids are rewritten so the proxy can enforce ownership: "This is done
by encrypting the response ID with the user ID, enabling users to only access
their own response IDs." Ids the proxy did not mint are refused: "An ID in
any other shape, a raw provider ID or one handed out before response ID
encryption was on, carries no owner, so the proxy refuses it with 403 on
retrieve, cancel, delete, input items, and on `previous_response_id`".

### Encrypted reasoning pinned by rewriting item ids

The same page documents that encrypted reasoning items are only valid for the
API key that produced them, and that LiteLLM records the origin inside the
item id: "For each output item that contains `encrypted_content`, LiteLLM
rewrites the item ID to embed the originating `model_id`". When the origin
cannot serve a turn, the turn degrades rather than failing: "Without a peer
the turn is served in degraded form rather than failed." The
[reliability page](https://docs.litellm.ai/docs/proxy/reliability) states the
same rule for fallbacks: "The fallback drops what the target cannot
decrypt". We infer that LiteLLM treats provider-side encrypted state as
deployment-bound, and that a client cannot tell from the response body alone
that a turn lost its prior reasoning; the documented signal is a router
warning in the proxy's logs.

### Sticky routing keyed by a caller-supplied session id

The [routing page](https://docs.litellm.ai/docs/routing) documents
`session_affinity`: "Pin every request of a conversation to the deployment
that served its first request." The id is caller-chosen and arrives "from the
`x-litellm-session-id` header (`x-litellm-trace-id` is interchangeable), from
any `x-<vendor>-session-id` header such as `x-claude-code-session-id`, or from
`metadata.session_id` in the request body." The pin is scoped to the caller
on the proxy ("the virtual key, or the authenticated user id under JWT
auth"), refreshed per request ("`deployment_affinity_ttl_seconds` bounds the
idle time between turns, not the length of a conversation"), and kept in the
router cache ("With Redis configured they are shared across proxy instances;
without Redis each instance keeps its own."). Nothing about the conversation
content is stored by this mechanism.

### History reconstructed from spend logs

For providers without a native Responses API, `/responses` can rebuild
history from the proxy's own logs. The /responses page: "LiteLLM Proxy
supports session management for all supported models. This allows you to
store and fetch conversation history (state) in LiteLLM Proxy." It requires
the operator to store prompts: "Set `store_prompts_in_spend_logs: true` under
`general_settings` and `cold_storage_custom_logger: s3_v2` under
`litellm_settings`". We infer that the durable form of a conversation in this
mode is the spend-log row plus the object in cold storage, keyed by the
response id, and that its retention follows logging configuration rather
than a session lifecycle. No page documents ordering guarantees, cursors,
replay, branching or deletion for this reconstructed history: a gap.

### Other session-shaped surfaces

- WebSocket mode on `/v1/responses` has a connection lifetime, not a
  session object: "A WebSocket session on `/v1/responses` lasts at most 60
  minutes by default".
- Bedrock AgentCore A2A agents carry an AWS runtime session id derived from
  the A2A `contextId`: "LiteLLM prefixes `contextId` with a 16-character hash
  of the calling key before checking this"
  ([Bedrock AgentCore](https://docs.litellm.ai/docs/providers/bedrock_agentcore)).
  We infer the prefix is there so that two callers choosing the same
  `contextId` cannot share an upstream session.
- The harness `State` blob, covered under Agents above.

### Pins, ordering, lifecycle

| Question | Answer | Source |
|---|---|---|
| What does a conversation pin? | A deployment (`model_id`), through affinity checks. Not a model version, not a configuration, not a credential. | [routing](https://docs.litellm.ai/docs/routing), [/responses](https://docs.litellm.ai/docs/response_api) |
| Is there an ordered event log? | Gap. No sequence number or cursor is documented for any session-like surface. | |
| Replay? | Gap for the proxy. The harness offers `history()` on some harnesses only: "On the other harnesses `history()` raises `CapabilityUnsupported`." | [harness sessions](https://docs.litellm.ai/docs/harness/sessions) |
| Lifecycle | Harness only: `close()`, `detach()`, `stop()`. No proxy lifecycle. | [harness sessions](https://docs.litellm.ai/docs/harness/sessions) |

---

## Secrets and credentials

### Verdict

LiteLLM has no vault noun. It has **named provider credentials** stored in
its own database, environment-variable indirection in config, and adapters to
external secret managers. Probes for
`https://docs.litellm.ai/docs/vault.md`,
`https://docs.litellm.ai/docs/proxy/vault.md` and
`https://docs.litellm.ai/docs/proxy/credentials.md` returned 404. The secret
is used by the proxy process itself, which makes the outbound provider call;
in that sense the gateway is the egress point, and the caller's code never
holds the provider key (our inference from the architecture the pages
describe, not a stated guarantee).

### Where a provider secret can live

| Form | Verbatim | Source |
|---|---|---|
| Inline in `litellm_params` | `litellm_params.model` is "the model string passed to the litellm.completion() function"; `api_key` sits beside it | [configs](https://docs.litellm.ai/docs/proxy/configs) |
| Environment indirection | "**This works for ANY value on the config.yaml**" (the `os.environ/` prefix) | [configs](https://docs.litellm.ai/docs/proxy/configs) |
| Named credential | "`credential_name`: Unique identifier for the credential set" | [configs](https://docs.litellm.ai/docs/proxy/configs) |
| Database, encrypted | "Provider credentials for those models are encrypted at rest using `LITELLM_SALT_KEY` (falling back to `LITELLM_MASTER_KEY` if the salt key is not set)" | [model management](https://docs.litellm.ai/docs/proxy/model_management) |
| External secret manager | "LiteLLM supports **reading secrets (eg. `OPENAI_API_KEY`)** and **writing secrets (eg. Virtual Keys)** from Azure Key Vault, Google Secret Manager, Hashicorp Vault, CyberArk Conjur, and AWS Secret Manager." Enterprise only. | [secret managers](https://docs.litellm.ai/docs/secret_managers/overview) |
| Client-supplied | Per request `api_key` and, only after an operator opt-in, `api_base` | [clientside auth](https://docs.litellm.ai/docs/proxy/clientside_auth) |

The [encryption FAQ](https://docs.litellm.ai/docs/proxy/security_encryption_faq)
names the cipher: "**Algorithm:** NaCl SecretBox (XSalsa20-Poly1305 AEAD)".

Encryption coverage is partial. The
[OWASP mapping page](https://docs.litellm.ai/docs/proxy/security_owasp_llm_top10)
says: "Encryption at rest covers model `litellm_params`, environment
variables, and MCP credentials; guardrail `litellm_params` (which can hold a
provider API key), non-string values, and the `router_settings`,
`general_settings`, and `litellm_settings` sections are stored as plaintext".

Terminology contradiction, recorded and not resolved: the
[master key rotation page](https://docs.litellm.ai/docs/proxy/master_key_rotations)
says "the proxy signs stored data (model `litellm_params`, credentials, MCP
server credentials, DB-stored environment variables) with `LITELLM_SALT_KEY`",
while the model management and encryption FAQ pages describe the same
operation as encryption.

### A credential binds a destination, not just a token

A named credential's values carry the endpoint along with the key. The
[credential routing page](https://docs.litellm.ai/docs/proxy/credential_routing)
creates one with `credential_values` holding both `api_base` and `api_key`,
and states its purpose as: "Route the same model to different LLM provider
endpoints (e.g. different Azure instances) based on which team or project
makes the request." We infer that in LiteLLM the unit of credential is a
(destination, secret) pair, so swapping the credential can move traffic to a
different host.

Resolution is first-match-wins across a fixed chain: "When a request comes in,
the system walks this precedence chain (first match wins):". The chain, in
order, is clientside credentials in the request body, the project's
model-specific override, the project default, the team's model-specific
override, the team default, and the deployment's own `litellm_params`.

Client-supplied destinations are gated. The
[clientside auth page](https://docs.litellm.ai/docs/proxy/clientside_auth)
says that without per-provider `configurable_clientside_auth_params` (which
accepts a regex for `api_base`), "or `general_settings.allow_client_side_credentials: true`
proxy-wide, a request body that sets `api_base` or `base_url` is rejected
with `Rejected Request: api_base is not allowed in request body`".

On the `/v1/messages` native passthrough, a caller-supplied auth header wins
over the deployment's key: "LiteLLM sends `Authorization: Bearer <api_key>`
unless the request already carries an `Authorization` or `x-api-key` header"
([native passthrough](https://docs.litellm.ai/docs/anthropic_unified/native_passthrough)).

### Read-back

- Deployments: `GET /model/info` "Returns the full model list with API keys
  masked" ([model management](https://docs.litellm.ai/docs/proxy/model_management)).
- Registered agents: "Any credential-bearing field in `litellm_params`
  (`aws_access_key_id`, `aws_secret_access_key`, `aws_session_token`,
  `api_key`, and similar) is write-only. `GET`/`POST`/`PUT`/`PATCH /v1/agents`
  responses, and the Admin UI's agent edit form, always show these fields as a
  redacted placeholder rather than the stored value."
  ([Bedrock AgentCore](https://docs.litellm.ai/docs/providers/bedrock_agentcore)).
- Named credentials: `GET /credentials` returns "each credential's
  `display_name` and its `source`, either `db` or `config`". Whether
  `credential_values` are returned, masked or omitted is a gap.

Write-only is enforced by **redaction**: the field exists in the read schema
and carries a placeholder. That is the opposite technique to schema-shaped
write-only.

### Identity versus material, rotation, deletion

- Identity is fixed: "Models and vector stores reference a credential by
  `credential_name`, so it can't change after creation. A `PATCH` with a
  different `credential_name` returns 400"
  ([configs](https://docs.litellm.ai/docs/proxy/configs)). Relabelling uses a
  separate `display_name`.
- Config-sourced credentials are immutable through the API: "`PATCH` and
  `DELETE` on them return 405".
- Agent credential rotation: "To rotate a credential, submit the new value for
  that field; the update is applied immediately and, again, never echoed
  back." Clearing: "Submitting a field as an empty string clears the stored
  credential (for example, to fall back to the proxy's ambient AWS credential
  chain)."
- The encryption key cannot rotate: "Do not rotate `LITELLM_SALT_KEY` itself.
  It must not change after you have added a model; there is no in-place
  migration for salt-key rotation, so every stored credential would need to be
  re-registered."
- Deletion semantics for a named credential that deployments still reference
  are a gap. No archive state is documented for any credential.

### Plaintext in the agent process

For model calls, the provider secret stays in the proxy and the caller holds
only a virtual key (our inference, above). For registered A2A agents the
direction is reversed: LiteLLM holds the credential it presents **to** the
agent, and does not forward the caller's: "The caller's **virtual key** and
**end-user ID** are not automatically forwarded." Whether an agent hosted
behind LiteLLM can be given a provider secret by LiteLLM is not documented.

---

## The normalised provider contract

This section has no counterpart in the other dossiers. It records what
LiteLLM promises a caller regardless of which provider serves the call,
because that promise is the closest thing LiteLLM has to an agent-facing
contract.

### Model addressing: `provider/model`

A model string carries its provider as a prefix. The
[configs page](https://docs.litellm.ai/docs/proxy/configs) describes
`litellm_params.model` as "the model string passed to the litellm.completion()
function", and its examples annotate the prefix, for example "the `openai/`
prefix tells litellm it's openai compatible". The
[agent resources page](https://docs.litellm.ai/docs/agent_resources) tells
code generators to "prefix models with their provider (`openai/`,
`anthropic/`, `bedrock/`)." An explicit override exists:
`custom_llm_provider` is the "Provider identifier to use when resolving the
model and its API configuration"
([input](https://docs.litellm.ai/docs/completion/input)).

On the proxy, callers do not see this string. They send a `model_name` (a
model group), and the router picks a deployment whose `litellm_params.model`
holds the provider string. The prefix also selects wire protocol, not just
vendor: `openai/responses/<model>` routes a Chat Completions call through
the Responses API ([OpenAI Responses](https://docs.litellm.ai/docs/providers/openai/responses_api)),
and `openai/chat_completions/<model>` forces the reverse bridge
([/responses](https://docs.litellm.ai/docs/response_api)).

### Input

The [input page](https://docs.litellm.ai/docs/completion/input) states that
"LiteLLM accepts and translates the" OpenAI Chat Completion params "across all
providers." Unsupported parameters are an error by default: "By default,
LiteLLM raises an exception if the openai param being passed in isn't
supported." Some parameters bypass that check: "`stream_options`,
`extra_headers` and `max_retries` are not checked against this list and are
accepted for every provider". The SDK signature returns
`ModelResponse | CustomStreamWrapper`.

Switches that change the default:

| Switch | Effect | Source |
|---|---|---|
| `drop_params` | "This **ONLY DROPS UNSUPPORTED OPENAI PARAMS**." Settable per call, per deployment in `litellm_params`, or globally in `litellm_settings`. | [input](https://docs.litellm.ai/docs/completion/input), [drop_params](https://docs.litellm.ai/docs/completion/drop_params) |
| `additional_drop_params` | Removes named fields, including paths such as `tools[*].input_examples` | [drop_params](https://docs.litellm.ai/docs/completion/drop_params) |
| `allowed_openai_params` | "Use this if you get a `litellm.UnsupportedParamsError` and want to allow a param. LiteLLM will pass the param as is to the model." | [drop_params](https://docs.litellm.ai/docs/completion/drop_params) |

Provider-specific parameters pass through unvalidated: "LiteLLM treats any
non-openai param, as a provider-specific param, and passes it to the provider
in the request body, as a kwarg."
([provider-specific params](https://docs.litellm.ai/docs/completion/provider_specific_params)).
We infer an asymmetry worth naming: an OpenAI-named parameter the provider
lacks is rejected, while a parameter with an unknown name is forwarded.

### Tool calling

Tools use the OpenAI `tools` and `tool_choice` shape. The input page adds a
LiteLLM extension: a tool's `type` may be `function` or `mcp`, the latter
"(matching the `/responses` schema) to call LiteLLM-registered MCP servers
directly from `/chat/completions`." Capability is queryable per model through
`litellm.supports_function_calling` and
`litellm.supports_parallel_function_calling`. For models with no tool support, tools can be folded into the prompt: "For
Models/providers without function calling support, LiteLLM allows you to add
the function to the prompt set: `litellm.add_function_to_prompt = True`"
([function calling](https://docs.litellm.ai/docs/completion/function_call)).
The `/v1/messages` to Responses mapping documents per-field translation of
`tools` and `tool_choice` ("Format-translated" and "Type-remapped")
([mapping](https://docs.litellm.ai/docs/anthropic_unified/messages_to_responses_mapping)).

### Output and `_hidden_params`

The [output page](https://docs.litellm.ai/docs/completion/output) promises
"the exact json output and type you can expect from all litellm `completion`
calls for all models". Finish reasons are normalised with the original kept:
"LiteLLM maps all provider-specific `finish_reason` values to
OpenAI-compatible values (`stop`, `length`, `tool_calls`, `function_call`,
`content_filter`). When the original provider value differs from the mapped
value, it is preserved in" the `native_finish_reason` key of
`provider_specific_fields`.

`_hidden_params` is the SDK's side channel for facts that do not fit the
OpenAI shape. No page defines its schema: a gap. Keys documented by example
across pages include `response_cost`
([token usage](https://docs.litellm.ai/docs/completion/token_usage)),
`model_id` (the deployment that served the call,
[routing](https://docs.litellm.ai/docs/routing)) and a provider-specific
`thread_id` for Azure AI agents
([Azure AI agents](https://docs.litellm.ai/docs/providers/azure_ai_agents)).
On the proxy the same facts surface as response headers such as
`x-litellm-model-id` and `x-litellm-response-cost`
([response headers](https://docs.litellm.ai/docs/proxy/response_headers)).
We infer that the deployment identity of a response is out-of-band metadata,
never part of the body a caller would persist.

### Streaming

`stream=True` yields OpenAI-shaped chunks, and `stream_options` usage works
"for every provider" per the input page. LiteLLM guards against a looping
provider: "LiteLLM provides error handling for this, by checking if a chunk is
repeated 'n' times (Default is 100). If it exceeds that limit, it will raise a
`litellm.InternalServerError`, to allow retry logic to happen."
([streaming](https://docs.litellm.ai/docs/completion/stream)). A failure after
the first byte cannot change the status: "Failing after streaming has begun
cannot change the status code, which is already 200. The gateway emits the
error as a final SSE event carrying the same `error` object", and the
consequence is stated as a client duty: "Check every SSE payload for an
`error` key before treating it as a chunk."
([error reference](https://docs.litellm.ai/docs/proxy/error_reference)).

### Errors

"LiteLLM maps exceptions across all providers to their OpenAI counterparts."
and "All our exceptions inherit from OpenAI's exception types"
([exception mapping](https://docs.litellm.ai/docs/exception_mapping)). The
fallback class for anything unmapped is `APIConnectionError` with status 500:
"If any unmapped error is returned, we return this error". OpenAI and Azure
errors are not rewrapped: "For OpenAI and Azure we return the original
exception (since they're of the OpenAI Error type). But we add the
'llm_provider' attribute to them."

On the proxy, errors use the OpenAI `error` envelope, except the management
API: "The `/v1/management/*` endpoints are a separate API surface and return"
RFC 9457 Problem Details. Internal failures are deliberately opaque: "An
unhandled exception inside the gateway returns a deliberately minimal body
with no `param` or `code`". Layering is explicit: "The gateway wraps every
upstream failure and names both layers in `error.message`."
([error reference](https://docs.litellm.ai/docs/proxy/error_reference)).

Contradictions, recorded and not resolved:

- `ContextWindowExceededError`: the exception table on the
  [exception mapping page](https://docs.litellm.ai/docs/exception_mapping)
  lists it under `litellm.BadRequestError`, and the
  [error reference](https://docs.litellm.ai/docs/proxy/error_reference) says
  "Subclass of `BadRequestError`", while the exception mapping page's own
  prose says "The `ContextWindowExceededError` is a sub-class of
  `InvalidRequestError`."
- `BudgetExceededError`: the
  [exception mapping page](https://docs.litellm.ai/docs/exception_mapping)
  gives its status as `N/A`, while the
  [error reference](https://docs.litellm.ai/docs/proxy/error_reference) lists
  it under 429.

### Retries and fallbacks as contract

| Rule | Verbatim | Source |
|---|---|---|
| Retries before fallbacks | "If a call fails after num_retries, LiteLLM falls back to another model group" | [reliability](https://docs.litellm.ai/docs/proxy/reliability) |
| Backoff | "For RateLimitError we implement exponential backoffs" and "For generic errors, we retry immediately" | [routing](https://docs.litellm.ai/docs/routing) |
| Single retry owner | "LiteLLM owns retries and pins the provider client to `max_retries: 0`" | [routing](https://docs.litellm.ai/docs/routing) |
| Caller can override | `num_retries` precedence runs from the `x-litellm-num-retries` header, to the body, to the deployment, to `litellm_settings` | [routing](https://docs.litellm.ai/docs/routing) |
| Health is per deployment | "Cooldowns apply to individual deployments, not entire model groups." | [routing](https://docs.litellm.ai/docs/routing) |
| SDK fallbacks are one pass | "There is no time budget, no repeated loop over the list and no cooldown" | [reliable completions](https://docs.litellm.ai/docs/completion/reliable_completions) |
| Error of a failed chain | "**When the whole chain fails, the gateway re-raises the original exception.**" | [error reference](https://docs.litellm.ai/docs/proxy/error_reference) |
| Success hides the hop | "A fallback that *succeeds* is invisible in the response body" | [error reference](https://docs.litellm.ai/docs/proxy/error_reference) |
| Fallback loses encrypted state | "A fallback to a deployment on another provider, or on another API key, cannot decrypt the encrypted reasoning the failed deployment produced on earlier turns" | [reliability](https://docs.litellm.ai/docs/proxy/reliability) |

Fallback variants named on the reliability page are `fallbacks`,
`context_window_fallbacks` and `content_policy_fallbacks`.

### `model_list` and `litellm_params`

A deployment is a `model_list` entry with `model_name` (the group a caller
addresses), `litellm_params` (everything needed to call the provider, secret
included) and `model_info` (metadata). The routing page separates the two
maps by where they travel: "`litellm_params` is copied into the actual
request sent to the LLM provider, so a router-only setting placed there would
leak into that request." When `store_model_in_db` is on, the
[configs page](https://docs.litellm.ai/docs/proxy/configs) says the database
"becomes the source of truth for whatever they wrote".

### Bridging the Responses API to providers without one

Bridging runs in both directions and is selected by the model's default mode.
The [/responses page](https://docs.litellm.ai/docs/response_api): "Requests to
/chat/completions may be bridged here automatically when the provider lacks
support for that endpoint." In the other direction: "LiteLLM allows you to
call non-Responses API models via a bridge to LiteLLM's `/chat/completions`
endpoint. This is useful for calling Anthropic, Gemini and even non-Responses
API OpenAI models." Supported parameters differ by path: the page's table
lists "All Responses API parameters are supported" for `openai` and `azure`,
and links every other provider to the shared completion transformation.
Stored state for bridged providers comes from spend logs, described under
Session and conversation state above. `/responses/compact` has no bridge: its
supported providers are `openai` only.

`/v1/messages` bridges the same way. The
[native passthrough page](https://docs.litellm.ai/docs/anthropic_unified/native_passthrough)
says "`openai/` deployments go through the OpenAI Responses API" and
"everything else goes through `/v1/chat/completions`", and is candid about
loss: "That translation only keeps what the target API can express:
`cache_control` blocks are dropped, `thinking` is mapped to the provider's own
reasoning parameter, and other Anthropic-only request details are
approximated or lost". A deployment can opt out of translation with
`model_info.supported_endpoints`.

---

## Identity and versioning

| Thing | ID form | Stability | Versioned? |
|---|---|---|---|
| Deployment | `model_id`; "LiteLLM generates a unique `model_id` for each deployment by creating a deterministic hash of all the `litellm_params`." ([routing](https://docs.litellm.ai/docs/routing)). Examples on the same page also set `model_info.id` by hand. | Content-derived when generated: we infer that an edit to inline `litellm_params`, including a new `api_key`, yields a different `model_id` unless `model_info.id` is pinned (see Open questions). | No. `model_info` accepts arbitrary fields, which the model management page says can annotate "a version", but that is user metadata. |
| Model group | `model_name`, operator-chosen string | Stable as long as the operator keeps it | No |
| Named credential | `credential_name` | Permanent: a rename returns 400 | No. Values rotate in place; no history is documented. |
| Response | Proxy-issued id, encrypted with the caller's identity | Stable; ids not minted by this proxy are refused by default | No |
| Encrypted reasoning item | `encitem_` id embedding the origin `model_id` | Stable for the life of the item | No |
| Registered A2A agent | UUID `agent_id`, or `agent_name` | Name resolves database first, then config | Gap: none documented |
| Google managed agent | Google resource name | Google's | Google's own `/versions`, read through passthrough |
| Proxy HTTP API | `/openapi.json` of the running proxy | "Additive changes are not breaking and can ship in any release" | By proxy release; breaking changes appear in release notes |

The API stability policy defines breaking change as "A change that removes or
renames a field, changes a default, tightens who may call an endpoint, or
otherwise requires action to keep prior behavior".

---

## Notable design decisions

1. **One output shape for every provider.** "Responses always use the OpenAI
   Chat Completions shape." Everything that does not fit goes into
   `provider_specific_fields`, `_hidden_params` or response headers rather
   than into new top-level fields.
2. **Reject by default, opt into loss.** Unsupported OpenAI parameters raise
   until the operator turns on `drop_params`; unknown parameters are forwarded
   untouched. Loss of fidelity is an explicit operator choice per deployment.
3. **Translation loss is documented, not hidden.** The `/v1/messages`
   passthrough page lists what is dropped or approximated, and the fallback
   path drops encrypted reasoning a different key cannot decrypt.
4. **Deployment identity is a content hash of its call parameters.** Health,
   cooldown and affinity all key on that hash. Inline secrets sit inside the hashed
   map.
5. **Credential identity is a permanent name; credential material is a
   rotatable map that includes the destination.** A credential moves traffic
   as well as authenticating it.
6. **Resolution is first-match-wins, not fail-on-ambiguity.** The credential
   precedence chain and the client-header override on the native passthrough
   both pick a winner silently.
7. **One retry owner.** The router pins provider SDK retries to zero so that
   retry counts compose additively rather than multiplicatively.
8. **The first error wins.** A failed fallback chain reports the first model
   group's error, and a successful fallback is visible only in headers.
9. **Ownership is enforced by encrypting ids.** Response ids are made
   unforgeable per caller instead of being looked up in an ACL table; foreign
   ids are refused.
10. **Agents are routed, not hosted.** Every agent surface is a pointer to
    something running elsewhere, with LiteLLM adding auth, logging, budgets
    and trace headers.

---

## Impact on the combined schema proposal

What LiteLLM's normalisation confirms or changes in
[the combined schema proposal](../combined-schema-proposal.md). LiteLLM is a
model gateway rather than an agent host, so its evidence bears most directly
on the model provider plane (draft
[ADR#0032](../../../adr/0032-model-route-and-credential-binding.md)) and only
by analogy on the tool and channel connections that §5 identifies as the gap.

**Confirms**

- **§1 and §6.2, material rotates and structural identity does not.** A third
  independent design reaches the same rule: `credential_name` "can't change
  after creation", while values are replaced in place and agent credential
  fields rotate by submitting a new value. This adds a third source to what
  the proposal currently grounds on two vendors.
- **§6.1, no digest over material.** LiteLLM is the counterexample that
  shows why the rule matters. Its deployment `model_id` is a hash over
  `litellm_params`, which include the `api_key`; we infer that a key rotation
  can therefore change deployment identity, resetting cooldown state and
  breaking affinity pins keyed on the old id. Whether it does when the key
  comes through a named credential is unresolved (see Open questions). The
  proposal's choice to record
  a reference only for connections, and to keep material out of any digest,
  avoids exactly this coupling.
- **§6.3, destination binding matters.** A LiteLLM credential carries
  `api_base` alongside `api_key`, so the credential is a (destination,
  secret) pair. That places LiteLLM's named credentials toward the tight end
  of the spectrum for the destination, while its client-supplied
  credentials sit at the loose end, gated by an operator regex on `api_base`.
  The regex gate is prior art for a declaration-time destination allowlist.
- **§6.3 and V3, implicit resolution is a hazard.** LiteLLM resolves
  credentials by a first-match-wins chain that starts with the request body,
  and its native passthrough lets a caller's `Authorization` header replace
  the deployment key. Both are the late, silent resolution the proposal
  rejects. This supports keeping "Multiple matches fail admission" and
  rejecting caller-supplied sensitive headers at declaration time.
- **§6.4, the secret stays out of the caller's process.** For model calls the
  gateway holds the provider key and the caller holds only a virtual key. We
  infer this is a fifth design in which substitution happens at a proxy in
  the data path, which bears on V4: LiteLLM accepts the proxy as an
  architecture commitment and documents its own blast radius only partially
  (encryption at rest does not cover guardrail `litellm_params` or the
  settings sections).
- **[ADR#0032](../../../adr/0032-model-route-and-credential-binding.md), attempt-scoped model route grant.** Per-team and per-project
  credential routing (`model_config.defaultconfig` and per-model overrides) is
  working prior art for binding a model route to a credential by tenant
  without duplicating the route.

**Changes or sharpens**

- **§6.7, write-only by schema.** LiteLLM does the opposite: `/v1/agents`
  returns credential fields "as a redacted placeholder rather than the stored
  value", and `/model/info` returns keys "masked". This is a second
  data point for write-only by redaction, and the empty-string-clears rule
  shows the cost: a placeholder field in the read shape invites a write shape
  where sentinel values carry meaning. It strengthens the case for §6.7
  rather than changing it.
- **§6.2 and V2, encryption key as a non-rotatable root.** LiteLLM's
  `LITELLM_SALT_KEY` "must not change after you have added a model". The
  proposal's custody tiers ([ADR#0033](../../../adr/0033-key-custody-product-model.md)) should state how the key that protects
  stored credentials rotates, since a gateway that cannot rotate it is the
  documented failure mode.
- **A session pin that is not a pin.** The proposal pins at admission.
  LiteLLM's nearest equivalent pins a **deployment** per session id in a
  cache with an idle TTL, and keeps the pin through cooldowns. If [ADR#0032](../../../adr/0032-model-route-and-credential-binding.md)'s
  grant is attempt-scoped, the proposal should say whether an attempt inside
  one session may land on a different deployment than the previous attempt;
  LiteLLM's encrypted-reasoning rules show that the answer has
  correctness consequences, not only cost ones.
- **Fallback crosses credential boundaries.** LiteLLM documents that a
  fallback to "another API key" loses encrypted provider state and serves the
  turn in degraded form. A route grant that permits fallbacks implicitly
  grants use of more than one credential; the proposal does not yet say
  whether fallback targets must be named in the grant.
- **Router settings must not ride with the secret-bearing map.** LiteLLM
  warns that `litellm_params` is forwarded to the provider, so a router-only
  field placed there "would leak into that request". The proposal's split
  between connection declaration and credential grant should keep any
  field that is forwarded upstream separate from fields that configure the
  gateway, for the same reason.
- **§6.5, archive versus delete.** LiteLLM documents neither for credentials,
  and config-sourced credentials refuse `DELETE` with 405. It adds no evidence
  for the archive idiom and remains a single-source finding.
- **D4 and V5, delegation.** LiteLLM forwards trace and agent id headers to a
  called agent but not the caller's virtual key or end-user id. That is a
  "start empty" default for identity across a delegation hop, matching the
  proposal's recommended answer to V5.

---

## Open questions

1. **What does `/openapi.json` actually list?** The public contract is
   whatever a running proxy publishes, and no copy is published at a docs
   URL. Every path in this dossier comes from prose. Fetching the spec from a
   pinned release would turn the path table into a verifiable inventory and
   could surface endpoints the prose does not describe.
2. **Does a key rotation change `model_id`?** The routing page says the id is
   a hash of "all the `litellm_params`", and keys live in `litellm_params`.
   No page says whether a deployment that references a named credential
   through `litellm_credential_name` hashes the name or the resolved values,
   or whether database-stored deployments get a hashed id at all.
3. **What does `GET /credentials` return for `credential_values`?** Masked,
   omitted or plaintext is not stated.
4. **What happens to deployments when a referenced credential is deleted?**
   Not documented.
5. **Is the spend-log session history ordered, bounded or deletable?** The
   /responses page documents how to enable it, not its guarantees.
6. **Are registered A2A agents versioned?** No version, revision or history
   field is documented for `/v1/agents`.
7. **Which `_hidden_params` keys are stable?** Keys appear only in examples
   across provider pages, and the API stability policy covers the HTTP API,
   not SDK attributes.
8. **Which exception hierarchy is authoritative** for
   `ContextWindowExceededError`, and which status applies to
   `BudgetExceededError`? The pages disagree (see Errors).
9. **Is stored data encrypted or signed with `LITELLM_SALT_KEY`?** The master
   key rotation page says "signs"; the model management and encryption FAQ
   pages say encrypted. The FAQ names an AEAD cipher, from which we infer
   both are true, but no page reconciles the wording.

---

## Source anchors

Retrieved 2026-10-13 (`date +%F`). The pin is weak: every page twin carries
`last_updated: "2026-10-12"`, the documentation names no single release it
describes, and the authoritative HTTP contract (`/openapi.json`) is generated
by each running proxy rather than published. The documentation source
repository [`BerriAI/litellm-docs`](https://github.com/BerriAI/litellm-docs)
was read at commit `799a4065c6799679da1fdcf5186cd62354106f86`. Quotes are
transcribed from the site's markdown twins (`.md` appended to each URL
below) and spot-checked against the repository files at that commit.

Index and policy:

- https://docs.litellm.ai/llms.txt
- https://docs.litellm.ai/docs/api_stability_policy
- https://docs.litellm.ai/docs/agent_resources

Unified contract:

- https://docs.litellm.ai/docs/completion/input
- https://docs.litellm.ai/docs/completion/output
- https://docs.litellm.ai/docs/completion/drop_params
- https://docs.litellm.ai/docs/completion/provider_specific_params
- https://docs.litellm.ai/docs/completion/function_call
- https://docs.litellm.ai/docs/completion/stream
- https://docs.litellm.ai/docs/completion/token_usage
- https://docs.litellm.ai/docs/completion/reliable_completions
- https://docs.litellm.ai/docs/exception_mapping
- https://docs.litellm.ai/docs/proxy/error_reference
- https://docs.litellm.ai/docs/proxy/response_headers
- https://docs.litellm.ai/docs/response_api
- https://docs.litellm.ai/docs/response_api_compact
- https://docs.litellm.ai/docs/providers/openai/responses_api
- https://docs.litellm.ai/docs/anthropic_unified/
- https://docs.litellm.ai/docs/anthropic_unified/messages_to_responses_mapping
- https://docs.litellm.ai/docs/anthropic_unified/native_passthrough

Routing, reliability and configuration:

- https://docs.litellm.ai/docs/routing
- https://docs.litellm.ai/docs/proxy/reliability
- https://docs.litellm.ai/docs/proxy/configs
- https://docs.litellm.ai/docs/proxy/model_management
- https://docs.litellm.ai/docs/proxy/credential_routing
- https://docs.litellm.ai/docs/proxy/clientside_auth

Secrets:

- https://docs.litellm.ai/docs/proxy/security_encryption_faq
- https://docs.litellm.ai/docs/proxy/security_owasp_llm_top10
- https://docs.litellm.ai/docs/proxy/master_key_rotations
- https://docs.litellm.ai/docs/secret_managers/overview

Agents and sessions:

- https://docs.litellm.ai/docs/a2a
- https://docs.litellm.ai/docs/managed_agents
- https://docs.litellm.ai/docs/providers/bedrock_agentcore
- https://docs.litellm.ai/docs/providers/azure_ai_agents
- https://docs.litellm.ai/docs/harness
- https://docs.litellm.ai/docs/harness/sessions

Probed and absent (HTTP 404 on 2026-10-13):

- https://docs.litellm.ai/docs/sessions.md
- https://docs.litellm.ai/docs/proxy/sessions.md
- https://docs.litellm.ai/docs/agents.md
- https://docs.litellm.ai/docs/vault.md
- https://docs.litellm.ai/docs/proxy/vault.md
- https://docs.litellm.ai/docs/proxy/credentials.md
- https://docs.litellm.ai/docs/proxy/openapi.json
- The documentation's former location inside the main repository,
  `https://raw.githubusercontent.com/BerriAI/litellm/main/docs/my-website/docs/`,
  returned 404 for every page tried. The documentation now lives in
  `BerriAI/litellm-docs` under `docs/`.

Editorial rewrites inside quotes, disclosed:

- Quotes that would have spanned a markdown link are split at the link, and
  the link text is paraphrased outside the quote (for example the input
  page's "LiteLLM accepts and translates the" OpenAI Chat Completion params
  sentence, and the error reference's RFC 9457 sentence).
- Source sentences containing an em dash, such as the credential routing
  precedence chain, are paraphrased rather than quoted, so no quote in this
  dossier was altered to remove a dash.
- Relative links in quoted material are not reproduced; every link in this
  dossier points at an absolute `https://docs.litellm.ai/docs/` URL or at a
  page in this repository.
- Escaped quote characters in the input and output pages' code spans are not
  reproduced inside quotes; those passages are paraphrased.
