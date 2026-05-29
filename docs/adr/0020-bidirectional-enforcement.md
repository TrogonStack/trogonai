# ADR 0020: Bidirectional Enforcement (Server to Client)

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block C item 7 (Bidirectional enforcement); unblocks Phase 4 callback ingress consumer, callback policy bundle section, inverted SpiceDB schema, and callback audit envelope fields (Wire-Format Pin 7) |
| **Related** | [bidirectional-enforcement.md](../identity/bidirectional-enforcement.md) · [overview.md](../identity/overview.md) · [act-chain.md](../identity/act-chain.md) · [adaptive-access.md](../identity/adaptive-access.md) · [sts-exchange.md](../identity/sts-exchange.md) · [reference-audit-envelope.md](../identity/reference-audit-envelope.md) · ADR 0005 (callback audience URI) · ADR 0008 (CEL) · ADR 0009 (reply correlation) · ADR 0011 (NATS auth callout / subscription scoping) · ADR 0012 (rate limits) · ADR 0013 (hierarchical merge) · `MCP_GATEWAY_PLAN.md` § NATS Subject Topology, § SpiceDB Integration Model, Wire-Format Pin 7 |

## Context

The Trogon MCP gateway today optimizes for **client to server** traffic: callers publish on `mcp.gateway.request.{server_id}.{method}`, the gateway authenticates, authorizes, redacts, audits, and forwards to `mcp.server.{server_id}.{method}`. NATS subject ACLs guarantee that edge clients cannot reach backend MCP servers without passing through the gateway ([ADR 0011](0011-nats-auth-callout.md), `MCP_GATEWAY_PLAN.md` § Subject ACL per principal).

MCP is **bidirectional**. After `initialize`, an MCP **server** may initiate JSON-RPC toward the **client** for capabilities the client advertised — sampling, elicitation, roots discovery, and lifecycle notifications. In `mcp-nats`, backend servers publish these on `mcp.client.{client_id}.{method}`; only the gateway may subscribe. Without an explicit enforcement design on that path, callback traffic becomes a **policy bypass**: a compromised backend that legitimately received client to server traffic can exfiltrate session context via `sampling/createMessage`, phish the user via `elicitation/create`, or fingerprint workspace layout via `roots/list` while the gateway audits only the forward path.

NATS ACLs already prevent a backend from publishing to **arbitrary** clients' edge subjects — it can only reach `mcp.client.>`, and only the gateway subscribes. That perimeter control is necessary but not sufficient. Enforcement adds **authorization inside the trusted backend zone**: even a connected server may only callback the client bound to the active MCP session, only with permitted capabilities, and only with auditable, redacted payloads. Subscription scoping at CONNECT time ([ADR 0011](0011-nats-auth-callout.md), `tests/subscription_scoping.rs`) constrains which callback subjects an edge client may **subscribe** to; this ADR pins the **policy model** for what the gateway allows to **relay** on the server-initiated path.

The normative design context is [bidirectional-enforcement.md](../identity/bidirectional-enforcement.md). That paper spec defines subject layout, inverted SpiceDB tuples, default policy matrix, HITL integration, failure modes, and callback audit envelope fields. This ADR records the durable decision, rationale, and trade-offs; implementation follows the spec unless explicitly overridden here.

**What stays undecided and breaks downstream work if this ADR is not accepted:**

- Phase 4 callback queue-group consumer (`mcp.client.>`, group `mcp-gateway-callbacks`) lacks authorization semantics — backends could reach any session-bound client with any MCP callback method.
- Policy bundle authors cannot rely on a stable `callback:` rule section or inverted `spicedb.check` tuple shape.
- SpiceDB schema for `list_roots`, `notify`, `mcp_client`, and `mcp_session` resource types remains proposed-only.
- Audit consumers cannot distinguish callback decisions from request-direction traffic (Pin 7 `direction` field unset).
- Integration tests for deny-default sampling and allow-with-audit elicitation have no canonical expected behavior.
- HITL parking for high-risk sampling/elicitation on the callback leg lacks a pinned decision record tied to Block C.

### Threat model summary

