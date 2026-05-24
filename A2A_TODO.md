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
- [ ] Populate remaining gateway decision-site `AuditEnvelope` fields — an authoritative JWT-derived `caller_id` once Tier 1 + auth callout land (`trace_id`, `rules_fired`, `rewrites`, and `stream_consumer` are populated for gateway decision sites today; fields stay optional on `AuditEnvelopeFields`).

## Phase 2 — streaming & lifecycle

- [ ] CEL → WASM compile path + real `Tier2CelEvaluator` impl to replace `NoopTier2Evaluator` in `a2a-gateway/src/policy/tier2.rs`.
- [ ] Extend gateway policy stack — authoritative Tier 2 CEL + richer Tier 3 skill matrix beyond preload-only redaction stubs (`A2A_GATEWAY_POLICY_BUNDLE_DIR` / `_SKILLS` already host Wasmtime preload today).
- [ ] Streaming back-pressure — gateway pull consumer with flow control; `A2A_EVENTS` policy `retention=interest, discard=old`. Ops/design: [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md).

## Phase 3 — push delivery & redaction

- [ ] Tier 3 authoritative redaction in the gateway beyond preload-only skill hosts — deterministic policies, refusal semantics, telemetry once Tier 2 CEL enforces payloads (engine/module loader/skills ship in `a2a-redaction`; gateway invokes substrate when bundles configured).
- [ ] Optional gateway-side `A2A_PUSH_DLQ` mirroring of agent-side DLQ envelopes (agent path already publishes).
- [ ] End-to-end principal propagation into push DLQ `caller_id` once auth-callout is deployed — `CallerId::from_principal` keys off `SpiceDbPrincipal.spicedb_subject`; the `_` fallback only triggers when the minted JWT principal omits it.

## Phase 4 — interop & federation

- [ ] **`A2A_BRIDGE_TRANSPORT=nats`** path exercised end-to-end against a deployed auth-callout mint (HTTPS surface, unary gateway, SSE ↔ JetStream already wired; **`stub` remains default** for unit/integration without live NATS).
- [ ] Operator-signed Account export/import contract for `a2a.discover.>` cross-Account federation — gated through `SpiceDbImportGate` at the import boundary (**stub still returns deny until SpiceDB-backed checks land; use `AllowAllImportGate` for labs — see crates/docs**).
- [ ] Cross-binding collaboration tests against a live NATS + gateway + bridge (depends on validating **`A2A_BRIDGE_TRANSPORT=nats`** + Tier 1 + deployed auth-callout mint).

## Cross-cutting

- Gateway request path (**partial**) — Wasmtime-hosted Tier-3 redaction preload + ingress Tier-2 predicate seam, decision-site audit publish with `trace_id`, `rules_fired`, `rewrites`, and `stream_consumer`, unary `message.send` deadline (`A2A_GATEWAY_UNARY_DEADLINE_SECS`), and caller tracing via NATS `X-A2a-Caller-Id` / HTTPS `x-a2a-caller-id` from `a2a-bridge`. **Still pending:** SpiceDB Tier 1, authoritative JWT-derived `caller_id`, end-to-end auth-callout verifier in the gateway tier.

---

## Suggested ordering

1. Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` (verifier crate shipped; operator wiring + NSC pipelines).
2. Finish `a2a-gateway` policy depth — SpiceDB Tier 1, authoritative JWT-derived `caller_id`, CEL Tier 2, richer decision-site audits atop the Wasmtime preload + unary deadline scaffolding now in-tree (`trace_id`, `rules_fired`, `rewrites`, and `stream_consumer` ship today).
3. SpiceDB Tier 1 — gateway client, resource-tuple derivation, owner tuples on task lifecycle, `BulkCheckPermission` catalog shaping, real `SpiceDbImportGate` impl.
4. Streaming back-pressure (gateway pull consumer + `A2A_EVENTS` policy); unary deadline knobs already wired via env.
5. Tier 3 redaction semantics + CEL Tier 2 (WASM compile path replaces `NoopTier2Evaluator`) once payloads are enforceable beyond today’s preload seam.
6. Hardened push residuals — optional gateway-side DLQ mirroring, end-to-end principal propagation once auth-callout deployed.
7. `a2a-bridge` env-gated **`A2A_BRIDGE_TRANSPORT=nats`** bootstrap (mint unary + unary gateway + SSE JetStream) alongside federated discovery exports + cross-binding collaboration tests (`stub` stays default so unit tests skip live NATS).
