# Synthesis: what an MCP gateway owns, and what trogonai has not decided yet

Part of MCP Gateway Research.
One question across the dossiers: when a platform puts a single edge between
agents and many tool servers, which responsibilities land on that edge, which
land on a registry behind it, and which stay with the tool owner? Purpose:
turn the industry evidence into the list of decisions this repository still
owes, with the records that already constrain each one. This synthesis is
decision-time input: where a conclusion here differs from an accepted record
in the [ADR index](../../adr/index.md), the ADR is authoritative.

## Evidence base

- [Uber MCP Gateway](./products/uber-mcp-gateway.md), the primary dossier of
  this corpus.
- [Bedrock AgentCore](../agent-platform/products/bedrock-agentcore.md), the
  "Gateway" section: MCP targets aggregated into a virtual server, outbound
  identity through a token vault keyed by agent and user, Cedar policy
  engines and guardrails applied at the gateway.
- [OpenAI Agents API](../agent-platform/products/openai-agents-api.md), the
  `mcp` tool with `allowed_tools`, `tool_search` with deferred loading, and
  programmatic tool calling.
- [xAI platform](../provider-agent-contracts/products/xai-platform.md), the
  `defer_loading` and `tool_search` contract where the caller holds the MCP
  token and the provider stores no server definition.
- [Guild](../agent-platform/products/guild.md), progressive disclosure of
  skills activated on demand.

## Convergence