| MCP method | Abuse without gateway policy | Impact |
|---|---|---|
| `sampling/createMessage` | Exfiltrate context via smuggled prompts; force client LLM inference off audit path | Data exfiltration, shadow inference cost |
| `elicitation/create` | Phishing dialog mimicking host application UI | Credential theft, social engineering |
| `roots/list` | Fingerprint filesystem layout from returned root URIs | Privacy / reconnaissance |
| `notifications/cancelled` | Cancel in-flight client operations unexpectedly | Integrity / availability of long tasks |
| `notifications/progress` | Spam progress tokens; timing side channel | DoS, information leakage |

Block C item 7 priority methods are `sampling/createMessage`, `elicitation/create`, and `roots/list`. Notifications share the same enforcement **class** (authorize, redact, audit, forward) but are lower initial priority per the gateway plan.

### Relationship to subscription scoping

Two complementary controls apply to server to client traffic:

| Control | Plane | Owner | Question answered |
|---|---|---|---|
| **Subscription scoping** | NATS CONNECT / subscribe ACL | Auth callout ([ADR 0011](0011-nats-auth-callout.md)) | May this **client** subscribe to `mcp.gateway.callback.{my_client_id}.>`? |
| **Bidirectional enforcement** | Gateway message path | Policy engine (this ADR) | May this **backend** relay this **callback** to the session-bound client? |

Test scaffold `rsworkspace/crates/trogon-mcp-gateway/tests/subscription_scoping.rs` covers the first row (perimeter subscribe templates, cross-client deny, notification wildcard scope). Callback authorization tests land in Phase 4 alongside this ADR's policy matrix.

---

## Decision

**The gateway intercepts, authorizes, redacts, and audits every server-initiated MCP method on `mcp.client.{client_id}.>` before relaying to `mcp.gateway.callback.{client_id}.{method}`.** The same policy engine, CEL interpreter, SpiceDB client, approvals module, and audit publisher as client to server traffic apply; callback traffic uses a **separate rule bundle section**, **inverted SpiceDB resource shape**, and **session-bound identity** from the MCP session established at `initialize`.

### End-to-end callback path (pinned)

```text
Backend publish          Gateway (policy chokepoint)              Edge client
mcp.client.{id}.*   -->  session bind + CEL + SpiceDB + redact  -->  mcp.gateway.callback.{id}.*
     ^                         |   audit direction=callback              ^
     |                         v                                         |
     +---- JSON-RPC reply on _INBOX.server.{nuid_s} <---- client reply --+
```

Gateway terminates reply correlation on both legs, identical to request-direction flow ([ADR 0009](0009-reply-correlation.md)). Queue group `mcp-gateway-callbacks` on `mcp.client.>` is independent from `mcp-gateway` on `mcp.gateway.request.>` for fairness ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Wire-Format Pin 3).

### Subject layout (mirror of request direction)

| Leg | Subject pattern | Principal |
|---|---|---|
| Backend ingress (gateway subscribes) | `mcp.client.{client_id}.{method_path}` | Backend publishes; gateway only subscriber |
| Edge egress (client subscribes) | `mcp.gateway.callback.{client_id}.{method_path}` | Gateway publishes after authorization |

`{method_path}` uses `mcp-nats` convention: `/` to `.`, camelCase to snake_case (e.g. `sampling/createMessage` to `sampling.create_message`). `{client_id}` comes from **session KV binding** at `initialize`, not from payload — subject segment mismatch against session rejects before policy evaluation (`session_client_mismatch`, proposed JSON-RPC `-32116`).

Priority Block C methods and subjects (default `MCP_PREFIX=mcp`):

| MCP method | Backend subject | Edge subject |
|---|---|---|
| `sampling/createMessage` | `mcp.client.{client_id}.sampling.create_message` | `mcp.gateway.callback.{client_id}.sampling.create_message` |
| `elicitation/create` | `mcp.client.{client_id}.elicitation.create` | `mcp.gateway.callback.{client_id}.elicitation.create` |
| `roots/list` | `mcp.client.{client_id}.roots.list` | `mcp.gateway.callback.{client_id}.roots.list` |

### Reverse-direction CEL gate

