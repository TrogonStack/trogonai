# Agent Platform

TrogonAI is building toward a data-driven Agent control plane and a durable,
role-neutral Session shell. The core direction is intentional: an Agent has a
stable identity and immutable revisions, and each Session runs one pinned
revision through one concrete implementation. The platform does not need
separate runtime classes for a planner, coder, researcher, validator, or
security agent.

This page is both a target architecture and a status map. It does not claim
that every box exists today. Accepted ADRs establish the durable direction;
draft ADRs describe proposed details that can still change.

## Status vocabulary

- **Current foundation** means a contract or implementation exists in this
  repository, or an accepted ADR fixes the boundary. A contract may still be
  partial or pre-production.
- **Target state** means the intended architecture. Some parts depend on draft
  ADRs and still require implementation.
- **Not a commitment** means a plausible extension, not a promised component or
  selected product.

## Current foundation

### Agent identity and revision genesis

[ADR#0024](../adr/0024-agent-platform-stream-topology.md) is accepted. It fixes
the main registry invariant: an Agent is a stable identity, revision 1 is minted
at provisioning, and later behavior changes become new immutable revisions
through a separate proposal and activation lifecycle.

The current `v1` wire contract is only the first slice of that lifecycle:

- `proto/trogonai/agents/agents/v1/agent.proto` defines
  `AgentConfiguration` as a runtime identifier plus runtime-owned settings.
- `proto/trogonai/agents/agents/v1/agent_provisioned.proto` records identity,
  display name, placement, configuration, revision 1, and its digest.
- `proto/trogonai/agents/agents/v1/events.proto` currently exposes only
  `AgentProvisioned`.

This is not yet the complete Agent Registry. Revision activation, proposal
streams, archive behavior, onboarding APIs, and onboarding UI remain future
work. The console home screen is currently an operator-console placeholder.

### Session event contracts

The repository has a broad `v1alpha1` Session event catalog under
`proto/trogonai/session/sessions/v1alpha1/`. It includes:

- canonical plan bytes and a digest in `execution_plan.proto`;
- one-time plan storage in `session_started.proto`;
- transcript, tool-call, execution-attempt, artifact, and lifecycle events in
  `events.proto`;
- parent and child facts such as `delegation_dispatched.proto` and
  `parent_linked.proto`.

Generated Rust types, event codecs, and local per-event semantic validation
live in `rsworkspace/crates/platform/trogonai-proto/`. The package remains
`v1alpha1`, and [ADR#0035](../adr/0035-session-store-decider-aggregate.md) is
still draft. Session commands, aggregate decisions, projections, the
coordinator, and the executing harness are not made complete by the event
catalog alone.

### Protocol and discovery building blocks

MCP over NATS already supports protocol transport, including tool listing and
tool calls, through `rsworkspace/crates/mcp/mcp-nats/`. ACP, A2A, the decider
substrate, observability, and ARD discovery provide other reusable building
blocks.

Several edge and bridge implementations also exist, but they do not yet
form one Session-integrated gateway layer:

- `a2a-gateway` contains a feature-gated live dispatch path that authenticates,
  policy-screens, audits, and forwards A2A gateway ingress to mapped agent RPC
  subjects;
- `mcp-nats-server` and `mcp-nats-stdio` bridge remote HTTP or local stdio MCP
  traffic onto the NATS MCP binding; and
- `trogon-gateway` verifies external source traffic and publishes raw source
  events to JetStream. Its current sources include Telegram, Slack, Discord,
  GitHub, GitLab, and other webhook or event producers.

These are not the same thing as the target control plane. MCP transport is not
a Tool Registry. ARD is a discovery surface and, by accepted
[ADR#0012](../adr/0012-ard-compatible-discovery-catalog.md), does not execute,
route, or authorize work. The existing A2A and MCP paths also do not establish
the target Session admission and execution contracts by themselves.

## Target state

### Registration produces immutable behavior revisions

An onboarding API, and eventually a product UI, should collect enough data to
create a complete Agent behavior declaration:

- name, description, and instructions;
- one concrete Agent implementation and its typed configuration;
- model selections and deterministic model parameters;
- declared tool, skill, memory, and delegate dependencies;
- runtime requirements and version metadata.

The exact wire shape is not settled. The shipped contract keeps instruction,
model, and dependency details inside runtime-owned settings. Draft
[ADR#0043](../adr/0043-agent-instructions-ownership-and-shape.md) preserves
that runtime-owned instruction shape and defers a generic platform instruction
abstraction. Draft
[ADR#0025](../adr/0025-agent-definition-data-ownership.md) and
[ADR#0031](../adr/0031-agent-implementation-and-session-plan.md) propose a
broader platform-readable configuration model. These views must be reconciled
before any conflicting platform fields become normative.

Registration does not copy every operational concern into an Agent revision.
The Agent revision owns behavior and dependency declarations. Separate planes
own live policy, grants, credentials, secrets, model-provider connections,
tool availability, memory content, budgets, evaluation, and scheduling. This
separation lets an operator revoke access or rotate a secret without silently
rewriting verified Agent behavior.

```text
Onboarding API or UI
        |
        v
validate typed behavior and dependency declarations
        |
        v
Agent Registry
  stable Agent identity
  immutable AgentConfiguration artifacts
  append-only AgentRevision history
  separate proposal and activation history
```

### Gateways mediate external edges

[ADR#0003](../adr/0003-ai-protocol-transport-taxonomy.md) reserves
**gateway** for a production edge component that accepts external traffic and
routes it inward. A transport bridge, registry, coordinator, policy service, or
outbound provider proxy is not automatically a gateway. The names below
describe logical boundaries. They do not require one universal proxy or one
deployment for every protocol.

The **Owns** column describes the mature target authority of each boundary.
Current foundations own only the responsibilities demonstrated by their
implemented contracts; target responsibilities still need decisions and code.

| Boundary | Status | Owns | Must not own |
| --- | --- | --- | --- |
| Product/API gateway | Accepted boundary in [ADR#0018](../adr/0018-connectrpc-gateway-for-browser-product-surfaces.md), but the console gateway is not implemented in this checkout | Browser or partner authentication, server-side browser auth sessions, CSRF and CORS controls, an explicit ConnectRPC method allowlist, mechanical translation, and signed calls onto the backbone | Domain decisions, workflow coordination, Agent definitions, or Agent Session state |
| Agent gateway / protocol edges | `a2a-gateway` is a current A2A-specific, feature-gated implementation foundation; ACP remote transport exists as a bridge. Shared Session admission is target state | Protocol connection and lifecycle handling, external caller identity, ingress policy enforcement, version and method validation, correlation, audit, and routing to an admitted platform operation | Agent behavior, revision lifecycle, choosing the next Agent, or creating and mutating Sessions outside Session admission |
| MCP gateway | MCP HTTP, stdio, and NATS bridges are current transport foundations. A managed, Session-integrated MCP edge remains target state | MCP connection and capability negotiation, transport mediation, peer and target binding, invocation authorization enforcement, correlation, and protocol telemetry | ToolDefinition versioning, externally owned tool schemas, undeclared tool selection, or the Session operation ledger |
| Source and channel ingress gateway | `trogon-gateway` is current | Webhook and event connection handling, source verification, raw payload fidelity, claim-check transport, stream provisioning, and publication | Conversation binding, prompt construction, Agent behavior, media interpretation, or tool execution |

The Agent gateway / protocol edges and MCP rows name logical capabilities, not
a requirement to add one generic Agent Gateway or merge A2A, ACP, and MCP into
one workload. Their transports, security postures, scaling, and failure modes
can justify separate deployments under
[ADR#0003](../adr/0003-ai-protocol-transport-taxonomy.md). Likewise, Session
integration is target state: the current A2A gateway routes to agent RPC
subjects, and the current MCP components route protocol messages. Neither
currently resolves an immutable `SessionExecutionPlan`.

The source ingress boundary is intentionally narrow. The channel bridge, not
`trogon-gateway`, normalizes a source event, resolves the endpoint, principal,
and conversation, and invokes an Agent adapter. Draft
[ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md) also keeps media
interpretation out of the gateway by assigning it to a dedicated consumer.

### Outbound model access is a separate mediation plane

The requested provider-gateway capability is called the **model access
service** in draft
[ADR#0032](../adr/0032-model-route-and-credential-binding.md). It is not a
gateway in the
[ADR#0003](../adr/0003-ai-protocol-transport-taxonomy.md) sense because it does
not accept general external ingress. It is a Session-scoped outbound proxy on
the model-call path.

If accepted and implemented, the model access service will own:

- validation of the active Session, execution attempt, grant, and resolved
  route on every request;
- exact provider-driver and translation-contract enforcement;
- provider request and streaming-response translation;
- ephemeral credential resolution through the secrets service or provider
  workload identity; and
- the enforcement point for usage, rate, budget, audit, and provider-call
  telemetry.

It must not choose or silently fail over the model, provider connection,
credential binding, provider account, or driver. Those values are admitted
before execution and committed to the `SessionExecutionPlan`. It must not
expose provider credentials to the Agent implementation, become the Agent
harness, or own policy definitions merely because it enforces them.

Under that draft model-access contract, `ModelAccessGrant` remains live,
attempt-scoped authorization rather than `SessionExecutionPlan` content.
Revocation fails the next model request with a typed, non-retryable model-access
failure and cannot substitute another route. Model-access audit records the
request and outcome. The draft ADRs do not yet decide whether that denial leaves
the harness running, ends the active `ExecutionAttempt`, or fails the `Session`.
A later accepted coordinator policy must choose that transition and record the
applicable message, attempt, or Session lifecycle fact. Model calls remain
outside the Session operation ledger.

[ADR#0032](../adr/0032-model-route-and-credential-binding.md) is draft and
blocked on the unresolved model-ownership contract. No complete model access
service exists in this checkout, so this is target architecture rather than
current behavior.

### Gateways depend on, but do not absorb, control-plane services

The surrounding services retain their own authority:

- the applicable Agent, implementation, tool, and model registries or catalogs
  own definitions, versions, digests, and lifecycle state;
- Session admission and coordination own plan resolution, Session creation,
  child-Session delegation, and durable execution state;
- identity, policy, and authorization services make access decisions that a
  gateway or proxy enforces;
- the platform secrets service remains the only OpenBao client and returns
  bounded secret material only to the authorized consumer; and
- event stores, audit streams, and observability services retain durable facts
  and telemetry.

This separation prevents an edge component from becoming a second registry,
orchestrator, secret store, and execution engine merely because traffic passes
through it.

```text
EXTERNAL INGRESS                         INTERNAL CONTROL AND EXECUTION

Browser or partner
  -> product/API gateway ----------------------+
A2A or ACP caller                              |
  -> Agent gateway / protocol edge ------------+-> service APIs and NATS
External source or channel                     |          |
  -> source ingress gateway -> channel bridge -+          v
                                                     Session admission
                                                            |
                                                            v
                                                   Session shell and
                                                   pinned implementation
                                                            |
                              +-----------------------------+----------------+
                              |                                              |
                              v                                              v
                    Session operation ledger                      Session-bound model endpoint
                              |                                              |
                              v                                              v
                    MCP gateway or adapter                       model access service
                              |                                              |
                              v                                              v
                  external MCP server or tool                         model provider

External MCP client
  -> MCP remote gateway or bridge -> platform MCP server

Cross-cutting: identity, live policy, SecretStore, audit, and tracing
```

### Session admission freezes one execution

Starting a Session resolves the immutable definition and the live environment
at one explicit boundary:

```text
Caller
  |
  v
appropriate product, Agent protocol, or channel edge
  |
  v
Session admission
  |-- pinned AgentRevision
  |-- exact Agent implementation version and typed settings
  |-- exact model selections and admitted provider routes
  |-- resolved tool, skill, delegate, memory, and workspace inputs
  |-- live policy and authorization checks
  v
immutable SessionExecutionPlan
  |
  v
durable Session shell <--> pinned Agent implementation or harness
  |                         |
  |                         +--> model access service --> model provider
  |                         +--> operation ledger --> MCP gateway or adapter
  |                                                    |
  |                                                    +--> MCP server or tool
  |
  +--> transcript, tool dispatch, delegation, state, events, and tracing
```

The `SessionExecutionPlan` is the reproducibility boundary. Draft ADRs propose
that it commit to the exact revision, implementation, models, routes,
dependencies, and inputs admitted for one Session. It contains references and
digests, not plaintext secrets or a frozen copy of live grants. Changing a
behavioral pin starts another Session or an explicit fork rather than mutating
the running plan.

[ADR#0032](../adr/0032-model-route-and-credential-binding.md) proposes the
model-route and credential portion, but it is draft and explicitly blocked on
the unresolved model-ownership contract. It is a target, not current runtime
behavior.

### The Session shell is generic, the implementation is concrete

Generic does not mean that one component can interpret arbitrary settings. The
platform Session shell should be role-neutral and implementation-neutral. It
owns durable identity, the plan, transcript, dispatch records, cancellation,
recovery evidence, and observability.

The pinned Agent implementation or harness remains concrete. It understands
its own typed configuration and owns prompt and context assembly, model turns,
tool sequencing, delegation requests, stopping behavior, and implementation
checkpoint semantics. This is the boundary that makes a configuration-driven
platform possible without pretending that LangGraph, ADK, a platform loop, or
another implementation all behave identically.

### Dynamic coordination creates child Sessions

When an Agent decides at runtime to delegate, its implementation requests a
delegation from the platform. After live authorization, the platform creates a
child Session. The child has its own pinned Agent revision, immutable plan,
authorization, transcript, lifecycle, and durable stream. Parent and child are
linked by explicit facts rather than by hidden in-process spawning.

The parent implementation may decide what to do after receiving the child
result. It does not select or mutate another Agent by bypassing Session
admission.

### Directional v1 boundary: deterministic workflows stay external

The frozen research record proposes this v1 split:

- deterministic work with no model decision is a tool or ordinary program;
- model-directed work inside one implementation is an Agent Session;
- fixed coordination across several Agents belongs to an external workflow
  engine that starts Sessions as durable steps;
- dynamic coordination belongs to authorized child-Session delegation.

The [agent-platform decision record](../research/agent-platform/decision-record.md)
is directional research, not an accepted ADR. It does not propose a first-party
workflow language or Workflow aggregate for v1. Treat external deterministic
orchestration as the current direction, not a final commitment. Any native
Workflow noun, or a final decision to keep workflows external, requires its own
accepted decision.

## Not a commitment or currently undecided

The following possibilities should not be read as promised architecture:

- **Agent Marketplace.** Discovery, publishing, version selection, and sharing
  could support a marketplace product later. No marketplace lifecycle,
  commercial model, trust model, or product commitment exists today. ARD
  discovery alone is not a marketplace.
- **Specific framework integrations.** LangGraph, Google ADK, n8n, Codex,
  Claude Code, OpenClaw, or another framework may become a pinned registered
  implementation, a Session adapter, an external workflow engine, or an
  external delegated system. No framework is selected as the generic runtime.
- **A first-party Workflow Orchestrator.** External deterministic orchestration
  is the v1 direction. A native orchestrator, DSL, or Workflow resource is not
  currently part of the core model.
- **Exact onboarding surface.** An onboarding API is necessary for the target
  product, and a UI is expected, but their endpoints, screens, validation flow,
  and rollout order are not decided here.
- **Complete registries for every concern.** Tool, model, skill, memory, and
  implementation catalogs are useful target capabilities. Their service
  boundaries, storage topology, and ownership contracts require separate
  decisions.
- **Draft ADR details.** ADRs 0025, 0031, 0032, 0035, and 0043 are proposals.
  Their concepts help describe the target, but acceptance and implementation
  are still required.

## The concise answer

Yes, the core architecture is what TrogonAI is building toward: immutable,
configuration-driven Agents executed through a durable generic Session shell
and a pinned concrete implementation, with external gateways, outbound model
mediation, orchestration, and live control planes kept separate.

The marketplace, chosen framework adapters, a first-party workflow engine, and
the exact registry and onboarding product surfaces remain possibilities rather
than promises.
