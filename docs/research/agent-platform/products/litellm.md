# LiteLLM: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from unversioned live documentation at docs.litellm.ai, fetched as
markdown by appending `.md` to each doc path (for example
`https://docs.litellm.ai/docs/a2a.md`), enumerated from the `llms.txt` page
index, with the GitHub release feed of `BerriAI/litellm` used only to date the
current release. LiteLLM is an open source AI gateway (a proxy server plus a
Python SDK) that fronts LLM providers, MCP servers, and A2A agents behind one
set of virtual keys. Retrieved 2026-10-13. The pin is weak; see
[Source anchors](#source-anchors).

## The `agent` noun (primary-source quotes)

- **The primary noun is a registered gateway record that points at an agent
  LiteLLM does not run.** The Agent Gateway page frames the whole feature as
  routing and access: "Add A2A Agents on LiteLLM AI Gateway, Invoke agents in
  A2A Protocol, track request/response logs in LiteLLM Logs. Manage which
  Teams, Keys can access which Agents onboarded"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)). Registration takes a
  name and a URL: "The URL should be the invocation URL for your A2A agent".
  The card page restates the value proposition as proxying: LiteLLM exposes
  A2A agents "to your clients through LiteLLM with virtual keys, team
  scoping, observability, and a unified agent card"
  ([A2A Agent Card](https://docs.litellm.ai/docs/a2a_agent_card)).
- **It is persistent and identified.** The record has an `agent_id` (a UUID)
  and an `agent_name`, is created through `POST /v1/agents` or the Admin UI
  under **Agentic** > **Agents**, and is addressable by either: "`{agent_id}`
  may be the agent UUID or the registered agent name"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)). The name is "A unique
  identifier for your agent (used in API calls)"
  ([A2A Agent Cost Tracking](https://docs.litellm.ai/docs/a2a_cost_tracking)).
  The storage table is named in the audit-log description: a kill switch fire
  "writes a `kill_switch_fired` row to the audit log against the
  `LiteLLM_AgentsTable` table with the agent id as the object"
  ([Agent Kill Switch](https://docs.litellm.ai/docs/a2a_kill_switch)).
- **The record carries no persona and no loop.** Every documented field is
  either the upstream's published card (`agent_card_params`: name,
  description, url, version, capabilities, skills, protocol version), upstream
  connection settings (`litellm_params`: provider, credentials, card path,
  trace-id flags, session budgets), header forwarding (`static_headers`,
  `extra_headers`), a `kill_switch` webhook, rate limits, and an
  `object_permission` block. No page documents a system prompt,
  instructions, tool list, or default model field on the gateway agent; we
  infer from that absence that the definition of what the agent *is* lives
  entirely upstream. Skill routing confirms it: "LiteLLM forwards the entire
  message envelope, including metadata, to the upstream agent unchanged. The
  upstream agent is responsible for reading `skillId` and routing internally"
  ([A2A Agent Card](https://docs.litellm.ai/docs/a2a_agent_card)).
- **The same record is a policy principal.** A virtual key can be bound to an
  agent (`agent_id` passed to `/key/generate`), and a JWT can be bound to one
  by claim: "Provision agent identities in your identity provider (for
  example, one Microsoft Entra ID app registration per agent) and let LiteLLM
  enforce the agent's policies on every JWT that agent presents"
  ([OIDC, JWT-based Auth](https://docs.litellm.ai/docs/proxy/token_auth)).
  The record is therefore both a callee (something clients invoke at
  `/a2a/{agent_id}`) and a caller identity (something whose outbound model
  and MCP calls are narrowed and attributed).
- **The word "agent" names other, unrelated things in the same product.**
  - *`litellm.agent()`*, an SDK function: "Run an agent with
    `litellm.agent()` to drive Claude Code, Codex, OpenCode, Deep Agents or
    Tool Loop from Python with one API." It is flagged
    "`litellm.agent()` is in beta. The API may change between releases"
    ([Agents (litellm.agent)](https://docs.litellm.ai/docs/harness)). Nothing
    on the harness pages references a registered gateway agent; attribution
    is by request tag (`harness,<name>`), not `agent_id`.
  - *Google managed agents*, a pass-through to another vendor's resource at
    `/v1beta/agents`: "The agent lives entirely on Google's side. LiteLLM is
    just the auth + routing layer"
    ([Google AI Studio, Managed Agents](https://docs.litellm.ai/docs/managed_agents)).
  - *Agents as model routes*: LangGraph, Bedrock Agents, Vertex AI Agent
    Engine, Azure AI Foundry Agents, and Databricks agents are reachable as
    `model` strings (for example the provider route `langgraph/{agent_id}`,
    [LangGraph](https://docs.litellm.ai/docs/providers/langgraph)), and any
    registered agent is reachable from `/chat/completions` with the `a2a/`
    prefix ([Invoking A2A Agents](https://docs.litellm.ai/docs/a2a_invoking_agents)).
- **Conceptual model (our inference): agent-as-policy-principal over a proxied
  endpoint.** The gateway agent is closest to the corpus's
  agent-as-network-endpoint (the A2A card), plus an identity that keys and
  tokens can be bound to. It is not agent-as-config in the corpus's usual
  sense, because the config it holds governs access to the agent, not the
  agent's behaviour.

## Subagents

- **No subagent noun on the gateway record.** The record has no field naming
  other agents, no roster, and no parent reference. The only documented
  sub-agent concept is a *calling convention*: "When the backend agent itself
  calls LiteLLM (for chat completions or to invoke a sub-agent), LiteLLM
  forwards two headers to maintain trace continuity", namely
  `X-LiteLLM-Trace-Id` and `X-LiteLLM-Agent-Id`
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)).
- **Delegation is the upstream agent's choice; the gateway only observes it.**
  An agent invoked through LiteLLM that calls another registered agent does so
  as an ordinary A2A client of the proxy. The propagation is cooperative: "To
  enable these features, your A2A server must **forward these headers** to
  any LLM calls it makes back to LiteLLM." What does not cross: "The caller's
  **virtual key** and **end-user ID** are not automatically forwarded"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)).
- **A sub-agent role can be enforced, not declared.** The
  `require_trace_id_on_calls_to_agent` flag exists for agents meant only to
  be called inside someone else's trace: "Use when the agent should only be
  called as a sub-agent with a trace context. Returns **400** if missing"
  ([Agent Iteration Budgets](https://docs.litellm.ai/docs/a2a_iteration_budgets)).
  The gateway cannot tell which agent is the parent; it only checks that a
  trace id is present.
- **Inheritance between agents is not modelled.** Each agent's permissions,
  limits, and credentials stand alone, and outbound headers are isolated:
  "Each agent invocation uses an isolated HTTP connection"
  ([A2A Agent Authentication Headers](https://docs.litellm.ai/docs/a2a_agent_headers)).
  No depth or fan-out limit is documented for agent-to-agent calls; the
  nearest bound is the per-session iteration and dollar cap, which counts LLM
  calls, not delegations.
- **The SDK harness passes subagents through to the wrapped runtime.** For
  `Harness.DEEPAGENTS`, `DeepAgentsOptions.subagents` holds "deepagents
  SubAgent dicts, passed through", and the page recommends "a cheaper group
  for subagents" ([Deep Agents](https://docs.litellm.ai/docs/harness/deepagents)).
  Subagent semantics are LangChain's, not LiteLLM's.

## Configuration surface (what, where, why)

- **Fields on the gateway agent**, grouped by what they govern (assembled from
  [Agent Gateway](https://docs.litellm.ai/docs/a2a),
  [A2A Agent Card](https://docs.litellm.ai/docs/a2a_agent_card),
  [A2A Agent Authentication Headers](https://docs.litellm.ai/docs/a2a_agent_headers),
  [Agent Kill Switch](https://docs.litellm.ai/docs/a2a_kill_switch),
  [Agent Iteration Budgets](https://docs.litellm.ai/docs/a2a_iteration_budgets),
  [Budgets, Rate Limits](https://docs.litellm.ai/docs/proxy/users), and
  [MCP Permission Management](https://docs.litellm.ai/docs/mcp_control)):
  - *Identity and discovery*: `agent_name`, generated `agent_id`, and
    `agent_card_params` (the A2A card LiteLLM republishes, including a pinned
    `protocolVersion` of `0.3` or `1.0`, curated `skills`, and a free-form
    `version` string).
  - *Upstream connection*: `agent_card_params.url` for A2A servers, or
    `litellm_params.custom_llm_provider` plus `model` and credentials for
    "completion-bridge" agents (LangGraph, Bedrock AgentCore, Databricks,
    and others); `agent_card_path` for non-standard card locations.
  - *Outbound credentials*: `static_headers`, `extra_headers` (client header
    names eligible for forwarding), provider credentials in `litellm_params`,
    and minted credentials (Databricks OAuth, Microsoft Entra).
  - *Resource limits*: `tpm_limit` and `rpm_limit` "on the agent itself",
    `session_tpm_limit` and `session_rpm_limit` per session, and
    `max_iterations` plus `max_budget_per_session` in `litellm_params`.
  - *Trace enforcement*: `require_trace_id_on_calls_to_agent` (inbound) and
    `require_trace_id_on_calls_by_agent` (outbound, required for session
    budgets).
  - *Outbound authority*: `object_permission` with `mcp_servers` and
    `mcp_tool_permissions`, which narrows what the agent's own calls can reach.
  - *Cost*: flat cost per query or per-token input and output cost, used to
    price calls *to* the agent
    ([A2A Agent Cost Tracking](https://docs.litellm.ai/docs/a2a_cost_tracking)).
  - *Operations*: a `kill_switch` webhook, and agent access group tags.
- **Absent knobs**, each a gap rather than a denial: system prompt,
  instructions, tool list, default model, memory, sandbox or environment,
  schedule, and triggers. No page states that these are out of scope; the
  configuration model simply has no place for them, because the upstream
  agent owns them.
- **Where configuration lives.** The Admin UI, the `/v1/agents` REST API
  (`POST`, `GET`, `PATCH`, `PUT`, plus `make_public` and `kill_switch`
  actions), and the top-level `agents:` key in `config.yaml`. The config path
  exists for immutable deployments: "Agents can also be declared in
  `config.yaml` under the top-level `agents` key, which is useful when the
  gateway is deployed from a ConfigMap or another read-only source"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)). Some settings are
  reachable only from one surface: "Tagging an agent with access groups is
  currently a dashboard-only operation"
  ([Agent Permission Management](https://docs.litellm.ai/docs/a2a_agent_permissions)).
- **Who may grant access to an agent sits on keys and teams, not on the
  agent.** "Agent Permission Management lets you restrict which agents a
  LiteLLM Virtual Key or Team can access", with stated rationale
  "**Multi-tenant environments**: Give different teams access to different
  agents", security, and compliance. The default is open: "Open access by
  default when no restrictions are set"
  ([Agent Permission Management](https://docs.litellm.ai/docs/a2a_agent_permissions)).
  Access Groups bundle agents with models and MCP servers: "One group can
  grant access to all three resource types"
  ([Access Groups](https://docs.litellm.ai/docs/proxy/access_groups)).
- **Stated rationale for the remaining knobs, quoted.**
  - Session budgets exist for cost: "When agents run agentic loops, they can
    make unbounded LLM calls, causing unexpected costs"
    ([Agent Iteration Budgets](https://docs.litellm.ai/docs/a2a_iteration_budgets)).
  - Agent-level MCP grants exist to bound autonomy independently of the
    caller: "Useful for capping what an autonomous agent can do regardless of
    which key originally invoked it"
    ([MCP Permission Management](https://docs.litellm.ai/docs/mcp_control)).
  - Static header precedence exists so callers cannot impersonate the gateway
    to the upstream: "This ensures admin-controlled credentials cannot be
    overridden by client requests"
    ([A2A Agent Authentication Headers](https://docs.litellm.ai/docs/a2a_agent_headers)).
  - The AI Hub exists for reuse: "Share pre-built agents (A2A spec) across
    your organization. Users can discover and use agents without rebuilding
    them" ([AI Hub](https://docs.litellm.ai/docs/proxy/ai_hub)).
  - JWT binding exists so an identity provider, not a LiteLLM key, can carry
    the agent's identity; the agent's policies then apply because "Everything
    that already applies to a virtual key bound to an agent then applies to
    the JWT caller too"
    ([OIDC, JWT-based Auth](https://docs.litellm.ai/docs/proxy/token_auth)).
- **Credential handling is not uniform across the record.** Provider
  credentials are hidden: "Any credential-bearing field in `litellm_params`
  (`aws_access_key_id`, `aws_secret_access_key`, `aws_session_token`,
  `api_key`, and similar) is write-only"
  ([Bedrock AgentCore](https://docs.litellm.ai/docs/providers/bedrock_agentcore)).
  Static headers are not: "`static_headers` values are stored in the
  database and returned by the API"
  ([A2A Agent Authentication Headers](https://docs.litellm.ai/docs/a2a_agent_headers)).
  Kill switch auth is redacted on read and visible only to proxy admins.
- **The SDK harness is configured per call, not per record.**
  `litellm.agent()` takes a `Harness` enum value, a prompt, a required
  `sandbox`, a `model`, and optional `tools`, `permissions`, `options`, and
  `metadata`; "Options you pass to `agent_session()` apply to every turn"
  ([Sessions](https://docs.litellm.ai/docs/harness/sessions)). Its stated
  rationale is outsourcing the loop: "With `litellm.completion` you get one
  model call and write the loop yourself. With a harness you get a finished
  loop that somebody else maintains"
  ([Agents (litellm.agent)](https://docs.litellm.ai/docs/harness)).

## Binding time

- **Registration time binds the card and the upstream.** LiteLLM fetches the
  upstream card once, the admin curates it, and "LiteLLM saves the curated
  card and serves it at" `/a2a/{agent_id}/.well-known/agent.json`
  ([A2A Agent Card](https://docs.litellm.ai/docs/a2a_agent_card)). The
  served card does not track the upstream automatically; an explicit
  operation rebinds it: "Use the **Re-sync from upstream** button to pick up
  new skills or capabilities the upstream agent has added since
  registration; it shows a diff and lets you accept changes selectively."
  Invalid protocol versions are rejected at registration with HTTP 400.
- **Database records are mutable at any time; config records only at
  restart.** `PATCH` and `PUT` update a database agent in place, including
  limits, headers, and kill switch. Config-declared agents "are not stored in
  the database, so they cannot be edited or deleted from the Admin UI. Change
  the config file and restart the gateway instead"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)). Name collisions
  resolve toward the database: "If an agent created in the UI already uses a
  name you then declare in `config.yaml`, the database record wins and the
  config entry with that name is skipped, so the name always resolves to the
  record you can edit."
- **Per-request binding is where the policy is evaluated.** Each call to
  `/a2a/{agent_id}` resolves the key and team allow-lists at request time;
  each outbound MCP call by a bound key intersects the agent's grants on
  every request ("the agent's list is intersected with the key's on every
  request the key makes",
  [MCP Permission Management](https://docs.litellm.ai/docs/mcp_control)).
  Changes to access groups apply to every holder at once. Unpinned protocol
  versions bind per request from client signals (method name or an
  `a2a-version` header).
- **Session state is bound by a caller-supplied id.** The "session" that
  budgets and session rate limits count is whatever the caller names: a
  "Dollar cap per session (identified by `x-litellm-trace-id`)", with the id
  also accepted as `metadata.session_id`
  ([Agent Iteration Budgets](https://docs.litellm.ai/docs/a2a_iteration_budgets)).
  Counters are ephemeral: "Counters expire after 1 hour by default".
- **Derived state is cached until the card changes.** "Agent embeddings are
  computed once per process and reused until the agent card changes"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)).
- **Versioning: none for the gateway record.** No page documents revisions,
  history, or pinning of the agent record. `agent_card_params.version` is the
  card's own free-form string, republished to clients. The pass-through to
  Google exposes Google's versions (`/v1beta/agents/{name}/versions`), which
  LiteLLM neither stores nor manages
  ([Google AI Studio, Managed Agents](https://docs.litellm.ai/docs/managed_agents)).
- **What happens to in-flight work on edit is undocumented.** Because the
  gateway does not run the loop, an edit can only change routing, limits,
  and grants applied to subsequent requests; we infer an in-flight upstream
  task is unaffected until its next call through the proxy, but no page
  states this.
- **SDK harness binding** is per call or per session: options bind at
  `agent_session()` creation, and resumable state "holds the harness, its
  native session id, the working directory and the model. It never holds
  credentials" ([Sessions](https://docs.litellm.ai/docs/harness/sessions)).

## Relationships between nouns

- **Agent to virtual key: N:M for invocation, 1:N for ownership.** Any number
  of keys may be allowed to *invoke* an agent through `object_permission`
  lists or agent access groups. Separately, a key may be *bound* to one agent
  (`agent_id` at `/key/generate`), which makes the key the agent's own
  credential for outbound calls; the iteration budget walkthrough ends with
  "Proceed to create a new key for the agent", and the gateway page calls it
  "a key owned by an agent"
  ([Agent Iteration Budgets](https://docs.litellm.ai/docs/a2a_iteration_budgets);
  [Agent Gateway](https://docs.litellm.ai/docs/a2a)). We infer an agent can
  hold several bound keys; no page caps it.
- **Agent to team: allow-list only.** "A2A permission resolution operates
  over two levels: Key and Team." A key with no list inherits the team's
  ("The key inherits agent permissions from the team"), and when both have
  lists the result is the "Intersection of both lists (most restrictive
  wins)" ([Agent Permission Management](https://docs.litellm.ai/docs/a2a_agent_permissions)).
  No page places an agent *inside* a team or organization as its owner; the
  record shows a `created_by` user.
- **Agent to organization: only through MCP.** Organizations do not appear in
  A2A invocation permissions. They appear in the MCP permission hierarchy,
  where the organization "Acts as a **ceiling**" over key, team, end user,
  agent, and internal user lists
  ([MCP Permission Management](https://docs.litellm.ai/docs/mcp_control)).
- **Agent to MCP server: the agent is a grantee, never a host.** Agents hold
  MCP server and per-tool grants in `object_permission`; those grants narrow
  the outbound calls of keys and JWTs bound to the agent, and of requests
  that name the agent in `x-litellm-agent-id`. "Granting a server to the key
  alone is not enough; the agent must also hold the grant". MCP clients can
  also *find* agents: a key with tool search enabled sees an `agent_search`
  virtual tool that ranks the registry
  ([MCP Tool Search](https://docs.litellm.ai/docs/mcp_tool_search)).
- **Agent to model: none on the A2A record, a route for bridge agents.** An
  A2A agent references no gateway model group; the models it calls are
  whatever its bound key may call. Completion-bridge agents carry a provider
  `model` in `litellm_params`. Conversely, every agent is reachable *as* a
  model via `model: a2a/<agent>`, and some agent platforms are configured as
  ordinary `model_list` entries. Search uses a configured embedding model
  (`agent_search_embedding_model`).
- **Agent to session: 1:N, but the session is an id, not a resource.** The
  gateway's session is a caller-supplied trace id used to key counters, with
  a default one-hour TTL. A2A tasks and contexts belong to the upstream
  agent: task methods are "forwarded to the upstream agent unchanged"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)).
- **Agent to memory: by naming convention only.** The `/memory` store is
  "CRUD endpoints for storing and retrieving user/team-scoped memory entries
  on the LiteLLM proxy", and suggests the key pattern `agent:{agent_id}:memory`
  under the rule "Keys are globally unique. Use namespaced keys to organize
  entries" ([/memory](https://docs.litellm.ai/docs/memory_management)). Memory
  is scoped to users and teams; the agent prefix is a convention the store
  does not enforce.
- **Agent to sandbox: none on the gateway.** The SDK harness alone has a
  sandbox noun, and there "A sandbox is required by every call"
  ([Sandboxes](https://docs.litellm.ai/docs/harness/sandboxes)). The
  harness `Session` ("a live runtime with its sandbox, working directory and
  history") never becomes a gateway record.
- **Agent to agent: no edge.** Calls between registered agents are invisible
  as relationships; they surface only as shared trace ids and per-agent spend.

## Lifecycle

- **Create**: Admin UI, `POST /v1/agents`, or a `config.yaml` entry read at
  startup. Config entries missing `agent_name` or `agent_card_params` are
  skipped: "`agent_name` and `agent_card_params` are both required; entries
  missing either one are skipped at startup"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)).
- **Publish**: optionally made public on the AI Hub (`make_public`), which
  lists the curated card at `/public/agent_hub`
  ([AI Hub](https://docs.litellm.ai/docs/proxy/ai_hub)).
- **Invoke**: `POST /a2a/{agent_id}` with JSON-RPC, or `/chat/completions`
  with `model: a2a/<agent>`. Only messaging runs through LiteLLM's own
  client: "`message/send` and `message/stream` go through LiteLLM's A2A
  client (logging, guardrails, spend). All other methods are forwarded to the
  upstream URL in `agent_card_params.url`"
  ([Agent Gateway](https://docs.litellm.ai/docs/a2a)).
- **Who owns the loop: never LiteLLM, on the gateway path.** The registered
  agent runs elsewhere, and the gateway cannot stop it except by asking. The
  kill switch is an outbound webhook to the upstream runtime: "LiteLLM only
  makes the webhook call; the agent stays registered and enabled on the
  LiteLLM side, so you decide separately whether to delete it." The UI warns
  "that firing stops the agent's upstream runtime and can cause an outage"
  ([Agent Kill Switch](https://docs.litellm.ai/docs/a2a_kill_switch)).
  Budgets and rate limits stop a loop indirectly, by answering its model
  calls with 429.
- **Pause and resume**: no gateway equivalent. The record has no running
  state to pause; disabling access means editing key or team lists, and
  stopping work means firing the kill switch.
- **Delete**: database agents are deleted through the UI or API; config
  agents only by editing the file and restarting. Deleting a database agent
  that shadows a config entry hands the name back: "Delete the database agent
  and the config entry takes the name back on the next reload."
- **Persists**: the record, its card, spend logs and Agent Usage analytics,
  and audit rows for kill switch fires. Session counters expire. Conversation
  history, tasks, and memory are the upstream agent's.
- **The SDK harness has its own, client-side lifecycle.** `litellm.agent()`
  opens a session for one turn and closes it; `agent_session()` keeps it
  open; sessions end with `close()`, `detach()` (resumable, sandbox left
  running), or `stop()` (resumable, runtime stopped), and resume across
  processes from serialized `State`
  ([Sessions](https://docs.litellm.ai/docs/harness/sessions)). The loop is
  the wrapped runtime's (Claude Code, Codex, OpenCode, Deep Agents) except for
  `Harness.TOOL_LOOP`, which is LiteLLM's own minimal loop around
  `litellm.acompletion()`. Setup is the caller's job: "Do any setup in your
  own code. There are no lifecycle hooks"
  ([Sandboxes](https://docs.litellm.ai/docs/harness/sandboxes)).
- **Server-side reruns exist, but not as an agent.** The Agentic Loop Hook
  page says to "Build a `CustomLogger` callback that intercepts a model
  response, fulfills tool calls server-side, and reruns the model, all
  transparently to the caller"
  ([Agentic Loop Hook](https://docs.litellm.ai/docs/proxy/agentic_loop_hook)),
  with a default of three reruns. It attaches to model requests, not to
  registered agents.

## What makes it "an agent" here (our inference)

Our inference: **in LiteLLM an agent is a registered, named gateway record
that points at someone else's A2A endpoint, republishes its curated card, and
serves as a policy principal that keys and tokens can be bound to, while the
loop it names always runs outside the gateway.** *Registered* traces to
"the registered agent name" and `POST /v1/agents`; *someone else's A2A
endpoint* to "The URL should be the invocation URL for your A2A agent" and
the forwarding of task methods upstream; *curated card* to "LiteLLM saves the
curated card and serves it at"; *policy principal* to key and JWT binding
("enforce the agent's policies on every JWT that agent presents"),
agent-level MCP grants, limits, and spend attribution; and *outside the
gateway* to the kill switch, which "stops the agent's upstream runtime" and
for which "LiteLLM only makes the webhook call".

The distinctive datum for the study is the split between identity and
definition. Every other product in the corpus that keeps an agent record keeps
something that defines or deploys the agent (a prompt, a model, tools, code,
or an image) and attaches authority to it. LiteLLM stores *what the agent may
do and spend* and nothing that determines how it behaves; the card it keeps is
a description the upstream published. The record is an authorization and
accounting subject, with persona and loop left wholly to the upstream. The
word is also overloaded inside one product: `litellm.agent()` is a client-side call
that drives third-party harnesses and never touches the record, and the
`/v1beta/agents` route forwards to Google's resource without storing it.
And the gateway's "session" is only a caller-asserted trace id with a
one-hour counter, so session-level budgets bound cost per trace rather than
per conversation.

## Open questions

- **Where session budgets are stored.** The same page says both "Budget
  controls (`max_iterations`, `max_budget_per_session`) are stored in the
  virtual key's metadata" and "Budget controls are set on the agent's
  `litellm_params` (not on individual keys), so they apply across all keys
  for the agent"
  ([Agent Iteration Budgets](https://docs.litellm.ai/docs/a2a_iteration_budgets)).
  [Budgets, Rate Limits](https://docs.litellm.ai/docs/proxy/users) sides with
  the agent's `litellm_params`. Recorded as an upstream contradiction, not
  resolved.
- **Who may assert `x-litellm-agent-id`.** MCP grants for "the agent
  identified by `x-litellm-agent-id`" are intersection-only, so asserting the
  header can only narrow tool access. The same header "Attributes spend to
  the correct agent". No page states whether an arbitrary key can attribute
  its spend to, or consume the session budget of, an agent it is not bound
  to.
- **Whether `max_iterations` and session budgets apply to A2A invocations or
  only to the bound key's outbound LLM calls.** The prose says "Each LLM call
  for a session increments a counter", and enforcement requires the outbound
  trace flag; the treatment of `message/send` calls *to* the agent is not
  stated.
- **Ownership of the record.** The API response carries `created_by`, but no
  page says whether a team admin (rather than a proxy admin) can create,
  edit, or delete an agent, or whether agents are scoped to a team or
  organization at all.
- **Several keys bound to one agent.** Key binding is documented one key at a
  time; whether limits such as `tpm_limit` aggregate across all bound keys is
  implied by "on the agent itself" but never shown.
- **Org-level control over A2A access.** Organizations bound MCP access as a
  ceiling but do not appear in A2A permission resolution; whether an
  organization can restrict agent invocation is undocumented.
- **Edits during in-flight tasks.** No page states what happens to a running
  upstream task, or to pending `tasks/get` polls, when the agent's URL,
  credentials, or protocol version change mid-task.
- **Version history of the record.** No revision or audit trail of card or
  policy edits is documented beyond kill switch fires; whether general audit
  logging covers `PATCH /v1/agents` is not stated.
- **The `agents:` config key's release status.** The gateway page says "The
  `agents` key is read correctly starting in the next release (after
  `v1.95.0`)", while the current release at retrieval is `v1.105.0`; the note
  reads as stale, and the release that fixed it is not named.
- **Whether `litellm.agent()` will ever bind to a registered agent.** The
  harness pages attribute spend by tag and never mention `agent_id`. The SDK
  is "in beta," so the separation may be deliberate or merely early.

## Source anchors

The docs are unversioned live pages. Every page fetched carries a
`last_updated: "2026-10-12"` field in its markdown frontmatter, and none
states the LiteLLM version it describes. The current GitHub release at
retrieval was `v1.105.0`, published 2026-10-11
(`https://github.com/BerriAI/litellm/releases/tag/v1.105.0`). Feature-floor
statements on individual pages give more version signals: the AI Hub notes
"Agents are only available in v1.79.4-stable and above", and the gateway page
references `v1.95.0` for the `agents:` config key. The pin is therefore weak:
retrieval date 2026-10-13, page `last_updated` 2026-10-12, and the release
current on that date. The site publishes no archive URL and no content digest,
and many of these pages have no counterpart at the same path in the
repository's `docs/my-website/docs/` tree, so the served `.md` form was used
throughout.

- Agent Gateway: [overview](https://docs.litellm.ai/docs/a2a),
  [agent card](https://docs.litellm.ai/docs/a2a_agent_card),
  [invoking](https://docs.litellm.ai/docs/a2a_invoking_agents),
  [authentication headers](https://docs.litellm.ai/docs/a2a_agent_headers),
  [cost tracking](https://docs.litellm.ai/docs/a2a_cost_tracking),
  [permissions](https://docs.litellm.ai/docs/a2a_agent_permissions),
  [iteration budgets](https://docs.litellm.ai/docs/a2a_iteration_budgets),
  [kill switch](https://docs.litellm.ai/docs/a2a_kill_switch), and
  [Databricks agents](https://docs.litellm.ai/docs/a2a_databricks_agent).
- Identity and access: [Access Groups](https://docs.litellm.ai/docs/proxy/access_groups),
  [AI Hub](https://docs.litellm.ai/docs/proxy/ai_hub),
  [Budgets, Rate Limits](https://docs.litellm.ai/docs/proxy/users),
  [OIDC, JWT-based Auth](https://docs.litellm.ai/docs/proxy/token_auth),
  [MCP Permission Management](https://docs.litellm.ai/docs/mcp_control), and
  [MCP Tool Search](https://docs.litellm.ai/docs/mcp_tool_search).
- Providers reached as agents: [Bedrock AgentCore](https://docs.litellm.ai/docs/providers/bedrock_agentcore),
  [LangGraph](https://docs.litellm.ai/docs/providers/langgraph), and
  [Google AI Studio, Managed Agents](https://docs.litellm.ai/docs/managed_agents).
- SDK harness: [Agents (litellm.agent)](https://docs.litellm.ai/docs/harness),
  [Using with LiteLLM AI Gateway](https://docs.litellm.ai/docs/harness/gateway),
  [Sessions](https://docs.litellm.ai/docs/harness/sessions),
  [Sandboxes](https://docs.litellm.ai/docs/harness/sandboxes),
  [Events](https://docs.litellm.ai/docs/harness/events), and
  [Deep Agents](https://docs.litellm.ai/docs/harness/deepagents).
- Adjacent surfaces: [/memory](https://docs.litellm.ai/docs/memory_management)
  and [Agentic Loop Hook](https://docs.litellm.ai/docs/proxy/agentic_loop_hook).

Mechanical adaptations to quoted passages: markdown emphasis inside quotes
(`**...**`) is kept as LiteLLM wrote it, so it renders as bold here. Quotes
that span a markdown link in the source are split at the link. Page titles
containing a dash separator in the source (for example "OIDC - JWT-based
Auth") are written with a comma in link text, and the Managed Agents page
title is shortened the same way. No quoted passage required removing an em
dash or en dash; passages containing one were not quoted.
