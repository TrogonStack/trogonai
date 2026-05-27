# A2A Client SDK — Contract (DRAFT)

> **Status:** DRAFT — for human review before implementation.
>
> Companion specs (not yet written): [`sts-exchange.md`](./sts-exchange.md), [`act-chain.md`](./act-chain.md), [`registry.md`](./registry.md), [`overview.md`](./overview.md).

---

## Purpose

The A2A client SDK is the **only supported path** for agent-to-agent calls and for agent-initiated MCP tool calls. Application code must not assemble NATS subjects, attach caller JWTs, or parse `act_chain` claims by hand.

Developers configure **who they are** (workload attestation + own `agent_id`) and **what they want to do** (`target_agent`, `purpose`, payload). The SDK owns:

1. Registry lookup of the callee.
2. STS token exchange (`aud` = target hop).
3. Gateway ingress publish with the mesh token.
4. Inbound verification of mesh tokens and `act_chain` on the server side.
5. OpenTelemetry attribution on every hop.

The secure path becomes the path of least resistance: a new agent should be writable in **≤ 50 lines** and get correct identity propagation for free.

---

## Dependencies on other blocks

Several behaviors in this contract **cannot ship** until upstream identity primitives exist. Call these out during review and implementation planning.

| Dependency | Block | What the SDK needs |
|------------|-------|-------------------|
| **STS token exchange** | [2.1](../PENDING_TODO.md#21-security-token-service-sts) | `call()` must exchange bootstrap/mesh token for `aud=target` before publish. Without STS, SDK can only wrap today's connect-time JWT (bootstrap mode). |
| **Agent registry lookup** | [1.1](../PENDING_TODO.md#11-agent-registry) | Resolve `target_agent` → allowed workloads, gateway subject, lifecycle state. STS also uses registry to bind `agent_id` ↔ `wkl`. |
| **`act_chain` schema + verification** | [2.2](../PENDING_TODO.md#22-actor-chain-act_chain) | `serve()` must verify chain depth, loop detection, and expose typed `Caller`. |
| **`purpose` claim** | [2.3](../PENDING_TODO.md#23-intent--purpose-claim) | Propagated on `call()`; validated against registry `allowed_purposes`. |
| **Workload attestation (SVID)** | [1.2](../PENDING_TODO.md#12-workload-attestation-spiffe--spire) | Constructor `svid_source` feeds STS `actor_token`. Dev fallback: file-based SVID; prod: SPIRE. |
| **Block 0 ADRs** | [0](../PENDING_TODO.md#block-0--cross-cutting-decisions-that-gate-everything-else) | Tenancy model, bootstrap-vs-mesh, TTL/`aud` discipline affect constructor defaults and error semantics. |
| **Gateway egress mint** | [2.4](../PENDING_TODO.md#24-gateway-egress-mint-dont-propagate) | Transparent to SDK on A2A ingress; relevant when callee is an MCP backend reached via `trogon-mcp-gateway`. |

**Bootstrap mode (interim):** Until STS ships, the SDK MAY operate in `SdkMode::BootstrapOnly`: attach connect-time [`MintedUserJwt`](../../rsworkspace/crates/a2a-auth-callout) via gateway ingress (today's `a2a_nats::Client::routing_via_gateway_ingress`). Mesh mode becomes default once Block 2.1 is accepted.

---

## Relationship to existing crates

The verified branch (`yordis/agentgateway`) already contains transport and perimeter auth. This contract defines a **new identity-aware layer** that sits above them.

| Crate | Role today | SDK relationship |
|-------|------------|------------------|
| `a2a-auth-callout` | Connect-time NATS User JWT (`sub`, `aud`, caller ACL) | Bootstrap credential; SDK refreshes/replaces via STS in mesh mode. |
| `a2a-nats` | JSON-RPC client + agent `Bridge` over `{prefix}.agent.*` / `{prefix}.gateway.*` | **Transport substrate** — SDK wraps `Client` / `Bridge`, does not reimplement wire codec. |
| `a2a-gateway` | Ingress policy, forwarding `gateway.*` → `agent.*` | SDK always targets `{prefix}.gateway.{target}.{method}`; never `agent.*` directly. |
| `a2a-nats-discovery` | KV + `discover.*` AgentCard catalog | SDK uses for target resolution until `mcp.registry.agent.lookup` (Block 1.1) lands; then prefers registry API with AgentCard as cache. |
| `a2a-nats-agent` | Process wrapper around agent `Bridge` | `serve()` replaces hand-rolled `a2a-nats-agent` + manual JWT handling. |
| `a2a-bridge` | HTTPS → NATS gateway ingress | Bridge callers use the same SDK `call()` from non-NATS runtimes. |
| `a2a-types` | Protobuf/JSON A2A message types | Typed request/response surfaces for `call()` and handler methods. |
| `trogon-mcp-gateway` | MCP `{prefix}.gateway.request.*` ingress | SDK exposes `call_mcp(server_id, method, params, purpose)` — same identity pipeline, different subject grammar. |

**Planned crate names (Rust):** `a2a-sdk` (public API), with thin re-exports from `a2a-nats` for wire types. TypeScript: `@trogon/a2a-sdk` under `tsworkspace/` (not yet in tree). Python: `trogon_a2a` (third priority).

---

## Constructor contract (cross-language)

All language bindings expose the same configuration surface. Names are idiomatic per language; semantics are identical.

```rust
// Rust sketch — not implemented
A2aSdk::builder()
    .agent_id("acme/oncall-agent")           // this workload's registry id
    .svid_source(SvidSource::file("/var/run/svid"))  // or SpireAgent::default()
    .sts(StsEndpoint::nats("mcp.sts.exchange"))    // Block 2.1 — subject or URL
    .registry(RegistryEndpoint::nats("mcp.registry.agent.lookup")) // optional
    .nats(NatsConfig::from_env())
    .audience_policy(AudiencePolicy::StrictTarget)   // default
    .build()
```

### Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| **`agent_id`** | yes | This workload's registered agent id. Used as STS `sub` context and appended to `act_chain` on outbound calls. |
| **`svid_source`** | yes | Provides current workload attestation (SPIFFE JWT-SVID, file PEM, or documented non-SPIFFE sentinel for dev). Rotates transparently; SDK re-reads before each STS exchange. |
| **`sts_endpoint`** | yes | Where to exchange tokens. NATS subject `mcp.sts.exchange` (default) or HTTP URL per ADR 0004. |
| **`registry_endpoint`** | no | Enables self-introspection (`allowed_tools`, `allowed_purposes`) and explicit lookup before `call()`. When omitted, SDK falls back to AgentCard discovery (`a2a.discover.{id}` / KV). |
| **`nats` / transport** | yes | Connection used for gateway ingress and agent `serve()` subscription. |
| **`audience_policy`** | no (default: strict) | Controls STS `audience` selection — see below. |
| **`mode`** | no (default: mesh) | `Mesh` (STS per hop) or `BootstrapOnly` (interim — see Dependencies). |

### `AudiencePolicy`

| Variant | Behavior |
|---------|----------|
| **`StrictTarget`** (default) | `aud` = resolved target agent id (A2A) or MCP `server_id` (MCP calls). Hard-fail if STS returns a token whose `aud` does not match. |
| **`GatewayScoped`** | `aud` = local gateway workload id; for deployments where gateway re-mints on egress (Block 2.4). |
| **`PermissiveDev`** | Logs `aud` mismatches; does not reject. **Never enabled in production CI.** |

---

## Method: `call(target_agent, payload, purpose)`

Outbound agent-to-agent (or agent-to-MCP-server) invocation. The developer passes a **typed payload**; the SDK returns a **typed response**. No raw NATS handles escape the API.

### Signature (language-neutral)

```
call(
  target: AgentId | McpServerId,
  method: RpcMethod,          // e.g. message/send, tools/call
  payload: TRequest,
  purpose: Purpose,            // enumerated or free-form per Block 2.3 ADR
) -> Result<TResponse, SdkError>
```

Rust maps `TRequest`/`TResponse` to `a2a_types` structs (e.g. `SendMessageRequest` → `SendMessageResponse`). TypeScript and Python use generated bindings from the same schema.

### Pipeline (in order)

```
┌─────────┐    ┌──────────┐    ┌─────┐    ┌─────────────────────┐    ┌────────┐
│ Lookup  │───▶│ Exchange │───▶│ Send│───▶│ Receive / deserialize│───▶│ Return │
│ target  │    │ at STS   │    │ via │    │ typed TResponse      │    │        │
│         │    │ aud=target    │ gw  │    │                      │    │        │
└─────────┘    └──────────┘    └─────┘    └─────────────────────┘    └────────┘
```

1. **Lookup** — Resolve `target_agent` via registry (`mcp.registry.agent.lookup`) or AgentCard catalog. NACK → `SdkError::RegistryUnknown`. Revoked/deprecated → fail before exchange.
2. **Exchange** — Present bootstrap/mesh token + SVID + `purpose` to STS with `audience = target`. STS appends this agent to `act_chain` (Block 2.2). Failure → typed STS errors (below).
3. **Send** — NATS `request` on ingress subject:
   - A2A: `{prefix}.gateway.{target_agent}.{method_dots}` (e.g. `a2a.gateway.planner.message.send`).
   - MCP: `{prefix}.gateway.request.{server_id}.{method_dots}` (e.g. `mcp.gateway.request.github.tools.call`).
   - Attach mesh token as `A2a-Caller-Jwt` header (same wire name as today; **token contents** change in mesh mode).
4. **Receive** — Await reply inbox; deserialize JSON-RPC result into `TResponse`. Map JSON-RPC errors (-32800 gateway deadline, -32603 internal, `-32107 approval_required`) to `SdkError`.
5. **Return** — Typed value; no token or chain data leaked unless caller opts into `CallMetadata` (trace id, exchange id for audit correlation).

Streaming methods (`message/stream`, `tasks/resubscribe`) return typed event streams; only the bootstrap RPC runs the exchange step.

### Typed return on the server side

Handlers return the same `TResponse` types the client expects (e.g. `SendMessageResponse`). `serve()` injects `Caller` as the first argument on every callback — extending today's [`A2aHandler`](../../rsworkspace/crates/a2a-nats/src/agent/handler.rs), not raw JSON.

---

## Method: `serve(handler)`

Inbound server surface for an agent workload. Replaces manual `a2a-nats-agent` + gateway header parsing.

### Signature

```
serve(
  handler: impl AgentHandler,   // typed callbacks per A2A method
  options: ServeOptions,        // agent_id, concurrency, JetStream provisioning
) -> Result<ServeHandle, SdkError>
```

### Pipeline (per inbound request)

1. **Verify inbound token** — Validate JWT signature (STS issuer JWKS), `exp`/`iat`, `aud == this_agent_id` (or gateway-scoped per policy).
2. **Verify `act_chain`** — Parse claim; enforce max depth (default 8), loop detection `(agent_id, wkl)` duplicates, registry resolution of each entry (Block 2.2). Fail → `SdkError::ChainTooDeep` / `ChainLoop` surfaced to client as JSON-RPC error.
3. **Build `Caller`** — Typed struct (below) passed to handler.
4. **Invoke handler** — Dispatch by JSON-RPC method; serialize typed response.
5. **Audit** — SDK span + optional audit hook (`caller.chain`, `purpose`); gateway audit unchanged.

Default subscription: `{prefix}.agent.{agent_id}.>` via queue group (same as today's `a2a-nats` `Bridge`).

---

## Typed `Caller` struct

Parsed from verified mesh token claims. Immutable for the lifetime of one request.

| Field | Type | Source claim | Description |
|-------|------|--------------|-------------|
| **`originator`** | `Originator` | `act_chain[0]` | First hop — usually human `user_sub` or batch job sentinel. |
| **`chain`** | `Vec<ChainEntry>` | `act_chain` | Ordered oldest → newest; last entry is immediate caller. |
| **`purpose`** | `Purpose` | `purpose` | Why this call was made; may be refined down-chain. |
| **`agent_id`** | `AgentId` | immediate caller's `agent_id` | Who directly invoked this agent. |
| **`workload_id`** | `WorkloadId` | `wkl` | SPIFFE ID of immediate caller workload. |
| **`session_id`** | `SessionId` | `session_id` | Correlates multi-call workflows. |
| **`token_expires_at`** | `Timestamp` | `exp` | For handlers that need to bound downstream work. |

### `ChainEntry`

| Field | Type |
|-------|------|
| `sub` | Subject id (user or service) |
| `agent_id` | Optional agent id |
| `wkl` | Workload id |
| `iat` | Hop timestamp |

### Helpers (all languages)

| Method | Returns | Semantics |
|--------|---------|-----------|
| `is_originator_human()` | `bool` | `originator.auth_method == Oidc` or `wkl == "human"`. |
| `chain_depth()` | `usize` | `chain.len()` |
| `contains_agent(id)` | `bool` | Any hop matches |
| `immediate_caller()` | `ChainEntry` | `chain.last()` |
| `root_sub()` | `Subject` | `originator.sub` — for audit "on behalf of" |
| `purpose_matches(set)` | `bool` | Enum membership check for policy gates in handler |

Handlers **must not** parse JWTs or NATS headers for identity; use `Caller` only.

---

## Telemetry

Every `call()` and every inbound `serve()` dispatch starts an OpenTelemetry span (via `trogon-telemetry` in Rust).

### Required span attributes

| Attribute | Example | When |
|-----------|---------|------|
| `agent.id` | `acme/oncall-agent` | Always — this SDK instance's agent id |
| `agent.target.id` | `acme/planner` | Outbound `call()` only |
| `agent.chain.depth` | `3` | Always — from verified token |
| `agent.purpose` | `triage_incident` | Always |
| `agent.call.direction` | `outbound` / `inbound` | Always |

Optional: `agent.chain.root_sub`, W3C trace propagation, `sts.exchange.latency_ms`.

---

## "Don't roll your own" enforcement

Direct NATS publishes bypass identity, policy, and audit. CI must flag them in agent application code (not in SDK or gateway crates).

### Lint rules

| Rule id | Pattern | Scope |
|---------|---------|-------|
| `a2a-sdk/no-raw-gateway-publish` | Publish/request to `{prefix}.gateway.{agent}.>` or `{prefix}.gateway.request.>` | Agent app crates, examples, services |
| `a2a-sdk/no-caller-jwt-header` | Setting `A2a-Caller-Jwt` / `X-A2a-Caller-Id` manually | Same |
| `a2a-sdk/no-direct-agent-subject` | Publish to `{prefix}.agent.{id}.>` from client code | Same — clients must use SDK `call()` |

Allowlist: SDK, gateway, and transport crates; integration tests via `#![allow(a2a_sdk_lints)]`. Implement as Clippy/dylint (Rust), ESLint (TS), or semgrep (Python).

---

## Error model

Typed errors — no stringly-typed `anyhow` at the SDK boundary. Each variant maps to a stable `code` for logging and client JSON-RPC surfacing where applicable.

| Error | Code | When | Client-visible |
|-------|------|------|----------------|
| **`AudMismatch`** | `aud_mismatch` | Token `aud` ≠ expected target after STS or on inbound verify | yes (-32001) |
| **`ChainTooDeep`** | `chain_too_deep` | `act_chain.len()` > max (default 8) | yes (-32002) |
| **`ChainLoop`** | `chain_loop` | Duplicate `(agent_id, wkl)` in chain | yes (-32003) |
| **`RegistryUnknown`** | `registry_unknown` | Target or chain entry not in registry | yes (-32004) |
| **`ApprovalRequired`** | `approval_required` | Gateway/CEL returned `-32107` with approval handle | yes (-32107) + `approval_url` |
| **`StsUnavailable`** | `sts_unavailable` | STS timeout/outage | yes (-32005) |
| **`PurposeNotAllowed`** | `purpose_not_allowed` | Purpose ∉ registry `allowed_purposes` | yes (-32006) |
| **`SvidInvalid`** | `svid_invalid` | Attestation failed before exchange | no (retry locally) |
| **`Transport`** | `transport` | NATS/I/O failure | yes (-32099) |

Each error carries: `message`, optional `trace_id`, optional `chain_depth`, optional `approval` struct `{ request_id, subject, expires_at }`.

---

## Migration: raw NATS → SDK

For agents already using `a2a_nats::Client::routing_via_gateway_ingress(caller_jwt)` or hand-rolled publishes.

### Phase 0 — Inventory (no code change)

1. List agents publishing to `a2a.gateway.*` or `mcp.gateway.request.*`.
2. Assign each workload an `agent_id`; register in catalog (Block 1.1 backfill).
3. Enable `MCP_GATEWAY_AGENT_IDENTITY=shadow` (Block 6) — log violations only.

### Phase 1 — Wrap client

```rust
// Before
let client = Client::new(config, agent_id, nats, js)
    .routing_via_gateway_ingress(caller_jwt);
client.message_send(SendMessageRequest { ... }).await?;

// After
let sdk = A2aSdk::builder()
    .agent_id("acme/oncall-agent")
    .svid_source(SvidSource::from_env())
    .sts(StsEndpoint::from_env())
    .mode(SdkMode::BootstrapOnly)  // until STS live
    .build()?;
sdk.call(
    TargetAgent::new("acme/planner")?,
    Method::MessageSend,
    SendMessageRequest { ... },
    Purpose::new("handoff")?,
).await?;
```

Remove all `MintedUserJwt` handling from application code; SDK owns refresh.

### Phase 2 — Wrap server

Replace `A2aHandler` impls that ignore caller identity with handlers taking `Caller`:

```rust
async fn message_send(&self, caller: &Caller, req: SendMessageRequest) -> ... {
    if !caller.purpose_matches(&[Purpose::Triage]) {
        return Err(HandlerError::deny("purpose"));
    }
    // ...
}
```

Run `serve(sdk_handler)` instead of `a2a-nats-agent` + custom bridge wiring.

### Phase 3 — Mesh mode

Flip `SdkMode::Mesh` when STS Block 2.1 is deployed. Verify shadow-mode audit shows zero `aud_mismatch` / `chain_*` violations for N days (Block 6 cutover).

### MCP tool calls

Agents calling MCP tools today via raw `mcp-nats` client:

```rust
// Before — forbidden after lint enabled
nats.request("mcp.gateway.request.github.tools.call", payload).await?;

// After
sdk.call_mcp(
    McpServerId::new("github")?,
    McpMethod::ToolsCall,
    params,
    Purpose::new("open_issue")?,
).await?;
```

---

## Language priorities

| Priority | Language | Package | Notes |
|----------|----------|---------|-------|
| 1 | **Rust** | `a2a-sdk` in `rsworkspace/` | First implementation; wraps `a2a-nats`, `a2a-auth-callout`, `trogon-telemetry`. |
| 2 | **TypeScript** | `@trogon/a2a-sdk` in `tsworkspace/` | Matches frontend/agent hosts; NATS.ws transport. |
| 3 | **Python** | `trogon_a2a` | Agent ecosystem (LangChain, etc.); code-gen from same IDL as Rust. |

Feature parity order: `call()` + `Caller` + errors → `serve()` → streaming → `call_mcp()` → approval resume flow (Block 5).

---

## Quickstart sketch (target ≤ 50 lines)

```rust
use a2a_sdk::{A2aSdk, Purpose, TargetAgent, Method};
use a2a_types::SendMessageRequest;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sdk = A2aSdk::from_env()?;

    let reply = sdk.call(
        TargetAgent::new("acme/planner")?,
        Method::MessageSend,
        SendMessageRequest { /* ... */ },
        Purpose::new("schedule_meeting")?,
    ).await?;

    println!("task id: {:?}", reply.task_id());
    Ok(())
}
```

Server:

```rust
sdk.serve(MyHandler).await?.wait().await?;
```

---

## Open questions

1. **Single SDK for A2A + MCP?** This contract merges both ingress grammars under one type. Should MCP be a separate `McpSdk` crate sharing only STS/registry clients?
2. **Registry vs AgentCard during transition.** When lookup sources disagree, which wins? Proposal: registry authoritative; AgentCard is cache with TTL.
3. **Handler trait migration.** Add `Caller` to existing `A2aHandler` (breaking) vs new `IdentityA2aHandler` trait with default impls?
4. **Approval resume API.** Block 5 `-32107` requires `sdk.resume(approval_token)` — in scope for v1 or v2?
5. **Bootstrap token refresh.** Does SDK call auth-callout on reconnect only, or proactively at `exp - skew`?
6. **Cross-language `Purpose` type.** Free-form string vs generated enum from registry manifest?
7. **Lint false positives.** Generated subject constants in tests — sufficient allowlist, or path-based excludes?
8. **Gateway subscription model.** Should `serve()` ever bind `gateway.*` directly (trusted gateway forwards identity headers), or always `agent.*`?
9. **Offline dev without SPIRE/STS.** Documented `LocalDevBundle` (file SVID + mock STS in-process) — part of SDK or separate test crate?

---

## Default purpose handling

When `Client::call` receives `purpose: None`, the SDK **does not** forward an absent or caller-supplied body field. It resolves purpose from the **calling agent's** registry record (`allowed_purposes` for `Client::agent_id`):

| Registry `allowed_purposes` | SDK behavior |
|----------------------------|--------------|
| **Empty** | Synthesize `purpose:default` and send that value on the STS exchange. |
| **Exactly one** | Use that sole purpose automatically. |
| **Two or more** | Return `SdkError::PurposeRequired`; the caller must pass an explicit `Purpose`. |

Explicit `Some(purpose)` always wins and is validated downstream by STS against the caller's `allowed_purposes`. Handlers still read the resolved purpose from `Caller::purpose` on the serve side.

---

## Quickstart

See [`rsworkspace/crates/trogon-a2a-sdk/README.md`](../../rsworkspace/crates/trogon-a2a-sdk/README.md) for runnable commands (`echo_agent` example), registry + STS wiring, and live test invocation.

Rust client/server sketches remain in [Quickstart sketch (target ≤ 50 lines)](#quickstart-sketch-target--50-lines) above.

---

## Telemetry attributes

OpenTelemetry spans are emitted via the global `trogon-a2a-sdk` tracer.

### Outbound `Client::call`

| Span name | Attributes |
|-----------|------------|
| `a2a.call` | `agent.id`, `agent.target.id`, `agent.purpose`, `agent.call.direction=outbound` |
| `a2a.call.lookup` | (child; inherits trace context) |
| `a2a.call.exchange` | (child) |
| `a2a.call.chain` | `agent.chain.depth` (from minted mesh token) |
| `a2a.call.send` | (child) |

### Inbound `serve` dispatch

| Span name | Attributes |
|-----------|------------|
| `a2a.serve.dispatch` | `agent.id`, `agent.chain.depth`, `agent.purpose`, `agent.call.direction=inbound` |

Depth and purpose on the serve span come from the verified mesh token / parsed `Caller`, not from client headers.

---

## Acceptance criteria (from Block 3)

- [ ] Contract reviewed; open questions triaged into ADRs or implementation issues.
- [ ] New agent ≤ 50 lines using `call()` + `serve()` with no manual JWT/subject code.
- [ ] CI lint blocks raw gateway publishes in agent application crates.
- [ ] OpenTelemetry attributes present on integration test traces.
- [ ] Migration guide validated against at least one existing `a2a-nats` agent.