Policy bundles declare callback rules under a dedicated **`callback:`** section ([bidirectional-enforcement.md](../identity/bidirectional-enforcement.md) § Authorization policy). The gateway evaluates **only** that section when handling messages dequeued from `mcp.client.>`.

**CEL context binding (proposed Pin 8 extension):**

| Variable | Semantics |
|---|---|
| `mcp.direction` | `"callback"` — selects reverse-direction rule bundle (distinct from `"request"`) |
| `callback.server_id` | From session / NATS subject |
| `callback.client_id` | From session / NATS subject |
| `callback.method` | JSON-RPC method string |
| `callback.is_notification` | `true` when method starts with `notifications/` |
| `jwt.act_chain` | Full chain snapshot from session (unchanged) |
| `chain.originator().sub` | User principal for inverted SpiceDB resource checks |

**Client-egress semantics:** On the authorized path, the gateway's final publish to `mcp.gateway.callback.*` is the **client egress leg** — transport egress toward the edge zone. CEL rules and audit envelopes use the wire value **`callback`**, not `egress`, to align with Pin 7 and audit subject grammar `mcp.audit.{outcome}.callback.{method_root}`. Redaction follows the same inbound/outbound split as request direction with roles inverted: **inbound** redaction on server-published params before edge publish; **outbound** redaction on client reply before server inbox relay.

Example bundle guard (illustrative):

```cel
mcp.direction == "callback"
  && callback.method == "sampling/createMessage"
  && spicedb.check("mcp_server:" + callback.server_id, "sample", "user:" + chain.originator().sub)
```

Hierarchical merge ([ADR 0013](0013-hierarchical-policy-merge.md)) applies independently to `callback:` rules — org/tenant/server/method layers, deny-sticky, default deny when no allow side matches.

### SpiceDB resource shape (callback direction)

Client to server checks `user:{sub}` acting on `mcp_server:{id}` or `tool:{server_id}/{name}`. Server to client **inverts** the tuple: the **actor** is the backend server; the **resource** is the user and/or client being acted upon.

| MCP method | Subject (actor) | Permission | Resource (target) | Default posture |
|---|---|---|---|---|
| `sampling/createMessage` | `mcp_server:{server_id}` | `sample` | `user:{originator_sub}` | **DENY** unless explicit permit |
| `elicitation/create` | `mcp_server:{server_id}` | `elicit` | `user:{originator_sub}` | **ALLOW with audit** |
| `roots/list` | `mcp_server:{server_id}` | `list_roots` **(proposed)** | `mcp_client:{client_id}` **(proposed)** | **DENY** unless explicit permit |
| `notifications/cancelled` | `mcp_server:{server_id}` | `notify` **(proposed)** | `mcp_session:{session_id}` **(proposed)** | **ALLOW with audit** when session-correlated |
| `notifications/progress` | `mcp_server:{server_id}` | `notify` **(proposed)** | `mcp_session:{session_id}` **(proposed)** | **ALLOW with audit** when token matches session |

`{originator_sub}` is denormalized from `act_chain[0].sub` ([act-chain.md](../identity/act-chain.md)). `{server_id}` on the NATS message must match session binding; mismatch denies before SpiceDB.

**Scope token hooks (proposed, mesh JWT `scope` claim):**

| Permission string | Meaning |
|---|---|
| `sampling:permit` | Explicit allow for `sampling/createMessage` for this server/user pairing |
| `elicitation:user_in_loop` | Allow `elicitation/create`; may still trigger HITL via risk engine |
| `roots:list` **(proposed)** | Explicit allow for `roots/list` |

Agent registry `callback_permissions` block shape is deferred to a registry ADR; defaults above apply when unset.

### Default-deny vs default-allow matrix

Fail-closed is the platform posture for high-risk callback capabilities. The gateway plan SpiceDB table ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § SpiceDB Integration Model) covers sampling and elicitation; this ADR extends it with roots and notifications per the paper spec.

