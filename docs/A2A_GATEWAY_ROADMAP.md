# A2A Gateway Roadmap

Engineering checklist for [`a2a-gateway`](../rsworkspace/crates/a2a-gateway/) beyond today's opaque forward. The gateway is the ingress decision site for `{prefix}.gateway.>` — auth, policy, and audit land here before agent RPC subjects.

## Related links

| Document | Purpose |
|----------|---------|
| [`../A2A_PLAN.md`](../A2A_PLAN.md) | Architecture, subject shapes, policy tiers, SpiceDB tuples, audit schema |
| [`../A2A_TODO.md`](../A2A_TODO.md) | Open engineering items and suggested ordering (auth callout, Tier 1, gateway audit) |
| [`./A2A_STREAMING_BACKPRESSURE_OPS.md`](./A2A_STREAMING_BACKPRESSURE_OPS.md) | Task event egress — pull consumer flow control, `A2A_EVENTS` policy, agent `Bridge` limits |
| [`./A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./A2A_NSC_ACCOUNT_BOOTSTRAP.md) | Per-Account NSC provisioning, caller/gateway/registrar ACL templates |
| [`../rsworkspace/crates/a2a-gateway/src/lib.rs`](../rsworkspace/crates/a2a-gateway/src/lib.rs) | Crate entrypoint and Rustdoc (ingress role, future seams) |

---

## Shipped today

Opaque request/reply forward; Wasmtime policy substrate scaffolded but not yet wired into the request path.

- [x] **Queue group** — optional `A2A_GATEWAY_QUEUE_GROUP` (CLI `--queue-group`) on `{prefix}.gateway.>`; unset = ephemeral subscriber.
- [x] **Ingress → agent subject map** — `a2a_nats::gateway_ingress` resolves `{prefix}.gateway.{agent_id}.{method…}` → `{prefix}.agent.{agent_id}.{method…}`; invalid shapes get JSON-RPC `-32600` on the caller reply inbox when present.
- [x] **Transparent forward** — `publish_with_reply_and_headers` preserves payload, headers, and reply inbox; messages without reply are ignored (logged).
- [x] **Tracing span fields** — span `gateway.ingress.dispatch` records:
  - `gateway_ingress.subject` — inbound NATS subject
  - `ingress.reply_present` — whether a reply inbox was attached
  - `caller_id` — reserved (`tracing::field::Empty` until JWT extraction)
  - `agent_subject` — mapped target (success paths)
  - `routing_outcome` — `forwarded` \| `ingress_error` \| `ignored_no_reply` \| `forward_failed`
- [x] **Wasmtime policy substrate scaffold** — `src/policy/{mod,wasmtime_substrate,tier2,error}.rs`. `WasmtimeSubstrate` composes Tier 2 (`Tier2CelEvaluator` trait + `NoopTier2Evaluator`) with Tier 3 (re-uses `a2a_redaction::wasm::WasmRedactorHost`). Not yet called from the ingress path.

Run: `cargo run -p a2a-gateway` from `rsworkspace/` (`NATS_URL`, `A2A_PREFIX`, optional queue group).

---

## Next

Do not implement auth-callout or SpiceDB in this crate until NSC + ACL templates from the bootstrap runbook are in place. Track ordering in [`../A2A_TODO.md`](../A2A_TODO.md) §Suggested ordering.

### NATS `$SYS` / auth-callout integration

- [ ] Wire gateway to expect **Account-bound User JWTs** minted by an auth-callout subscriber on **`$SYS.REQ.USER.AUTH`** (OIDC primary, mTLS service-to-service, API keys transitional). See [`../A2A_TODO.md`](../A2A_TODO.md) Phase 0 and [`./A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./A2A_NSC_ACCOUNT_BOOTSTRAP.md) §Auth callout service — **reference only; do not implement callout in `a2a-gateway`.**
- [ ] Extract caller identity from the connection/JWT and populate span field `caller_id` plus audit attribution fields.

### JWT minting posture

