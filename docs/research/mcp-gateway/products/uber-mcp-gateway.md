# Uber MCP Gateway: a registry-backed proxy for internal services

Part of MCP Gateway Research.
Evidence snapshot retrieved 2026-10-07 from the Uber Engineering post
[Designing MCP Gateway](https://www.uber.com/us/en/blog/designing-mcp-gateway/),
published 2026-10-01 by Alok Srivastava, Deepanshu Mehndiratta, and Gaurav
Gill. The post is prose with diagrams and names no public repository, so the
post itself is the only evidence. Text in quotation marks is verbatim from the
post; unquoted prose paraphrases it; anything that goes beyond what the post
says is labelled as our inference or listed under open questions. Quotations
that contain typographic dashes in the original are shortened to the clause
that carries the claim.

## Summary

Uber fronts its internal services with a single MCP edge made of a control
plane (the MCP Registry) and a data plane (the Proxy Gateway). Services are
not rewritten for agents: a crawler turns existing protobuf and Thrift
definitions into MCP tools, native MCP servers are discovered from heartbeat
signals, and third-party SaaS servers are reached through token exchange. Every
discovered server and tool is disabled until its owning team reviews and
enables it. At the time of writing the edge hosts "over 800 MCP servers and
over 5000 tools". The post's closing lesson is that "the hardest part isn't the
AI", it is "the discovery, the security, the reliability that makes agents
trustworthy enough to act on behalf of real users in a production
environment".

## Problem as framed by the post

Early adoption produced fragmented, per-team MCP integrations with no shared
tooling, security standard, or operational consistency, and the organisation
needed to go from dozens to hundreds of teams using MCP. The post also names a
protocol-level constraint that shaped the later design: MCP has no cross-server
search, so "an agent has to already know which server to talk to before it can
ask what tools are available. Configuring an agent to use an MCP server
requires explicitly wiring up the server URL, credentials, and tool list. Doing
this for hundreds of servers doesn't scale, as all this context would eat up
the model context limit."

## Control plane: the MCP Registry

"The MCP Registry maintains a catalog of hundreds of MCP servers backed by
internal services, along with thousands of MCP tools." It is the source of
truth for discovery, ownership, and enablement.

### Discovery does not imply exposure

"A core design principle of the MCP Gateway is that discovery doesn't imply
exposure. Every MCP server and tool starts in a disabled state and must be
explicitly reviewed and enabled by the owning team."

### AutoCrawler

"Autocrawler is a Cadence-powered distributed workflow system subscribed to
Uber's IDL registry and internal service signals." A scheduled job triggers
workflows that scan for new services, APIs, and schema changes. It derives
servers from the IDL registry and from native-server heartbeats.

- IDL-backed services (protobuf and Thrift). The crawler creates or updates a
  virtual MCP server for the service, parses the IDL for method names,
  schemas, and documentation, translates the schemas into MCP JSON schemas,
  and "uses an LLM to generate enriched, agent-friendly MCP tool descriptions
  based on the extracted schemas and comments". Generated tools are registered
  disabled.
- Native MCP servers. Services built on MCPFx, Uber's framework for native
  servers, emit heartbeat metrics that signal presence and readiness. The
  crawler watches those metrics, calls `tools/list`, and creates a virtual
  proxy server holding every discovered tool, disabled.

### Third-party MCP servers

Servers such as Jira and Google are provisioned through a separate flow
rather than crawled. The gateway "relays the caller's user token downstream
while enforcing essential gateway capabilities, including authorization, rate
limiting, and sensitive data redaction", and a Third-Party MCP Service
"exchanges the internal user token for a corresponding third-party
authentication token before dispatching the request to the external MCP
server."

### Ownership and change control

Owners review and refine generated definitions before enabling them. "Every
change to the tool description triggers a config change diff, which must be
approved by server owners. Owners can approve and deploy the config change,
and, if needed, roll back to a previous known version."

## Data plane: the Proxy Gateway

"The Proxy Gateway is responsible for executing MCP requests at runtime." It
"continuously consumes server and tool configurations from the control plane
and refreshes its in-memory state at a fixed cadence, allowing configuration
changes, such as tool updates or enablement changes, to take effect in real
time without service restarts or redeployments." Virtual servers are
materialised from that configuration: "for each virtual server, the Gateway
exposes a single `/<service-name>/mcp` endpoint that serves as the entry
point for AI agent execution."

### Protocol translation

Server handlers are described as tool-aware and downstream-aware. For
IDL-backed services the gateway translates the JSON payload to the wire
format, serialises it to protobuf or Thrift bytes, forwards it, and translates
the response back to MCP JSON. For native MCP servers the request is proxied
transparently. The stated principle is "translating HTTP, gRPC, and TChannel
calls into MCP-compatible interactions transparently, through Muttley, with
zero changes to downstream services."

### Service mesh delegation

Request execution is delegated to Muttley, Uber's service-mesh sidecar, so the
gateway inherits existing service-to-service routing. Our inference: retries,
timeouts, load balancing, and mTLS are therefore mesh concerns, and the post
does not describe any of them at the MCP layer.

## Security

"MCP gateway uses Uber's internal Access Control System to apply different
charter policies configured on detected caller actors (humans, services, and
agents)." Policies are defined per server with optional per-tool overrides.
Redaction of PII and other sensitive data from tool responses is provided out
of the box. The post does not say how the caller actor is detected, how
policies compose when a server rule and a tool rule disagree, or what the
redaction engine is.

## Context budget tactics

### Omni MCP

A single proxy server that reaches any gateway server through gradual
discovery. Its tools, quoted as listed:

- `discover_server`, "discover MCP server based on the query intent"
- `discover_tools`, "lookup tools for a server"
- `get_tool_schema`, "get the json schema for a tool"
- `invoke_tool`, "invoke a tool"

### Response projection

"MCP Gateway also offers Response Projection, a GraphQL-like calling pattern
for MCP tools. It works by injecting a new field in the tool request schema,
which instructs the gateway to request only the needed fields, not all. The
Gateway then trims the response in runtime by only keeping the projected
fields." The model supplies an array of nested field paths. The post credits
this with making enterprise-scale API schemas usable by agents.

### Code mode

`aifx`, Uber's CLI for agentic operations, routes MCP calls through the gateway
without installing an MCP server on the agent: "aifx mcp list - list available
MCP servers; aifx mcp search - search for tools across all MCP servers; aifx
mcp call - invoke an MCP tool through MCP Gateway." Agents chain commands in a
single invocation, write output to files, and grep selectively so only the
needed content enters model context. "Code Mode is now the company default for
MCP tool use in coding agents."

## Scale and stated outcome

"MCP Gateway unlocked a scalable, fast, and consistent path to building AI
agents within Uber and is currently hosting over 800 MCP servers and over 5000
tools." The value propositions the post lists are discovery and installation
without manual onboarding, exposing existing APIs with no code changes,
centralised observability and security, and centralised ownership and
governance.

## Open questions the post leaves

- Session semantics. Nothing is said about MCP sessions, `initialize`, or how
  a replicated gateway keeps per-session state.
- Version pinning. Configuration takes effect "in real time"; the post does
  not say whether an in-flight agent sees a tool change mid-task.
- Identity carried to backends. Whether the backend receives the human, the
  agent, or both, and in what form, is not described.
- Authorization composition. Tool-level overrides are mentioned without saying
  whether a narrower allow can override a broader deny.
- Projection provenance. What is recorded about a trimmed response, and
  whether the projection parameter changes the tool's schema identity, is not
  discussed.
- Rate limiting. Named only for third-party servers.

## Sources

- Uber Engineering, [Designing MCP Gateway](https://www.uber.com/us/en/blog/designing-mcp-gateway/),
  2026-10-01.