| MCP method | Default | SpiceDB required | Rationale |
|---|---|---|---|
| `sampling/createMessage` | **DENY** | Yes — `sample` must pass **and** `sampling:permit` in policy when configured | Highest exfiltration risk; implicit client LLM access |
| `elicitation/create` | **ALLOW with audit** | Yes — `elicit` check; HITL when risk score exceeds threshold | Sanctioned user-in-the-loop surface; phishing guardrails via risk engine |
| `roots/list` | **DENY** | Yes — `list_roots` **and** `roots:list` scope when configured | Filesystem fingerprinting |
| `notifications/cancelled` | **ALLOW with audit** | Optional — session `requestId` correlation | Low risk when correlated; orphan cancels denied **(proposed)** |
| `notifications/progress` | **ALLOW with audit** | Optional — `progressToken` session match | Deny unknown tokens **(proposed)** |

**SpiceDB unreachable:** fail-closed for sampling, roots, and elicitation; fail-open with audit for progress/cancel notifications only when session correlation succeeds ([bidirectional-enforcement.md](../identity/bidirectional-enforcement.md) § Failure modes; full matrix in failure-mode-matrix spec when published).

**Missing callback rules in bundle:** deny (same as day-zero default deny for unpinned rules) — JSON-RPC `-32100 policy_deny` / `-32108 no_policy` when no bundle loaded in enforce mode.

### Session binding and identity

At `initialize`, the gateway records session KV keys (exact bucket name follows session model ADR — Block C sibling):

| Key | Value |
|---|---|
| `mcp.session.{session_id}.client_id` | Edge client id |
| `mcp.session.{session_id}.server_id` | Backend server id |
| `mcp.session.{session_id}.tenant` | Tenant claim |
| `mcp.session.{session_id}.originator_sub` | `act_chain[0].sub` |
| `mcp.session.{session_id}.act_chain` | Full chain snapshot |

Callback handler **must reject** if `server_id` on the NATS message differs from session `server_id`. STS re-exchange before edge publish uses audience `urn:trogon:mcp:client:{tenant}:{client_id}` ([ADR 0005](0005-token-ttl-and-audience.md), [sts-exchange.md](../identity/sts-exchange.md)); STS appends one `mcp_server:{server_id}` hop to `act_chain`.

### Audit envelope — Pin 7 `direction` field

Every callback decision emits one envelope to `mcp.audit.{outcome}.callback.{method_root}` on JetStream stream `MCP_AUDIT`. The envelope **must** set:

| Field | Callback value |
|---|---|
| `direction` | `"callback"` (Pin 7 — distinguishes from `"request"`) |
| `schema` | `"trogon.mcp.audit/v1"` |
| `is_notification` | `true` for `notifications/*`; else `false` |
| `jsonrpc_id` | JSON-RPC `id`; null for notifications |
| `callback.server_id` | Backend initiator |
| `callback.client_id` | Edge target |
| `callback.originator_sub` | Denormalized user for indexing |
| `target.aud` | `urn:trogon:mcp:client:{tenant}:{client_id}` |
| `target.client_id` | Same as `callback.client_id` |
| `target.user_sub` | User resource for SpiceDB |
| `caller.sub` | `mcp_server:{id}` on callback allow |
| `spicedb.permission` / `spicedb.subject` / `spicedb.resource` | When SpiceDB checked — inverted tuple fields |

Full example and deny envelope shape: [bidirectional-enforcement.md](../identity/bidirectional-enforcement.md) § Audit envelope. Callback audit extends Pin 7 ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Wire-Format Pin 7) without breaking forward-compat (`consumers MUST tolerate unknown fields`).

Audit subject `{method_root}` mapping:

| MCP method | `{method_root}` |
|---|---|
| `sampling/createMessage` | `sampling` |
| `elicitation/create` | `elicitation` |
| `roots/list` | `roots` |
| `notifications/cancelled`, `notifications/progress` | `notification` |

### HITL and risk integration

`policy::run_with_risk` ([adaptive-access.md](../identity/adaptive-access.md)) applies to callbacks. Sampling and credential-bearing elicitation may park at gateway with `-32107 approval_required` and notify via `mcp.approvals.{request_id}` / Slack console URL — same contract as request direction. Edge MCP client is not exposed to `-32107` in v1 unless gateway proxies approval to host UI (open question in paper spec).

