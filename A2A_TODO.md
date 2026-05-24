# A2A over NATS — TODO

Work tracker for the gap between [`A2A_PLAN.md`](./A2A_PLAN.md) and what is in-tree on `yordis/feat-a2a-nats`.

Every item below is open work. Shipped work lives in `A2A_PLAN.md` §Implementation Status — not duplicated here. All architectural decisions are landed (see `A2A_PLAN.md` §Decisions).

Partial items state the in-tree code surface and what remains before the item is fully effective. They stay open until the remaining work lands.

**Design supplements (not duplicate trackers):** [`docs/A2A_AUTH_CALLOUT_SKETCH.md`](./docs/A2A_AUTH_CALLOUT_SKETCH.md), [`docs/A2A_BRIDGE_SKETCH.md`](./docs/A2A_BRIDGE_SKETCH.md), [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md), [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md), [`docs/A2A_GATEWAY_ROADMAP.md`](./docs/A2A_GATEWAY_ROADMAP.md), [`docs/A2A_PUSH_DLQ_OPS.md`](./docs/A2A_PUSH_DLQ_OPS.md), [`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md), [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md), [`docs/A2A_SUBJECT_ACL_QUICKREF.md`](./docs/A2A_SUBJECT_ACL_QUICKREF.md), [`docs/A2A_RUNTIME_ENV.md`](./docs/A2A_RUNTIME_ENV.md), [`docs/A2A_TIER2_CEL.md`](./docs/A2A_TIER2_CEL.md), [`docs/A2A_DOCS_INDEX.md`](./docs/A2A_DOCS_INDEX.md) (navigation hub).

## Phase 0 — perimeter & catalog

- [ ] Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` against a live NATS account server. **Code shipped** in `a2a-auth-callout` (OIDC / mTLS / HMAC API-key verifiers + Account-bound User JWT mint). **Remaining:** account resolver wiring, signing-key material in a secret store, deployment manifest.
- [ ] Enforce `{prefix}.catalog.register.{agent_id}` ACL-exclusive writes in every environment. **Code shipped** — per-role NSC templates + bootstrap script under `scripts/`. **Remaining:** operator deployment automation to push bindings to the registrar User across environments.
- [ ] NSC operator pipelines — secret-store-backed key handling and automated `nsc push` for per-Account provisioning beyond the idempotent bootstrap script. **Remaining:** end-to-end automation; today only the idempotent script ships.

## Phase 1 — policy & audit

- [ ] Tier 1 declarative policies — bundle tables beyond SpiceDB wired into the gateway request path. **Code shipped:** SpiceDB Tier-1 gate (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`) and federated `SpiceDbImportGate`. **Remaining:** non-SpiceDB declarative bundle tables and their evaluator.
- [ ] Authoritative JWT-derived `caller_id` on gateway decision-site audits. **Code shipped:** `AuditEnvelopeFields` populates `trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, `zed_token_snapshot`; caller attribution rides `X-A2a-Caller-Id` header. **Remaining:** replace header-derived caller with JWT-derived `caller_id` once auth-callout is deployed.

## Phase 2 — streaming & lifecycle

- [ ] Richer Tier 3 skill matrix beyond preload-only redaction stubs. **Code shipped:** `A2A_GATEWAY_POLICY_BUNDLE_DIR` / `A2A_GATEWAY_POLICY_SKILLS` host the Wasmtime preload, and Tier-2 CEL evaluator (`A2A_GATEWAY_TIER2_CEL_ENABLED`) gates payloads. **Remaining:** broader skill catalog wired through the bundle layer.

## Phase 3 — push delivery & redaction

- [ ] Tier 3 authoritative redaction in the gateway beyond preload-only skill hosts — deterministic policies, refusal semantics, telemetry. **Code shipped:** engine, module loader, and skill-id dispatch in `a2a-redaction`; gateway invokes the substrate when bundles are configured. **Remaining:** authoritative call site, deterministic policy semantics, refusal codes, and telemetry on the redaction path.
- [ ] End-to-end principal propagation into push DLQ `caller_id`. **Code shipped:** `PrincipalCarrier`, `CallerId::from_principal` over `SpiceDbPrincipal.spicedb_subject`, agent `Bridge` / `message/stream` wiring, gateway-side DLQ mirror (env-gated). **Remaining:** deployed auth-callout mint + gateway `X-A2a-Spicedb-Principal` header forward so the populated caller_id surfaces in DLQ subjects (today's `_` fallback stays active without a principal).
- [ ] Exactly-once dedup across agent + mirror DLQ paths ([`A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md)). **Code shipped:** `DeliverySemantics` JSON-RPC extension, `PushDeliverySemanticsRegistry`, `PushIdempotencyKey` + `IdempotencyKeyHeader`. **Remaining:** sketched dedup contract wired across both paths.

## Phase 4 — interop & federation

- [ ] `A2A_BRIDGE_TRANSPORT=nats` production wiring. **Code shipped:** integration harness in `a2a-bridge::nats_transport_harness` (in-process mocks + optional `#[ignore]` live NATS smoke); `stub` remains default for unit tests. **Remaining:** deployed auth-callout mint for production traffic.
- [ ] Operator-signed Account export/import contract for `a2a.discover.>` cross-Account federation. **Code shipped:** `SpiceDbImportGate` at the import boundary (deny-only until `A2A_SPICEDB_*` env is set; `AllowAllImportGate` for labs). **Remaining:** operator-signed export/import contract + deployment.
- [ ] Cross-binding collaboration tests against a live NATS + gateway + bridge. **Remaining:** depends on validating `A2A_BRIDGE_TRANSPORT=nats` + Tier 1 + deployed auth-callout mint.

## Cross-cutting

- [ ] Gateway request path completeness. **Code shipped:** Tier-1 SpiceDB gate (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), Tier-2 CEL evaluator (`A2A_GATEWAY_TIER2_CEL_ENABLED`), Wasmtime-hosted Tier-3 preload, decision-site audit publish (`trace_id`, `rules_fired`, `rewrites`, `stream_consumer`, `zed_token_snapshot`), unary `message.send` deadline (`A2A_GATEWAY_UNARY_DEADLINE_SECS`), caller tracing via NATS `X-A2a-Caller-Id` ↔ HTTPS `x-a2a-caller-id` from `a2a-bridge`. **Remaining:** authoritative JWT-derived `caller_id`, end-to-end auth-callout verifier in the gateway tier, authoritative Tier-3 redaction call site.

---

## Suggested ordering

1. Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` (verifier crate shipped; operator wiring + NSC pipelines remain). Unblocks JWT-derived `caller_id` and live push DLQ subject population.
2. Finish gateway policy depth — authoritative JWT-derived `caller_id` on audits, non-SpiceDB Tier-1 declarative bundle tables.
3. Tier 3 authoritative redaction call site on the gateway request path.
4. Operator-signed cross-Account export/import + cross-binding collaboration tests against a live stack, then flip `A2A_BRIDGE_TRANSPORT=nats` to production wiring.
