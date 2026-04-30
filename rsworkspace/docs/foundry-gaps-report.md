# What Trogon Needs from Microsoft Foundry Agent Service

## Executive Summary

Microsoft's Foundry Hosted Agents announcement validates the architectural direction trogon has already taken — per-agent identity, durable event processing, protocol-based agent communication, and horizontal scaling are all present in trogon today. However, the comparison surfaces four concrete gaps that matter for production use.

---

## What Trogon Already Has (No Action Needed)

Before covering gaps, it's worth being clear about what trogon has solved that Foundry is productizing:

**Per-agent identity and credential isolation.** Foundry's Entra Agent ID gives each agent its own identity so credentials are never shared between tenants. Trogon's `tok_{provider}_{env}_{id}` token scheme plus the vault system (completed this week with Infisical backend) does exactly this — agents never see real API keys, and a compromised agent cannot access another tenant's credentials.

**Activity Protocol / ACP.** Foundry lists "Activity Protocol" as one of its three supported protocols. Trogon has a full ACP implementation (`trogon-acp`, `trogon-acp-runner`) with session branching, authentication, tool permission callbacks, and stdio transport.

**Horizontal scaling.** Foundry offers scale-to-zero and elastic compute. Trogon's JetStream pull consumer model already distributes work across however many worker instances are running (`docker compose up --scale worker=3`). Adding auto-scaling is an infrastructure concern, not a code one.

**Observability.** Foundry uses OpenTelemetry. Trogon already has `acp-telemetry` with OTel traces and Datadog integration wired in.

**Exactly-once execution.** Foundry guarantees agents don't re-execute on restart. Trogon's promise store checkpoints state after every LLM turn and resumes from the last checkpoint on worker restart.

---

## What Trogon Needs

### 1. Streaming Responses — High Priority

Foundry streams tokens to the client in real time. Trogon runs the agent to completion and returns the full response at once. For the chat API (`POST /sessions/{id}/messages`), this means the user stares at a blank screen for the entire duration of the agent run.

The fix is Server-Sent Events (SSE) in the proxy layer. The worker already has retry/backoff infrastructure. What's needed is a streaming reply channel from worker back to proxy, and the proxy forwarding SSE events to the HTTP client. This is a UX-critical gap for any interactive use case.

### 2. Tool Approval Gates — High Priority

Foundry has permission callbacks before tool execution — a human or policy can approve or deny a tool call before it runs. Trogon has this pattern for credential writes (`trogon-vault-approvals` with Slack approval workflow) but not for tool calls themselves.

If an agent is about to execute a shell command, merge a pull request, or post to a production channel, there is currently no way to intercept that and ask for confirmation. The NATS request-reply pattern already in the codebase makes this straightforward to add: the worker publishes a `tool.approval_request` event and waits for a reply before executing.

### 3. Token and Cost Tracking — Medium Priority

Foundry meters everything. Trogon measures nothing. Every Anthropic API response includes `usage.input_tokens` and `usage.output_tokens`. Trogon currently discards these.

The transcript system already exists and is append-only. Adding a `TokenUsage` entry type to the transcript — and aggregating it per session and per tenant — is low implementation cost with high operational value. Without this, there is no visibility into which agents, sessions, or tenants are consuming the most resources.

### 4. Multi-Agent System in Production — Medium Priority

This is the most significant structural gap. Foundry runs multi-agent pipelines as a core feature. Trogon has the complete design implemented: `trogon-registry` (live agent discovery with TTL heartbeats), `trogon-router` (LLM-powered dynamic routing), `trogon-actor` (stateful entity actors with sub-agent spawning), and `trogon-transcript` (append-only audit trail across agent hops). None of it is deployed.

The code is ready. The gap is deployment and wiring, not architecture.

---

## What Foundry Has That Trogon Should Not Rush To Copy

**Per-session VM isolation.** Foundry spins up a hypervisor-isolated VM per agent session. This matters when agents execute untrusted code or write to the filesystem. Trogon's agents today call well-defined APIs (GitHub, Linear, Slack) — they don't run arbitrary code. VM isolation would be over-engineering for the current use case. Revisit when trogon adds a code execution tool.

**Versioned API endpoints.** Foundry has `/v1/`, `/v2/` routing with weighted rollouts. Trogon is a single-org internal platform today. API versioning adds overhead without benefit until there are external consumers who can't absorb breaking changes.

**Persistent session filesystem.** Foundry guarantees a per-session filesystem that survives scale-down. Trogon uses NATS KV for session state. This is adequate for single-node development but requires a 3-node NATS cluster for production durability. The fix is operational (deploy a cluster), not architectural.

---

## Recommended Implementation Order

| Priority | Feature | Effort | Impact |
|---|---|---|---|
| 1 | Token/cost tracking per session | Low — one afternoon | Immediate operational visibility |
| 2 | Streaming SSE in proxy | Medium — 2–3 days | Critical UX for interactive chat |
| 3 | Tool approval callbacks | Medium — 2–3 days | Security gate for sensitive tools |
| 4 | Deploy multi-agent system | Low code, medium ops | Unlocks the biggest architectural investment already made |

The vault work completed this week is the foundation for items 3 and 4 — every agent now has a clean identity and credential isolation model to build approval flows on top of.