### Enforcement mode flag (v1 rollout)

Per-tenant configuration **`reverse_direction_enforcement`** controls runtime behavior:

| Value | Behavior |
|---|---|
| `off` | Forward callbacks without policy evaluation; audit minimal or none — break-glass only |
| `audit` | Evaluate policy; log `would_deny` / full envelope; **do not block** — v1 default |
| `enforce` | Evaluate policy; deny on rule/SpiceDB failure — target after soak period |

Default **`audit`** in v1; migrate tenants to **`enforce`** after operator review of callback audit volume and false-positive rate. Shadow mode (`MCP_GATEWAY_AGENT_IDENTITY=shadow`) aligns with `audit` semantics for callback rules.

---

## Consequences

### Positive

- **Symmetric chokepoint.** Callbacks receive the same treatment class as requests: authn context from MCP session, authz via SpiceDB + CEL, redaction, audit, optional HITL. Supply-chain compromise of a backend cannot silently exfiltrate via sampling or phish via elicitation without a gateway decision record.
- **Session-bound targeting.** Backends cannot callback arbitrary clients — `{client_id}` and `{server_id}` must match session KV from `initialize`. Cross-client publish attempts fail at session validation before policy.
- **Audit completeness.** Pin 7 `direction: "callback"` and callback-specific envelope fields let SIEM and traffic-view projectors filter server-initiated traffic independently from request direction ([agent-traffic.md](../identity/agent-traffic.md)).
- **Reuse of existing modules.** Approvals (`trogon-mcp-gateway::approvals`), STS egress mint (`egress/`), CEL builtins, and hierarchical merge compile targets already in tree — callback path adds a consumer and rule section, not a parallel product.
- **Composable policy.** Operators extend org/tenant bundles with `callback:` rules without forking request-direction YAML; deny-sticky merge prevents org allow from overriding tenant callback deny.

### Negative

- **Two SpiceDB resource shapes to maintain.** Request-direction tuples (`user` to `mcp_server` / `tool`) and callback-direction tuples (`mcp_server` to `user` / `mcp_client` / `mcp_session`) require separate Zed schema definitions, migration, and operator documentation. BulkCheck patterns from ADR 0014 do not directly apply to single-item callback checks.
- **CEL rule count grows.** Each callback method needs default-deny/permit rules, risk hooks, and optional HITL predicates in the `callback:` section — multiplied across org/tenant/server hierarchy layers. Bundle size and compile time increase; misconfiguration risk rises without dry-run tooling.
- **Latency budget.** Callback path adds session KV lookup, inverted SpiceDB check, optional HITL park, and STS re-exchange before edge publish — acceptable for sampling/elicitation (human-scale) but progress notification bursts may need shedding ([bidirectional-enforcement.md](../identity/bidirectional-enforcement.md) § Failure modes).
- **Proposed permissions unfinished.** `list_roots`, `notify`, `mcp_client`, and `mcp_session` SpiceDB types are marked proposed in the paper spec; implementers must land Zed definitions before enforce-mode roots/notifications are safe.
- **Dual test matrices.** Request-direction integration tests plus callback-direction tests (deny default sampling, allow elicitation with audit, HITL approve path) expand CI NATS harness surface.

### Neutral

- **Subscription scoping remains separate.** [ADR 0011](0011-nats-auth-callout.md) and `tests/subscription_scoping.rs` continue to govern client subscribe ACLs; this ADR does not change callout permission templates except where callback subscribe patterns already grant `mcp.gateway.callback.{caller_id}.>`.
- **Phase 4 delivery.** Decision is pinned now; code lands in Phase 4 per gateway plan — paper spec checklist in [bidirectional-enforcement.md](../identity/bidirectional-enforcement.md) tracks implementation items.
- **Notification priority deferred.** `notifications/tools/list_changed` and similar server to client notifications follow the same enforcement class but Block C initial priority is sampling, elicitation, roots — see Open questions.

### Test coverage anchor

Integration test scaffold for subscription scoping (callback **subscribe** side):