1. Registry and proxy are separate planes. Uber names them (MCP Registry,
   Proxy Gateway); AgentCore's Gateway is configured from target records it
   does not own at runtime. The data plane consumes the registry's output and
   never writes to it. This matches the repository's current rule that
   gateways depend on, but do not absorb, control-plane services
   ([agent platform architecture](../../architecture/agent-platform.md)),
   and the platform's split between ARD discovery
   ([ADR#0012](../../adr/0012-ard-compatible-discovery-catalog.md)) and
   invocation transport
   ([ADR#0041](../../adr/0041-canonical-mcp-jsonrpc-bodies-over-nats.md)).

2. Discovery is not exposure. Uber states it as a core principle; AgentCore
   lists only the targets an operator attached to the virtual server; OpenAI
   and xAI require the caller to name `allowed_tools` or opt tools into
   deferred loading. No product lists a tool merely because a backend
   advertised it. The repository has the admission half of this
   ([ADR#0025](../../adr/0025-agent-definition-data-ownership.md) pins the
   tools an agent may use) but not the owner-side half: nothing records that
   a tool's owner enabled it for agents at all.

3. The edge enforces authorization by caller kind. Uber's caller actors are
   humans, services, and agents; AgentCore attaches Cedar policies at the
   gateway; the repository's CommandPrincipal already distinguishes the same
   kinds ([ADR#0026](../../adr/0026-command-authorization-principal.md)).
   Convergence stops at the edge, though: ADR#0026 rejects "authorize only at
   the gateway, trust everything past it", so for trogonai the edge is the
   first checkpoint, not the only one.

4. Tool definitions change through an owner-approved, reversible step. Uber
   gates every description change behind an owner-approved diff with
   rollback. The repository already has the shape for agent definitions
   (proposal, revision, revert minting a new revision in ADR#0025) and no
   equivalent for tool definitions, which today exist only as whatever the
   backend returns from `tools/list`.

5. Context budget is a gateway concern. Every product with many tools adds a
   lazy path: Uber's Omni server and code mode, OpenAI's and xAI's
   `tool_search` with deferred loading, Guild's progressive disclosure of
   skills. The repository's tool surface is still eager: the mcp-nats server
   forwards `tools/list` unchanged.

## Divergence

A. Where the lazy path lives. Uber puts it in the gateway (meta-tools and a
   CLI); OpenAI and xAI put it in the model provider's tool loop; Guild puts
   it in the agent's skill runtime. For trogonai the natural seam is the edge,
   because the provider route is resolved per session
   ([ADR#0032](../../adr/0032-model-route-and-credential-binding.md)) and a
   provider-side `tool_search` cannot see the session's pinned set.

B. Who owns the tool schema. Uber's registry owns a generated, owner-edited
   copy; AgentCore reads it from the target at configuration time; OpenAI and
   xAI never store it. The repository's stated rule is that the gateway must
   not own externally owned tool schemas and that an agent definition never
   copies a tool's schema; versions resolve at session start (ADR#0025). The
   open question is whether a registry record that owns a schema contradicts
   that rule or satisfies it by being the external owner.

C. Response shaping. Only Uber trims responses by a caller-supplied field
   projection; AgentCore applies guardrails to content; OpenAI and xAI leave
   responses intact. The repository has redaction for A2A through Wasm
   redactors and no MCP equivalent, and its session ledger records resource
   observations as byte-range extents rather than selected fields.

D. Third-party user identity. Uber exchanges an internal user token for the
   external provider's token at the gateway; AgentCore stores per-user
   credentials in a token vault keyed by agent and user; xAI makes the caller
   hold the token. The repository defers non-model grants
   ([ADR#0063](../../adr/0063-agent-connection-declarations.md)) and forbids
   delegated user authority through OIDC federation
   ([ADR#0053](../../adr/0053-external-oidc-federation-surface.md)), so a
   decision is required before any of these fits.

E. Transport and session state. Uber's edge is HTTP per virtual server with
   mesh delegation underneath and no stated session model. The repository's
   edge is NATS subjects with per-process MCP session state, and ADR#0055
   reserves queue-group semantics that the current mcp-nats transport does
   not use. The two are not comparable on reliability until the repository
   decides how session state survives a replica change.

## Decisions this repository still owes

Each row names the decision, the records that constrain it, and the shape we
expect it to take. None of these is decided here.

| Decision | Constrained by | Expected shape |
| --- | --- | --- |
| Tool registry as the control plane for MCP servers and tool definitions | ADR#0012, ADR#0025, agent platform architecture (registries deferred) | ADR: servers and tool definitions are owned, versioned records; the ARD catalog becomes a derived projection, not the source |
| Tool-level exposure, separate from authorization | ADR#0025 (admission), ADR#0026 (principal) | Exposure state on the registry record, owner-controlled, with discovery never implying it |
| MCP invocation authorization by principal kind with server and tool tiers | ADR#0026 (not gateway-only), a2a-gateway policy resolver | ADR mirroring the A2A per-skill resolver for MCP; override versus ceiling semantics decided explicitly; the term "charter" is already taken by `AGENT_CHANGE_CLASS_CHARTER` and must not be reused |
| Versioned tool definitions with owner-approved diffs and rollback | ADR#0025 (proposal lifecycle, revert mints a revision), digest-pinned `ExactToolVersionPin` | Reuse the proposal lifecycle; a running session keeps its pinned digest when `tools/list_changed` arrives |
| Proto services and decider commands as MCP tools | ADR#0016 (FileDescriptorSet transcoding), ADR#0057 (`Decide(Any)`) | Eligibility declared by a typed method option and explicit allowlists; never expose the raw decide entrypoint |
| Third-party user-token exchange | ADR#0053, ADR#0063 Decision 5 | Define the session-scoped grant and the connection resource for non-model providers that ADR#0063 leaves open; Uber and AgentCore both supply evidence |
| Response projection and MCP redaction | ADR#0039 (provenance), session `resource_observation` extents | Decide whether projection is request metadata or part of the agent-visible interface; fix the order authorize, invoke, redact, project, record |
| Lazy tool discovery at the edge | ADR#0025 (pinned set at session start) | Meta-tools that load only from the admitted pinned set |
| Replicated MCP sessions over NATS | ADR#0055 (queue groups), mcp-nats transport | Decide where session state lives before enabling queue groups; the subscribe call today has none |
| Catalog refresh and session consistency | ADR#0025 (versions resolve at session start) | Explanation page describing what a live registry change means for an in-flight session |
| MCP telemetry attributes | ADR#0008, `otel/semconv/registry/mcp.yaml` | Attributes for server, tool, principal kind, exposure decision, and projection applied |
| Glossary entries | existing glossary | ToolDefinition, tool exposure, invocation policy, `server_id`; disambiguate gateway, registry, and charter |

## What the repository already has that the dossiers do not

These are not gaps; they are commitments the gateway work must preserve.

- Signed proof-of-possession callers and fully bound request signing
  ([ADR#0050](../../adr/0050-signed-first-caller-authentication.md),
  [ADR#0051](../../adr/0051-fully-bound-request-signing.md)). Uber relies on
  an access-control system that detects the caller; it does not describe
  cryptographic binding of the request.
- Self-certifying agent identity and self-authenticating event provenance
  ([ADR#0036](../../adr/0036-agent-self-certifying-identity.md),
  [ADR#0039](../../adr/0039-self-authenticating-event-provenance.md)).
- Digest-pinned tool versions per session, with authorization evaluated live
  and never pinned (ADR#0025). Uber's live refresh has no stated pin, which
  is the mid-task drift case the repository already guards against.
- Revert as a new revision rather than a restore (ADR#0025), so rollback
  leaves a trail.
- Canonical, body-covered security over NATS
  ([ADR#0041](../../adr/0041-canonical-mcp-jsonrpc-bodies-over-nats.md),
  [ADR#0056](../../adr/0056-canonical-jsonrpc-bodies-over-nats.md)), rather
  than header-based trust.
- Tenancy enforced inside the trusted process, not only at the broker: a
  resolver declares the subject subtree it writes into and the store refuses
  anything outside it
  ([ADR#0027](../../adr/0027-decider-multi-tenancy-primitive.md)), with
  tenant namespaces carried in the subject prefix
  ([ADR#0055](../../adr/0055-nats-subject-design-jsonrpc-bindings.md)).
  Uber's isolation story is the mesh and the access-control system; it does
  not describe a check that survives a bug in a trusted component.

## Working definition

An MCP gateway is the single admitted edge through which an agent session
reaches tool servers. It executes against definitions it does not own,
enforces authorization for every principal kind on every call, shapes
responses only under rules the owner approved, and records enough that the
session ledger can reproduce what the model saw. The registry behind it owns
the definitions, their exposure state, and their change history; the edge
consumes that state and never originates it.
