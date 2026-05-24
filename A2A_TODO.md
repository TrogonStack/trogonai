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

- [ ] Tier 1 declarative policies (bundle tables beyond SpiceDB) wired into the gateway request path.
- [ ] Populate remaining gateway decision-site `AuditEnvelope` fields — an authoritative JWT-derived `caller_id` once auth callout lands (`trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, and `zed_token_snapshot` are populated for gateway decision sites today; fields stay optional on `AuditEnvelopeFields`).

**Shipped (Phase 1):** Federated `SpiceDbImportGate` — real `BulkCheckPermission` gate landed in `a2a-nats::catalog::import_gate` with ZedToken cache and env-gated construction (`A2A_SPICEDB_ENDPOINT` / `A2A_SPICEDB_TOKEN`); deny-only when unset. **Gateway Tier-1 request-path SpiceDB** — `SpiceDbTier1Gate` in `a2a-gateway/src/policy/spicedb_tier1.rs` reuses the Authzed client types; env-gated via `A2A_GATEWAY_TIER1_SPICEDB_ENABLED` (+ endpoint/token/TTL knobs); per-method resource tuples, session ZedToken cache, owner tuples on `message/send` accept, `-32801` closed-fail on deny/transport error; audit `zed_token_snapshot` on allow.

## Phase 2 — streaming & lifecycle

- [ ] Extend gateway policy stack — richer Tier 3 skill matrix beyond preload-only redaction stubs (`A2A_GATEWAY_POLICY_BUNDLE_DIR` / `_SKILLS` already host Wasmtime preload today).

**Shipped (Phase 2):** Streaming back-pressure — `A2A_EVENTS` provisioned with `retention=interest, discard=old, max_age=24h`; env-gated gateway pull consumer (`A2A_GATEWAY_EVENTS_PULL=on|off`, default off) with configurable `max_ack_pending`, fetch batch, heartbeat, and per-caller in-flight cap. **Tier-2 CEL evaluator** — `tier2/*.cel` bundle compile path + `RealTier2CelEvaluator` in `a2a-gateway` (env-gated via `A2A_GATEWAY_TIER2_CEL_ENABLED`; see [`docs/A2A_TIER2_CEL.md`](./docs/A2A_TIER2_CEL.md)).

## Phase 3 — push delivery & redaction

- [ ] Tier 3 authoritative redaction in the gateway beyond preload-only skill hosts — deterministic policies, refusal semantics, telemetry once Tier 2 CEL enforces payloads (engine/module loader/skills ship in `a2a-redaction`; gateway invokes substrate when bundles configured).
- [ ] End-to-end principal propagation into push DLQ `caller_id` — code wiring landed in agent `Bridge` / `message/stream` (`PrincipalCarrier`, `CallerId::from_principal` over `SpiceDbPrincipal.spicedb_subject`); populates when JWT principal arrives via deployed auth-callout + gateway header forward. Still requires deployed mint.

**Shipped (Phase 3):** Gateway-side push DLQ mirror — env-gated JetStream pull consumer republishes agent DLQ envelopes to **`{prefix}.push.dlq.mirror.{caller_id}.{task_id}`**; exactly-once dedup across agent + mirror paths remains deferred ([`A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md)).

## Phase 4 — interop & federation

- [ ] **`A2A_BRIDGE_TRANSPORT=nats`** integration harness landed in `a2a-bridge::nats_transport_harness` (in-process mocks + optional `#[ignore]` live NATS smoke); **still requires deployed auth-callout mint for production** (`stub` remains default).
- [ ] Operator-signed Account export/import contract for `a2a.discover.>` cross-Account federation — gated through `SpiceDbImportGate` at the import boundary (**deny-only until `A2A_SPICEDB_*` env is set; use `AllowAllImportGate` for labs — see crates/docs**).
- [ ] Cross-binding collaboration tests against a live NATS + gateway + bridge (depends on validating **`A2A_BRIDGE_TRANSPORT=nats`** + Tier 1 + deployed auth-callout mint).

## Cross-cutting

- Gateway request path (**partial**) — Tier-1 SpiceDB gate (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), Wasmtime-hosted Tier-3 redaction preload + env-gated CEL Tier-2 evaluator (`A2A_GATEWAY_TIER2_CEL_ENABLED`), decision-site audit publish with `trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, and `zed_token_snapshot`, unary `message.send` deadline (`A2A_GATEWAY_UNARY_DEADLINE_SECS`), and caller tracing via NATS `X-A2a-Caller-Id` / HTTPS `x-a2a-caller-id` from `a2a-bridge`. **Still pending:** authoritative JWT-derived `caller_id`, end-to-end auth-callout verifier in the gateway tier.

---

## Suggested ordering

1. Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` (verifier crate shipped; operator wiring + NSC pipelines).
2. Finish `a2a-gateway` policy depth — authoritative JWT-derived `caller_id`, richer decision-site audits atop the Wasmtime preload + env-gated CEL Tier-2 + unary deadline + Tier-1 SpiceDB scaffolding now in-tree (`trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, and `zed_token_snapshot` ship today).
3. Tier 3 redaction semantics once payloads are enforceable beyond today's preload seam (CEL Tier-2 ships in-tree, env-gated).
4. Hardened push residuals — principal → DLQ `caller_id` wiring shipped in-tree; live when auth-callout + gateway header forward deploy (gateway-side DLQ mirror already shipped, env-gated).
5. `a2a-bridge` env-gated **`A2A_BRIDGE_TRANSPORT=nats`** bootstrap (mint unary + unary gateway + SSE JetStream) alongside federated discovery exports + cross-binding collaboration tests (`stub` stays default so unit tests skip live NATS).