| File | Scope |
|---|---|
| `rsworkspace/crates/trogon-mcp-gateway/tests/subscription_scoping.rs` | Perimeter NATS subscribe ACL: own `mcp.client.{id}.notifications.>`, cross-client deny, wildcard caps, drain/expiry |

Phase 4 callback **relay** tests (to implement alongside this ADR):

- Deny default `sampling/createMessage` without `sampling:permit`
- Allow `elicitation/create` with audit envelope `direction=callback`
- HITL approve path for high-risk sampling (Slack / `mcp.approvals.{request_id}`)
- Session mismatch deny (`-32116`)
- STS egress audience `urn:trogon:mcp:client:{tenant}:{client_id}` on allow path

---

## Alternatives considered

### Trust the backend entirely (no gateway callback policy)

Rely on NATS ACLs (`mcp.client.>` publish allowed for backends; gateway subscribes) and backend operational trust. Gateway forwards all callbacks after session subject parsing only.

| Assessment | |
|---|---|
| **Pros** | Simplest implementation; lowest callback latency; no SpiceDB load on reverse path. |
| **Cons** | Supply-chain and insider risk: any compromised or malicious backend with a valid session can exfiltrate via sampling, phish via elicitation, or recon via roots without a gateway authorization decision or tamper-evident audit on that leg. Contradicts mesh identity model ([overview.md](../identity/overview.md)) where the server workload is an actor requiring policy. |
| **Verdict** | **Rejected.** NATS ACLs enforce **which clients are addressable**, not **which capabilities a server may invoke** on the bound client. |

### Block all server to client traffic

Gateway denies every message on `mcp.client.>` at ingress; edge clients never receive server-initiated JSON-RPC.

| Assessment | |
|---|---|
| **Pros** | Eliminates callback abuse surface entirely; minimal policy engine work. |
| **Cons** | Breaks MCP as specified: sampling, elicitation, and roots are core client-advertised capabilities. Agents cannot request user input through sanctioned elicitation; servers cannot use client-side LLM sampling; filesystem-aware tools lose roots discovery. Product value proposition vs plain RPC proxy collapses. |
| **Verdict** | **Rejected.** Platform requires selective allow with strong defaults (deny sampling/roots, allow-with-audit elicitation, session-scoped notifications) rather than blanket block. |

### Client-side-only enforcement (host MCP client filters callbacks)

Gateway forwards all callbacks; the host application decides whether to honor sampling/elicitation/roots at the client SDK layer.

| Assessment | |
|---|---|
| **Pros** | No gateway Phase 4 work; flexible per-host UX for elicitation. |
| **Cons** | No centralized audit or policy for SIEM; inconsistent enforcement across client implementations; NATS-native agents bypass host UI; fails tenancy and compliance requirements for operator-controlled policy. |
| **Verdict** | **Rejected** as primary control. Host client UX remains responsible for rendering elicitation UI; gateway remains policy enforcement point. |

### Separate callback gateway service

Dedicated binary subscribes to `mcp.client.>` with its own policy bundle and SpiceDB client, distinct from `trogon-mcp-gateway` request consumer.

| Assessment | |
|---|---|
| **Pros** | Blast-radius isolation; independent scale for callback bursts. |
| **Cons** | Duplicates session KV access, STS mint, audit publisher, bundle hot-swap, and hierarchical merge — two code paths drift on inverted tuple semantics and Pin 7 envelope fields. Same queue-group instances already subscribe to both subjects per Pin 3. |
| **Verdict** | **Rejected.** Single gateway fleet with separate queue group `mcp-gateway-callbacks` is sufficient. |

---

## Implementation notes

### Gateway consumer (Phase 4)

| Component | Detail |
|---|---|
| Subscribe | `mcp.client.>` queue group `mcp-gateway-callbacks` |
| Session correlation | `mcp-session-id` header or subject `{client_id}` + session KV |
| Pre-policy gates | Session exists; `server_id` match; JSON-RPC parse; notification shape validation |
| Policy eval | `callback:` bundle section only; `mcp.direction == "callback"` |
| Egress mint | STS exchange `aud=urn:trogon:mcp:client:{tenant}:{client_id}` before publish to `mcp.gateway.callback.*` |
| Reply map | Gateway inbox `_INBOX.gateway.{instance_id}.{nuid}`; server inbox from backend `reply-to` |