- [ ] **Per-request User inside the caller's tenant Account** — `sub` = external identity, `aud` = Account, SpiceDB principal in JWT `data` (see plan §Decisions).
- [ ] **Subject ACL** — caller User bounded to `{prefix}.gateway.>` and `_INBOX.{caller_id}.>`; dedicated gateway User bounded to `{prefix}.agent.>`, `{prefix}.task.>`, `{prefix}.push.>` (NSC templates, not gateway code).
- [ ] Reject single shared bridge User — caller attribution must survive ingress.

### Ingress audit envelopes (`AuditEmitter` parity)

- [ ] Inject `AuditEmitter` at gateway decision sites (allow/deny/rewrite), matching agent `Bridge` RPC audit shape (`AuditEnvelope` on `{prefix}.audit.{ok\|err}.{method}`).
- [ ] Populate gateway-only fields when policy lands: `rules_fired`, `rewrites`, `trace_id` (see `AuditEnvelopeFields` in `a2a-nats`).
- [ ] Do **not** duplicate agent-side `TaskLifecycleEnvelope` emission — lifecycle audit stays on the `message/stream` pump.

### Unary deadlines

- [ ] Enforce **30s gateway deadline** on unary `message/send` request/reply; longer work must use `message/stream` (see [`../A2A_PENDING_DECISION.md`](../A2A_PENDING_DECISION.md) §6).
- [ ] On timeout: structured JSON-RPC error to caller inbox + ingress audit outcome.

### Coordination with SpiceDB Tier 1

- [ ] Gateway holds org-standard **SpiceDB client**; derive resource tuples per method (`a2a-pack` / plan §SpiceDB).
- [ ] **Tier 1 declarative policies** on the gateway request path before forward (allow/deny, catalog shaping via `BulkCheckPermission`).
- [ ] Task owner tuples (`task:{id}#owner@user:{sub}`) at task creation on ingress paths that create tasks — coordinate with agent lifecycle, not duplicate in bridge-only paths.

---

## Explicit non-goals today

These are owned elsewhere; do not expand gateway scope to cover them unless the plan explicitly moves ownership.

| Area | Owner | Notes |
|------|-------|-------|
| **Push delivery & DLQ** | Agent `Bridge` / `a2a-nats::push::dlq` | Terminal push failures JetStream-publish to `{prefix}.push.dlq.{caller_id}.{task_id}` from the streaming pump — not from gateway ingress. |
| **AgentCard validation at gateway** | `a2a-nats-discovery` / KV catalog | Schema validation on registrar write and KV read defense-in-depth today. Gateway re-validation only if an **edge path materializes AgentCards outside NATS KV** ([`../A2A_TODO.md`](../A2A_TODO.md) Phase 0). |
| **JetStream provisioning** | `a2a-nats` provision / discovery processes | Gateway does not provision streams or manage streaming back-pressure yet (Phase 2). See [`./A2A_STREAMING_BACKPRESSURE_OPS.md`](./A2A_STREAMING_BACKPRESSURE_OPS.md). |
| **Tier 2 CEL / Tier 3 WASM** | Gateway Wasmtime substrate (scaffolded) | Substrate type lives in `src/policy/`; CEL→WASM compile path and request-path call sites still future. Tier 3 redaction engine ships in `a2a-redaction`. |
| **HTTPS termination** | `a2a-bridge` sidecar (runtime landed) | Crate ships axum HTTPS, auth-callout client, NATS publish/consume, SSE↔JetStream framing. Production-mode wiring against a deployed auth-callout still future. |

---

## Operator quick check

```text
# Expected today: ingress forward + span fields, nothing else
RUST_LOG=a2a_gateway=debug cargo run -p a2a-gateway

# Filter failures
# routing_outcome=ingress_error | forward_failed | ignored_no_reply
```

When auth-callout ships, verify: caller can publish only `{prefix}.gateway.>`; gateway User can reach `{prefix}.agent.>`; `caller_id` appears on ingress spans and audit envelopes.
