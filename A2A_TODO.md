# A2A over NATS — TODO

Work tracker for the gap between [`A2A_PLAN.md`](./A2A_PLAN.md) and what is in-tree on `yordis/feat-a2a-nats`.

Every item below is open work. Shipped work lives in `A2A_PLAN.md` §Implementation Status — not duplicated here. All architectural decisions are landed (see `A2A_PLAN.md` §Decisions).

**Design supplements (not duplicate trackers):** [`docs/A2A_AUTH_CALLOUT_SKETCH.md`](./docs/A2A_AUTH_CALLOUT_SKETCH.md), [`docs/A2A_BRIDGE_SKETCH.md`](./docs/A2A_BRIDGE_SKETCH.md), [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md), [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md), [`docs/A2A_GATEWAY_ROADMAP.md`](./docs/A2A_GATEWAY_ROADMAP.md), [`docs/A2A_PUSH_DLQ_OPS.md`](./docs/A2A_PUSH_DLQ_OPS.md), [`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md), [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md), [`docs/A2A_SUBJECT_ACL_QUICKREF.md`](./docs/A2A_SUBJECT_ACL_QUICKREF.md), [`docs/A2A_RUNTIME_ENV.md`](./docs/A2A_RUNTIME_ENV.md), [`docs/A2A_DOCS_INDEX.md`](./docs/A2A_DOCS_INDEX.md) (navigation hub).

## Phase 0 — perimeter & catalog

- [ ] Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` against a live NATS account server (verifiers + JWT mint already in `a2a-auth-callout`; missing: account resolver wiring, signing-key material in a secret store, deployment manifest).
- [ ] Enforce `{prefix}.catalog.register.{agent_id}` ACL-exclusive writes in every environment via deployed NSC bindings to the registrar User (per-role templates and bootstrap script already under `scripts/`; remaining is operator deployment automation).
- [ ] NSC operator pipelines — secret-store-backed key handling and automated `nsc push` for per-Account provisioning beyond the idempotent bootstrap script.
- [ ] Gateway / edge re-validates AgentCard JSON-Schema on read once a path materializes AgentCards outside NATS KV.

## Phase 1 — policy & audit

- [ ] Tier 1 declarative policies wired into the gateway request path.
- [ ] SpiceDB integration — gateway client to org-standard cluster; `BulkCheckPermission` for catalog shaping; per-method resource tuples; owner tuples on task lifecycle; ZedToken cache per session.
- [ ] Real `SpiceDbImportGate` implementation to replace `AllowAllImportGate` default in `a2a-nats::catalog::import_gate`.
- [ ] Populate gateway decision-site `AuditEnvelope` fields (`trace_id`, `rules_fired`, `rewrites`, `stream_consumer`) once auth callout + Tier 1 land — fields already optional on `AuditEnvelopeFields`.

## Phase 2 — streaming & lifecycle

- [ ] CEL → WASM compile path + real `Tier2CelEvaluator` impl to replace `NoopTier2Evaluator` in `a2a-gateway/src/policy/tier2.rs`.
- [ ] Call `WasmtimeSubstrate` (Tier 2 + Tier 3) from the gateway request path — substrate type exists; ingress doesn't invoke it.
- [ ] Streaming back-pressure — gateway pull consumer with flow control; `A2A_EVENTS` policy `retention=interest, discard=old`. Ops/design: [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md).
- [ ] `message/send` 30s gateway deadline; longer work transitions to `message/stream`.

## Phase 3 — push delivery & redaction

- [ ] Tier 3 redaction call site in the gateway request path — engine, module loader, and skill-id dispatch already ship in `a2a-redaction`.
- [ ] Optional gateway-side `A2A_PUSH_DLQ` mirroring of agent-side DLQ envelopes (agent path already publishes).
- [ ] End-to-end principal propagation into push DLQ `caller_id` once auth-callout is deployed — `CallerId::from_principal` keys off `SpiceDbPrincipal.spicedb_subject`; the `_` fallback only triggers when the minted JWT principal omits it.

## Phase 4 — interop & federation

- [ ] `a2a-bridge` production wiring against a deployed auth-callout (axum HTTPS surface, NATS publish/consume, SSE↔JetStream framing already shipped; tests use trait mocks).
- [ ] Operator-signed Account export/import contract for `a2a.discover.>` cross-Account federation — gated through `SpiceDbImportGate` at the import boundary.
- [ ] Cross-binding collaboration tests against a live NATS + gateway + bridge (depends on bridge production wiring and at least Tier 1 + auth-callout).

## Cross-cutting

- [ ] `a2a-gateway` request-path wiring — auth-callout client, JWT-derived `caller_id` extraction, `WasmtimeSubstrate` call sites, decision-site `AuditEnvelope` emission. Today the gateway forwards opaquely.

---

## Suggested ordering

1. Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` (verifier crate shipped; operator wiring + NSC pipelines).
2. Wire `a2a-gateway` request path end-to-end — auth-callout client, `caller_id` extraction, `WasmtimeSubstrate` call sites, decision-site audit envelopes (`rules_fired`, `rewrites`, `trace_id`).
3. SpiceDB Tier 1 — gateway client, resource-tuple derivation, owner tuples on task lifecycle, `BulkCheckPermission` catalog shaping, real `SpiceDbImportGate` impl.
4. Streaming back-pressure (gateway pull consumer + `A2A_EVENTS` policy) and `message/send` 30s gateway deadline.
5. CEL Tier 2 (compile CEL→WASM at bundle build; replace `NoopTier2Evaluator`) + Tier 3 redaction call site in the request path (engine already in `a2a-redaction`).
6. Hardened push residuals — optional gateway-side DLQ mirroring, end-to-end principal propagation once auth-callout deployed.
7. `a2a-bridge` production wiring + federated discovery exports + cross-binding collaboration tests.