### Bundle shape (illustrative)

```yaml
# Tier 1 + callback CEL section — exact manifest fields per ADR 0010
callback:
  - id: callback-sampling-default-deny
    priority: 100
    expr: |
      mcp.direction == "callback"
      && callback.method == "sampling/createMessage"
      && !spicedb.check("mcp_server:" + callback.server_id, "sample", "user:" + chain.originator().sub)
    effect: deny
  - id: callback-elicitation-default-allow
    priority: 50
    expr: |
      mcp.direction == "callback"
      && callback.method == "elicitation/create"
      && spicedb.check("mcp_server:" + callback.server_id, "elicit", "user:" + chain.originator().sub)
    effect: allow
```

### JSON-RPC error codes (callback leg)

| Code | Symbol | When |
|---|---|---|
| `-32100` | `policy_deny` | SpiceDB / CEL deny |
| `-32105` | `rate_limited` | Callback throttle (ADR 0012 Tier 2 bucket **proposed** for `(tenant, server_id, client_id)`) |
| `-32107` | `approval_required` | HITL park / timeout |
| `-32109` | `audience_mismatch` | STS egress `aud` wrong |
| `-32111` to `-32115` | `act_chain_*` | Chain validation |
| `-32116` | `session_client_mismatch` **(proposed)** | Wrong `client_id` for session |
| `-32117` | `session_not_found` **(proposed)** | Unknown / expired session |

### Code and config surfaces affected

| Surface | Change |
|---|---|
| `trogon-mcp-gateway` | Callback consumer module; session KV reader; callback policy eval path |
| `trogon-mcp-gateway::audit::AuditEnvelope` | Callback fields per paper spec § Audit envelope |
| `trogon-mcp-gateway::egress` | Callback target audience helper (partially present) |
| `trogon-mcp-gateway::approvals` | Park callback pending HITL |
| Policy bundle loader | Parse `callback:` section; bind `callback.*` CEL variables |
| SpiceDB schema | `list_roots`, `notify`, `mcp_client`, `mcp_session` **(proposed)** |
| First-party MCP pack | Default callback deny/allow rules, tuple derivation |
| Integration tests | Callback deny/allow/HITL paths (new test file or extend e2e harness) |

### Observability (proposed)

| Metric | Labels | When |
|---|---|---|
| `mcp_gateway_callback_total` | `tenant`, `method`, `outcome` | Each callback decision |
| `mcp_gateway_callback_latency_us` | `method`, `outcome` | End-to-end callback handling |
| `mcp.gateway.callback.queue_depth` | `instance_id` | Queue group lag |

---

## Open questions

1. **Server-initiated `notifications/tools/list_changed`** — Same enforcement class as `notifications/cancelled` (authorize, redact, audit, forward) but not Block C day-one priority. **Deferred:** gated like other notifications with session scope, or always allowed with audit only when `server_id` matches session? Subscription scoping tests in `subscription_scoping.rs` assume client may subscribe to `...notifications.tools/list_changed`; gateway relay policy for server-emitted list_changed remains unpinned until notification ADR or Phase 4 follow-on.

2. **`mcp_server` in `act_chain`** — Is `sub: mcp_server:{id}` the canonical STS hop shape, or should backends register as agents with `agent_id`? Affects audit `caller` embedding and SpiceDB subject stability.

3. **Orphan `notifications/cancelled`** — Deny when `requestId` has no matching in-flight gateway-tracked operation, or allow-with-audit? Paper spec recommends deny orphan cancels **(proposed)**.

4. **Client UI proxy for `-32107` on sampling** — Host MCP client displays approval UI vs Slack/console mandatory in v1.

5. **OAuth-MCP third-party servers** — Callback policy interaction with externally issued OAuth tokens (Block C OAuth item).

6. **Callback rate limits** — Separate bucket per `(tenant, server_id, client_id)` vs shared with client to server ([ADR 0012](0012-rate-limit-state.md) opt-in via CEL).

7. **Output redaction on sampling results** — Model output returning through gateway may contain PII; result redaction schema undefined.

