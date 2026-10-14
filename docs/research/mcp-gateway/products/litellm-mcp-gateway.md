# LiteLLM MCP Gateway: one proxy edge where virtual keys decide which tools exist

Part of MCP Gateway Research.
Evidence snapshot retrieved 2026-10-13 from the LiteLLM documentation at
[docs.litellm.ai](https://docs.litellm.ai/docs/mcp), the "MCP Gateway" section
of its [llms.txt index](https://docs.litellm.ai/llms.txt) and the neighbouring
proxy pages it links. Every cited page declares `last_updated: "2026-10-12"`.
LiteLLM is open source, but this dossier studies the documented behaviour, not
the code. Text in quotation marks is verbatim from the cited page; unquoted
prose paraphrases it; anything that goes beyond what the pages say is labelled
as our inference or listed under open questions. Where this dossier and an
accepted record in the [ADR index](../../../adr/index.md) differ, the ADR is
authoritative.

## Summary

LiteLLM's proxy is first an LLM gateway with virtual keys, teams, budgets and
spend tracking, and the MCP gateway reuses that machinery for tools. Its
overview states the purpose in one line: "LiteLLM Proxy provides an MCP Gateway
that allows you to use a fixed endpoint for all MCP tools and control MCP
access by Key, Team." Upstream servers are registered once (YAML, Admin UI,
management API, or a team submission an admin approves), and clients reach all
of them through `/mcp` or a scoped `/{server_name}/mcp` path, with tool names
prefixed by the server. What a caller sees is the intersection of grants held
by the key, team, end user, agent, internal user and organization, narrowed
further by server-level tool lists, toolsets, pins, guardrails and an
IP-based public or internal split. The same catalogue is offered to models
through an `mcp` tool type on `/v1/chat/completions` and `/v1/responses`, where
the proxy itself runs the tool loop when approval is waived.

Our inference: the closest comparison in this corpus is not a tool registry but
an API-management product. The control plane is the key and team model that
already governs LLM traffic; the server record is mostly connection and
credential configuration; and the tool definition stays upstream-owned unless an
admin pins a snapshot. That makes LiteLLM strong evidence for authorization
composition, credential forwarding and rate limiting, and weak evidence for
owner-controlled tool definitions.

## Position between agents and MCP servers

### Endpoints

The [MCP Configuration Reference](https://docs.litellm.ai/docs/mcp_config_reference)
calls itself "the canonical decision reference" and gives one endpoint matrix:

- `/mcp`, MCP JSON-RPC over streamable HTTP, scoped to every server the key may
  use and optionally narrowed by the `x-mcp-servers` header.
- `/{server_name}/mcp`, the same protocol scoped to one server, a
  comma-separated list of servers, a toolset, or an access group.
- `/toolset/{toolset_name}/mcp`, scoped to one toolset.
- `server_url: "litellm_proxy"` inside the `tools` array of an LLM request,
  where "`litellm_proxy` is a literal sentinel, never a URL"; the form
  `litellm_proxy/mcp/<name>` selects one server or toolset.
- `GET /v1/mcp/server` for the server list, and `GET /mcp-rest/tools/list` plus
  `POST /mcp-rest/tools/call` for scripted calls without an MCP client
  ([MCP REST API](https://docs.litellm.ai/docs/mcp_rest_api)).

Path and header selection are filters, not grants: "Selecting a server by URL
or header narrows existing access; it does not grant access." With
`enable_mcp_registry: true` the proxy also serves `GET /v1/mcp/registry.json`
in the official MCP Registry format, and "The registry only advertises server
URLs." ([MCP Permission Management](https://docs.litellm.ai/docs/mcp_control)).

### Transports

The gateway speaks one transport to clients and three to upstreams: "Clients
always reach LiteLLM over streamable HTTP regardless of the upstream transport".
Upstream `transport` is `http` (streamable HTTP), `sse`, or `stdio`, where the
proxy spawns the command and maps request headers into the child's environment
with `${X-HEADER-NAME}` substitution. Stdio is off by default: "stdio MCP
servers are disabled unless the proxy is started with
`LITELLM_ENABLE_MCP_STDIO=true` in its environment", and the flag "is read from
the proxy's process environment only"
([MCP Overview](https://docs.litellm.ai/docs/mcp)).

The two registration paths disagree on the default transport, and the reference
documents the disagreement rather than fixing it: when `transport` is omitted
the YAML loader uses `http`, while "The management API create/update request
models default to `sse`."

Protocol version is negotiated, not configured: "LiteLLM v1.101.0 (`18243cd7`)
pins MCP SDK 1.28.1", and "There is no supported per-server `spec_version`
configuration field."

### Registration

Servers enter the registry through any of these paths.

- `config.yaml` under `mcp_servers`, keyed by server name.
- The Admin UI ("MCP Servers", "Add New MCP Server"), which needs
  `store_model_in_db` so the record persists in the database.
- The management API, `POST` and `PUT /v1/mcp/server`.
- A team submission. A team-scoped key calls `POST /v1/mcp/server/register` and
  "the server is held in a `pending_review` state until an admin approves or
  rejects it"; approval loads it into the runtime registry, and rejecting a
  previously active server means "It is evicted from the runtime registry
  immediately, so clients will no longer see its tools."
  ([MCP Server Submissions](https://docs.litellm.ai/docs/mcp_server_submissions)).

Identity of a YAML server is derived, which makes grants fragile: "LiteLLM
derives a config server's id from its name and connection settings. If you
change those settings, the id changes and existing key or team permissions stop
matching." The fix is an explicit `server_id`. Database and YAML records can
collide; per the overview, clashes with a database-backed server "only log a
warning", and on an id match the database record wins.

### Namespacing and tool name prefixes

"LiteLLM namespaces multiple MCP servers by prefixing each tool name with its
MCP server name", so new server names must comply with SEP-986 and
"noncompliant names cannot be added anymore". The REST page gives the shape
`{server_prefix}{separator}{upstream_tool_name}`, says the default separator
"is `-` (hyphen)", allows overriding it with `MCP_TOOL_PREFIX_SEPARATOR`, and
offers `LITELLM_USE_SHORT_MCP_TOOL_PREFIX=true` for a 3-character id instead of
the server name. The upstream receives the unprefixed name.

Aliases sit on top: `litellm_settings.mcp_aliases` maps a short name to a
server, and "Only the first alias for each server is used". The overview's
example of the benefit uses an underscore join ("Use `github_create_issue`
instead of `github_mcp_server_create_issue`"), while the configuration
reference and REST page show hyphen joins such as `github_mcp-search_issues`.
We record the two pages as inconsistent and do not pick one.

## Registry: what a server record owns

Collected from the overview, permission, cost, rate-limit, public-internet and
per-user credential pages, a server record carries:

- Identity and naming: server name, optional stable `server_id`, alias (the
  tool prefix), description.
- Connection: `url` or, for stdio, `command`, `args` and `env`; `transport`.
- Upstream authentication: `auth_type` and `auth_value`, OAuth client fields,
  `static_headers`, `extra_headers` (client header names to forward),
  `upstream_token_header`, server variables scoped per instance or per user,
  and `is_byok` for per-user stored keys.
- Tool surface: `allowed_tools`, `disallowed_tools`, `allowed_params` (tool to
  permitted argument names), `pinned_tools` (a frozen name, description and
  input schema per tool), and a `tool_name_to_description` override.
- Access: `access_groups`, `allow_all_keys` (the server becomes available to
  every key), and `available_on_public_internet`.
- Cost and load: `mcp_info.mcp_server_cost_info` with `default_cost_per_query`
  and `tool_name_to_cost_per_query`, a server-wide `rpm`, and a max concurrent
  requests setting the rate-limit section mentions only by its UI label.
- Lifecycle for submitted servers: `approval_status`, `submitted_by`,
  `submitted_at`, and optional `review_notes`.

What the record does not own is the tool definition itself. Without a pin the
gateway serves whatever the upstream returns from `tools/list`; the permission
page states it directly, "`allowed_tools` trusts whatever the upstream says each
tool does." A separate proxy-wide registry,
[Tool Policies](https://docs.litellm.ai/docs/proxy/tool_policies), "is a
registry of every tool the proxy has seen in traffic", populated after the fact
from MCP calls and LLM request and response tool lists, with an input and an
output trust policy per tool. Newly seen tools start untrusted on both, and
"Tools are registered asynchronously after the request completes, as part of
the same background flush that records spend".

Our inference: LiteLLM has two registries with different keys. The MCP server
registry is keyed by server and is authored by admins; the Tool Policies
registry is keyed by bare tool name and is discovered from traffic. The pages
do not say how a prefixed MCP tool name maps onto a Tool Policies row.

## Authorization

### Two layers of authentication

"Gateway and upstream authentication are configured separately. Keep their
credentials separate". The gateway credential is a LiteLLM virtual key (or SSO
session or JWT), preferably in `x-litellm-api-key` so that `Authorization` stays
free for upstream tokens. The upstream credential comes from the server's
`auth_type` or from the client per request.

### Entity scoping and the permission hierarchy

[MCP Permission Management](https://docs.litellm.ai/docs/mcp_control) lists
grant levels: key, team, end user (from `x-litellm-end-user-id`), agent (from
`x-litellm-agent-id` or the agent a key is bound to), internal user, and
organization. Each level holds `object_permission.mcp_servers` and
`object_permission.mcp_access_groups`. The composition rule is intersection
with "most-restrictive wins", except that the organization is a ceiling, and an
empty key list inherits the team's list. When no level has a list the request is
"open by default".

The page argues against its own default. A `require_key_mcp_access_defined`
setting turns the team list into a ceiling instead of a default, and "Turning
this on is the recommended posture." The reason given: "With inheritance, every
key issued under a team silently reaches every MCP server that team can reach".
A sentinel `no-mcp-servers` denies a key everything, overriding both
inheritance and access-group grants.

The internal-user level binds the human rather than the credential, and an admin
role does not lift it: "the admin role widens what the credential reaches, and
leaves the scope attached to the person in place." Only a proxy admin may set
it, because an empty list means no restriction and a self-write could lift a
ceiling.

IdP groups never grant MCP access directly: "LiteLLM does not map IdP claims
straight to MCP access groups, and this is by design." A SCIM group becomes a
team, and the team's `object_permission` carries the grant
([Grant MCP Server Access](https://docs.litellm.ai/docs/mcp_grant_access)).

### Access groups and toolsets

MCP access groups are labels on server records (`access_groups: ["dev_group"]`)
that keys and teams can be granted and that clients can name in a path or in
`x-mcp-servers`. A separate, newer
[Access Groups](https://docs.litellm.ai/docs/proxy/access_groups) object bundles
models, MCP servers and agents into one attachable grant. A
[toolset](https://docs.litellm.ai/docs/mcp_toolsets) is "a named collection of
specific tools drawn from one or more MCP servers", stored with
`{server_id, tool_name}` pairs and granted like a server.

### Tool-level permissions

Tool restriction exists at two tiers.

- Server tier, applying to every caller: `allowed_tools` or `disallowed_tools`
  ("If you specify both `allowed_tools` and `disallowed_tools`, the allowed list
  takes priority"), and `allowed_params`, under which a call carrying any other
  argument name is rejected with HTTP 403.
- Entity tier: `object_permission.mcp_tool_permissions`, a map from server id to
  tool names, evaluated after server access and composed with the same
  intersection and organization ceiling.

Enforcement is at both listing and calling: "The entitlement is applied at
`tools/list` time and again at `tools/call` time, so a tool outside it is never
advertised and a client that hardcodes the name is still refused." The refusal
arrives as an MCP result with `isError: true`.

### Network and client gating

Two gates sit beside the entity model. An IP classifier splits callers into
internal and external, and external callers see only servers marked
`available_on_public_internet`; "This filtering is applied at every MCP access
point: the MCP registry, tool listing, tool calling, dynamic server routes, and
OAuth discovery endpoints." The
[deployment guide](https://docs.litellm.ai/docs/mcp_deployment) notes "The
default is `true`, so a server is visible to external callers unless you
explicitly set it to `false`." A client-application allowlist reads a JWT claim
such as `azp` or `cid`, and "LiteLLM checks the client identity after
authentication and returns HTTP 403 when it is not allowed"; "Passing the client
check does not grant access to additional MCP servers or tools."
([Allowlisting MCP Client Applications](https://docs.litellm.ai/docs/mcp_client_allowlist)).

### Forwarding credentials to upstream servers

The `auth_type` values span static credentials (`api_key`, `bearer_token`,
`basic`, `authorization`, `token`), OAuth (`oauth2` with an explicit
`oauth2_flow` of `authorization_code` or `client_credentials`), exchange
(`oauth2_token_exchange` for RFC 8693 or Entra On-Behalf-Of, `oauth2_id_jag`
for Okta ID-JAG), passthrough (`true_passthrough`, `oauth_delegate`), and
`aws_sigv4`.

Client-supplied credentials travel in per-server headers of the form
`x-mcp-{server_alias}-{header_name}`, and the reference calls this "the
supported client-side credential mechanism". A group form
`x-mcp-{access_group}-{header_name}` fans one credential out to every server in
the group, a server-specific header wins, and "if a server is in several groups
that carry different credentials none of them is forwarded". Aliases and group
names collide by design: "Server aliases and access group names share the
`x-mcp-` header namespace". The older broadcast header `x-mcp-auth` is
deprecated but still honoured.

Stored per-user credentials (BYOK keys, per-user variables, per-user OAuth
tokens) live in the "LiteLLM database, encrypted" and are keyed by user and
server, not by key: "Two keys with the same `user_id` share the stored
credential for that server." and "There is no native stored per-key or per-team
credential override"
([Per-User and Per-Key Upstream Credentials](https://docs.litellm.ai/docs/mcp_per_user_auth)).

The passthrough modes are where admission and forwarding separate
([MCP OAuth Passthrough](https://docs.litellm.ai/docs/mcp_oauth_passthrough)).
In both, "LiteLLM does not decode it, does not check its audience or scopes, and
does not exchange it; the upstream MCP server is the only party that validates
it." Under `true_passthrough` there is no LiteLLM admission at all, and the page
is blunt about the cost: "The MCP route becomes an unauthenticated ingress at
the LiteLLM layer." Under `oauth_delegate` the caller is admitted first and
"Spend, rate limits, and audit resolve against the admission identity." In
every mode "Admission credentials are never forwarded upstream." The page frames
the audience rejection that follows a misrouted token as correct behaviour,
because "the gateway did not launder a token minted for one resource into access
to another".

## Tool discovery and filtering

### What shapes a `tools/list` response

A listing on `/mcp` starts from every server the caller's composed entitlement
reaches and narrows from there: the client may pass `x-mcp-servers` with
server names, aliases or access group names, or call a scoped path
(`/{server_name}/mcp`, `/toolset/{name}/mcp`). Per the
[config reference](https://docs.litellm.ai/docs/mcp_config_reference), `/mcp`
returns "All servers the key/team is permitted to use, optionally narrowed by
`x-mcp-servers`", with tool names "prefixed with the server alias". Server-tier
`allowed_tools` and `disallowed_tools`, entity-tier `mcp_tool_permissions`,
the public-internet flag and any pin then remove tools before the response.

Our inference: the composition is subtractive. Nothing in the pages lets a
client see a tool that an admin-side list did not already admit, which matches
the principle that discovery is not exposure in the
[synthesis](../synthesis.md). The admitting party is the gateway admin, not the
tool owner.

### Pins and description scanning

Two features treat the upstream's own description as untrusted. Pinning
"freezes the tool list, the descriptions, and the input schemas too", so "A
tool the upstream added after the pin is not listed, and a call to it returns
`403`", and drift raises an `mcp_pinned_tools_changed` alert until someone
re-pins. The pin is not total: "A pin covers tool names, descriptions, and
input schemas; anything else the upstream reports about a tool (annotations,
output schema) is served live"
([MCP Permission Management](https://docs.litellm.ai/docs/mcp_control)).
Separately, a `pre_mcp_call` guardrail "also runs on every tool an upstream
server returns from `tools/list`, before the gateway serves the listing", hides
blocked tools, and sends an `mcp_tool_description_blocked` alert
([MCP Guardrails](https://docs.litellm.ai/docs/mcp_guardrail)).

The pin belongs to the server record and is applied to every caller at once.
The pages describe no per-session or per-key pin, so a re-pin changes the
surface for sessions already in flight (a gap, recorded under Open questions).

### Tool search

[MCP Tool Search](https://docs.litellm.ai/docs/mcp_tool_search) is a per-key
opt-in (`mcp_tool_search_enabled` on the key's `object_permission`) that
replaces the catalog with virtual tools: `mcp_tool_search`, `mcp_tool_call`,
`agent_search` and `skill_search`. MCP tool ranking "is a token-overlap count
against the tool's `name` and `description` fields; no embeddings and no extra
dependency", while agent and skill search are ranked by embeddings. Keys without
the flag get the full catalog.

The page contradicts itself on how many virtual tools a key sees. It says the
endpoints "return exactly four tools regardless of how many MCP servers the key
can reach", and that "the LLM sees three tools and drives discovery itself" and
"a key with tool search on will only expose the three virtual tools", and that
for keys without the flag "the two virtual tool names are rejected with a
`forbidden` error". Our inference is that the smaller numbers predate the agent
and skill search tools, but we do not resolve the inconsistency.

### Semantic filter

The [MCP Semantic Filter](https://docs.litellm.ai/docs/mcp_semantic_filter) is
the request-time alternative for LLM API callers: it embeds tool descriptions
and the user query, keeps up to `top_k` tools above `similarity_threshold`, and
reports the effect in response headers (`x-litellm-semantic-filter`,
`x-litellm-semantic-filter-tools`). It "Fails gracefully - returns all tools if
filtering fails". Our inference: because failure widens the tool list, the
filter is a context-budget device and not an authorization control, and the
entitlement filters above remain the only exposure boundary.

Deferred loading in the provider sense (`defer_loading`) is not documented on
the MCP pages we cached; LiteLLM's lazy path is the gateway-side tool search.

## Cost, rate limits, guardrails and observability

### Cost and spend

[MCP Cost Tracking](https://docs.litellm.ai/docs/mcp_cost) offers
"Config-based Cost Tracking" (`default_cost_per_query` per server and
`tool_name_to_cost_per_query` per tool under `mcp_info.mcp_server_cost_info`)
and a "Custom Post-MCP Hook" that sets
`response_obj.hidden_params.response_cost`. The overview adds that passing
`x-litellm-end-user-id` lets operators "Apply customer-specific budgets" and
"Track spend per customer". The cost page does not say whether tool cost counts
against key or team budgets the way model spend does (an open question).

### Rate limits

`mcp_rpm_limit` is "a `Dict[str, int]` keyed by MCP server name" on keys, teams
and users, and "A key-level limit takes precedence over a team-level limit for
the same server; the team limit otherwise applies to every key on the team as a
shared counter." A server-wide `rpm` is "keyed by the server's ID only, so all
keys, teams and users share one bucket, and when the proxy uses Redis the bucket
is shared across every proxy instance." "Every MCP operation sent to that server
counts", including listings, and rejections are an MCP error on `/mcp` and
`429 Too Many Requests` on REST routes. The limits switch off silently in one
configuration: "With `LEGACY_MULTI_INSTANCE_RATE_LIMITING=true`, neither
`mcp_rpm_limit` nor the server `rpm` is enforced."

Our inference: the key limit overriding the team limit is override semantics,
while server access and tool permissions use ceiling semantics. The same
product picks differently per concern, which is the choice the synthesis says
the repository must make explicitly.

### Guardrails

[MCP Guardrails](https://docs.litellm.ai/docs/mcp_guardrail) run in three
modes. `pre_mcp_call` runs before the call on its input and also scans tool
descriptions at listing time; `post_mcp_call` runs on the tool result "before
the model sees it", blocking or masking
text and `structuredContent` strings; `during_mcp_call` is described only as
"Use this mode for real-time monitoring and intervention". A blocked result on
`/mcp` is "returned as an MCP tool error (`result.isError: true`), which can
arrive with HTTP 200", and under Responses auto-execution "the model receives a
tool error instead of the blocked content". One configuration trap is
documented: "MCP sub-calls do not inherit the parent request's `guardrails`
selection, so set `default_on: true` on the guardrail."

[Tool Policies](https://docs.litellm.ai/docs/proxy/tool_policies) add a
trust-label layer over every tool the proxy has seen, MCP or not, with per-tool
input and output policies.

### Logging and telemetry

The MCP pages document logs, alerts and debug headers rather than traces.
[MCP Troubleshooting](https://docs.litellm.ai/docs/mcp_troubleshoot) relies on
proxy log lines and the `x-mcp-debug-*` response headers enabled by
`x-litellm-mcp-debug: true`. Alerts exist for pin drift and blocked
descriptions. The only OpenTelemetry signal tied to MCP on the pages we cached
is billing: the proxy "counts successful requests to LLM, MCP, and A2A
endpoints and pushes one OpenTelemetry counter to LiteLLM's collector", and
"Only the request count is sent"
([Billable Request Metering](https://docs.litellm.ai/docs/proxy/billing_metrics)).
No MCP-specific span or metric attributes are documented (a gap, not evidence
that none exist).

## MCP tools inside /chat/completions and /responses

The LLM APIs accept an `mcp` tool whose `server_url` is the sentinel
`litellm_proxy` (or `litellm_proxy/mcp/<name>` to target one server), so the
proxy resolves the tools from its own registry under the calling key's
entitlement. Per [Using your MCP](https://docs.litellm.ai/docs/mcp_usage), with that
sentinel "LiteLLM bridges non-MCP providers to your MCP tools" and "This enables MCP tool usage with any LiteLLM-supported provider,
regardless of native MCP support."

Execution depends on `require_approval`. Setting it to `"never"` "triggers
automatic tool execution, returning the final response in a single API call
without additional user interaction." On chat completions, "If you
omit `require_approval` or set it to any value other than `"never"`, the MCP
tool calls are returned to the client so that you can review and execute them
manually, matching the upstream OpenAI behavior."
([MCP overview](https://docs.litellm.ai/docs/mcp)).

Our inference: in auto-execution mode the proxy is the agent loop for tool
calls, so the gateway, the model route and the tool executor collapse into one
process for that request. Guardrails, pins, entitlements and the description
scan still apply, because the pages state that the listing scan covers "the
discovery a `/v1/responses` or `/v1/chat/completions` request runs with
`server_url: "litellm_proxy"`".

## Against this repository's ADRs

Where this section differs from a record in the
[ADR index](../../../adr/index.md), the ADR is authoritative. Draft records are
named as drafts; they constrain direction but are not yet decisions.

### Already decided, and where LiteLLM differs

- Authorization past the edge.
  [ADR#0026](../../../adr/0026-command-authorization-principal.md) (accepted)
  rejects "Authorize only at the gateway, trust everything past it". LiteLLM's
  entire model is gateway-side: the upstream receives either a credential the
  gateway chose or one the client supplied. We infer the upstream sees no
  statement of which LiteLLM principal is calling unless an exchange mode
  carries it. LiteLLM is evidence for what the first checkpoint can hold, not
  for where enforcement ends.
- Credential custody.
  [ADR#0023](../../../adr/0023-secret-management-and-key-custody-direction.md)
  (accepted) names outbound MCP with per-company `Authorization` headers as a
  consumer and has consumers hold `SecretRef`s resolved through the secrets
  service. LiteLLM keeps static, OAuth and BYOK credentials in its own
  database. The custody location is decided for this repository and differs;
  the forwarding shapes (static, client-supplied, per-user stored, exchanged)
  remain useful inputs.
- Discovery versus invocation.
  [ADR#0012](../../../adr/0012-ard-compatible-discovery-catalog.md) (accepted)
  separates the catalog from invocation. LiteLLM's optional
  `GET /v1/mcp/registry.json` lists servers for clients and is a projection of
  the registry, which fits that split.
- Transport. [ADR#0041](../../../adr/0041-canonical-mcp-jsonrpc-bodies-over-nats.md)
  and [ADR#0055](../../../adr/0055-nats-subject-design-jsonrpc-bindings.md)
  (both draft) carry MCP over NATS, as the mcp-nats transport already does. LiteLLM speaks streamable HTTP to clients and HTTP, SSE
  or stdio upstream; nothing in its transport model transfers.
- Caller authentication.
  [ADR#0050](../../../adr/0050-signed-first-caller-authentication.md) and
  [ADR#0051](../../../adr/0051-fully-bound-request-signing.md) require signed,
  request-bound callers. LiteLLM's gateway credential is a bearer virtual key,
  session or JWT, and the pages describe no proof of possession.
- Telemetry. [ADR#0008](../../../adr/0008-opentelemetry-observability.md)
  (accepted) makes OpenTelemetry the observability path. LiteLLM documents MCP
  logs and alerts but no MCP span attributes, so it does not inform the
  attribute set the repository still owes.

### Constrained by drafts, and what LiteLLM adds

- Tool definitions and pins.
  [ADR#0025](../../../adr/0025-agent-definition-data-ownership.md) (draft)
  gives the tool schema to a tool definition owned outside the agent, pins
  versions at session start, and never changes an in-flight pin. LiteLLM's
  `pinned_tools` is the closest feature in any dossier so far: names,
  descriptions and input schemas frozen, with drift alerts and refusal of
  unpinned tools. It differs on owner (the gateway admin rather than the tool
  owner), on scope (server-wide rather than per session) and on coverage
  (annotations and output schema stay live).
- Credentials for tools.
  [ADR#0032](../../../adr/0032-model-route-and-credential-binding.md) (draft)
  excludes tool credentials from model routing, and
  [ADR#0063](../../../adr/0063-agent-connection-declarations.md) (draft)
  Decision 5 leaves the session-scoped grant, the connection resource for
  non-model providers, and the egress mediation component undefined. LiteLLM's
  `auth_type` spectrum and its per-user store keyed by user and server are
  direct evidence for that grant record, including the documented limit that
  there is no per-key or per-team stored override.
- On-behalf-of exchange.
  [ADR#0053](../../../adr/0053-external-oidc-federation-surface.md) (draft)
  Decision 6 states that "delegated authority is not implied" and that an
  assertion "never carries delegated user authority"; such authority comes from
  the person-linked principal of
  [ADR#0017](../../../adr/0017-aauth-agent-authentication.md), carried
  separately. LiteLLM's `oauth2_token_exchange` mode derives the upstream token
  from the caller's token. Our inference: adopting that shape would require the
  person-linked principal to be the exchange input, not the gateway credential.

### Still owed, with LiteLLM evidence

- Override versus ceiling semantics. LiteLLM uses ceilings for server and tool
  access (organization ceiling, intersection, opt-in team ceiling for keys) and
  override for rate limits (key over team). The synthesis already lists this as
  owed.
- Owner-controlled exposure. LiteLLM's exposure is admin-controlled at the
  gateway; no page describes a tool owner enabling a tool for agents. LiteLLM
  does not supply evidence for the owner-side half.
- Untrusted upstream descriptions. Pins with drift alerts and a guardrail scan
  over descriptions at listing time are two concrete mechanisms for the
  versioned tool definitions row.
- Per-caller and per-server MCP rate limits. No record in the index addresses
  them; LiteLLM shows both tiers and a shared server bucket.

## Open questions

- MCP session state across replicas. The pages share rate-limit buckets
  through Redis but do not say whether an MCP session established on one proxy
  instance survives a request routed to another.
- Pins and in-flight sessions. A pin is server-wide and applies on the next
  listing; the pages do not say what a session that already listed tools sees
  after a re-pin or an unpin.
- `during_mcp_call` semantics. The guardrail page names the mode but does not
  describe what it can observe or interrupt.
- MCP cost against budgets. The cost page shows how a cost is computed but not
  whether it debits key, team or organization budgets alongside model spend;
  only end-user budgets are mentioned, on the overview.
- Prefixed names in Tool Policies. The proxy-wide tool registry is keyed by
  tool name; the pages do not say whether an MCP tool appears there with or
  without its server prefix, or how two servers exposing the same upstream name
  are told apart.
- Tool Policies timing. Registration is asynchronous after the request, so the
  first call to a never-seen tool happens before it has a policy row; the pages
  do not say which policy applies to that first call.
- MCP telemetry. No MCP-specific span or metric attributes are documented; the
  OpenTelemetry integration pages outside the MCP section were not studied.
- Virtual tool count under tool search. The page states four, three and two in
  different places (see Tool search).

## Source anchors

All pages retrieved 2026-10-13 as the markdown the documentation site serves at
`https://docs.litellm.ai/docs/<slug>.md`. Every page declares
`last_updated: "2026-10-12"`. That date is the only version pin the
documentation offers for most claims, so the pin is weak: the pages are
continuously edited, they are not tied to a release tag, and the one explicit
release reference is in the configuration reference ("LiteLLM v1.101.0
(`18243cd7`) pins MCP SDK 1.28.1" and "LiteLLM v1.103.0-rc.1 (`ccf5e8c9`) pins
SDK 2.2.0"). The documentation sources are no longer in the BerriAI/litellm
repository under `docs/my-website/docs/mcp/`, so the served markdown was cached
instead of the repository files.

- MCP overview: https://docs.litellm.ai/docs/mcp
- Using your MCP: https://docs.litellm.ai/docs/mcp_usage
- MCP Configuration Reference: https://docs.litellm.ai/docs/mcp_config_reference
- MCP Permission Management: https://docs.litellm.ai/docs/mcp_control
- Grant MCP Server Access to Keys and Teams: https://docs.litellm.ai/docs/mcp_grant_access
- MCP Cost Tracking: https://docs.litellm.ai/docs/mcp_cost
- MCP Guardrails: https://docs.litellm.ai/docs/mcp_guardrail
- MCP Tool Search: https://docs.litellm.ai/docs/mcp_tool_search
- MCP Semantic Tool Filter: https://docs.litellm.ai/docs/mcp_semantic_filter
- MCP Non-OAuth Authentication: https://docs.litellm.ai/docs/mcp_authentication
- MCP Per-User and Per-Key Upstream Credentials: https://docs.litellm.ai/docs/mcp_per_user_auth
- MCP OAuth: https://docs.litellm.ai/docs/mcp_oauth
- MCP OAuth Passthrough: https://docs.litellm.ai/docs/mcp_oauth_passthrough
- MCP OBO Auth: https://docs.litellm.ai/docs/mcp_obo_auth
- Exposing MCPs on the Public Internet: https://docs.litellm.ai/docs/mcp_public_internet
- Allowlisting MCP Client Applications: https://docs.litellm.ai/docs/mcp_client_allowlist
- MCP Toolsets: https://docs.litellm.ai/docs/mcp_toolsets
- MCP Server Submissions: https://docs.litellm.ai/docs/mcp_server_submissions
- MCP REST API: https://docs.litellm.ai/docs/mcp_rest_api
- MCP from OpenAPI Specs: https://docs.litellm.ai/docs/mcp_openapi
- MCP Troubleshooting Guide: https://docs.litellm.ai/docs/mcp_troubleshoot
- MCP Deployment Guide: https://docs.litellm.ai/docs/mcp_deployment
- MCP Zero Trust Auth (JWT Signer): https://docs.litellm.ai/docs/mcp_zero_trust
- Access Groups: https://docs.litellm.ai/docs/proxy/access_groups
- Tool Policies: https://docs.litellm.ai/docs/proxy/tool_policies
- Billable Request Metering: https://docs.litellm.ai/docs/proxy/billing_metrics
- Documentation index: https://docs.litellm.ai/llms.txt

Upstream inconsistencies recorded rather than resolved:

- Alias join character: an underscore in the overview's alias example, a
  hyphen in the configuration reference and the REST page.
- Default transport: `http` for YAML and `sse` for the management API, a
  divergence the configuration reference itself documents.
- Virtual tool count: four, three and two on the same tool search page.

Rewrites made so the quotes render in this site: none of the quoted passages
contained an em or en dash or a double-brace template. Relative documentation
links in the source pages (for example `./a2a.md` and `/docs/mcp#...`) are cited
here as absolute `https://docs.litellm.ai/docs/...` URLs.