8. **Federated virtual servers** — Callback when `server_id` is `virtual-*` fan-out member; session binds logical vs physical id.

9. **SpiceDB Zed definitions** — `list_roots`, `notify`, `mcp_client`, `mcp_session` permissions need schema publication before enforce-mode roots/notifications.

10. **Phase 1 vs Phase 2 enforcement scope** — Paper spec recommends enforce session binding + audit in Phase 1; full SpiceDB + HITL for sampling in Phase 2. This ADR pins semantics; phasing is an implementation schedule choice tracked in gateway plan open question #5.

11. **Audit subject unification** — `mcp.audit.rewrite.callback.sampling` for redaction-only events vs single allow/deny envelope per message.

12. **CEL Pin 8 formal addition** — `mcp.direction`, `callback.*` roots require Wire-Format Pin 8 amendment; until then treat as **proposed** extensions documented in [bidirectional-enforcement.md](../identity/bidirectional-enforcement.md).

---

## Rollback plan

Operators control callback enforcement without redeploying gateway binaries via per-tenant flag **`reverse_direction_enforcement`**:

| Phase | Setting | Operator action |
|---|---|---|
| **v1 default** | `audit` | Policy evaluates; denials logged in audit with full Pin 7 envelope; traffic forwarded — observe false positives and volume |
| **Soak complete** | `enforce` | Flip tenant (or global KV default) to `enforce`; denials return JSON-RPC errors to backend server inbox |
| **Incident / break-glass** | `off` | Disable callback policy evaluation; rely on NATS ACLs and session binding only — requires explicit operator ack and time-bounded change ticket |
| **Bundle rollback** | Pointer flip | Hot-swap bundle version to prior known-good `callback:` rules ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Bundles) |
| **Feature gate** | Env override **(proposed)** | `MCP_GATEWAY_CALLBACK_ENFORCEMENT=off\|audit\|enforce` overrides tenant default for canary instances |

Rollback does **not** require NATS subject grammar changes — `mcp.client.*` and `mcp.gateway.callback.*` remain stable. Downgrade from `enforce` to `audit` is immediate (config/KV flip); downgrade to `off` should emit `mcp.control.audit` or operator alert because audit completeness regresses.

Shadow mode remains available: log `would_deny: true` without blocking, equivalent to `audit` for callback rules.

---

## Status of supporting work

| Work item | Status | Owner / anchor |
|---|---|---|
| Block C bidirectional enforcement ADR | **Done** (this document) | `docs/adr/0020-bidirectional-enforcement.md` |
| Normative design spec | **Done** | [bidirectional-enforcement.md](../identity/bidirectional-enforcement.md) |
| Callback audience URI | **Done** | [ADR 0005](0005-token-ttl-and-audience.md), `trogon-mcp-gateway::egress::audience` |
| Subscription scoping test scaffold | **Scaffold** | `tests/subscription_scoping.rs` (ignored; ADR 0011) |
| Queue group `mcp-gateway-callbacks` | **Pinned** | `MCP_GATEWAY_PLAN.md` Pin 3 — not wired |
| Callback ingress consumer | **Pending** (Phase 4) | `trogon-mcp-gateway` |
| `callback:` bundle section + CEL variables | **Pending** (Phase 4) | Policy loader |
| SpiceDB inverted tuple schema | **Partial** — sampling/elicit in plan table; roots/notify proposed | SpiceDB / Zed |
| Callback audit envelope fields | **Pending** (Phase 4) | `audit.rs` |
| HITL callback parking | **Pending** (Phase 4) | `approvals/` |
| Integration tests (deny sampling, allow elicitation, HITL) | **Pending** (Phase 4) | NATS harness |
| `reverse_direction_enforcement` tenant flag | **Pending** (Phase 4) | KV config |
| Close `MCP_GATEWAY_PLAN.md` Block C item 7 checkbox | **Pending** | Editorial after ADR acceptance |

---

*Design context: [bidirectional-enforcement.md](../identity/bidirectional-enforcement.md). This ADR is the durable decision record for server to client MCP callback enforcement; implementation tickets follow the spec sections and Phase 4 gateway plan.*
