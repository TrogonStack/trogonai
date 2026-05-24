# A2A over NATS — TODO

Work tracker for the gap between [`A2A_PLAN.md`](./A2A_PLAN.md) and what is in-tree on `yordis/feat-a2a-nats`.

Every item below is open work. Shipped work lives in `A2A_PLAN.md` §Implementation Status — not duplicated here. All architectural decisions are landed (see `A2A_PLAN.md` §Decisions).

Partial items state the in-tree code surface and what remains before the item is fully effective. They stay open until the remaining work lands.

**Design supplements (not duplicate trackers):** [`docs/A2A_AUTH_CALLOUT_SKETCH.md`](./docs/A2A_AUTH_CALLOUT_SKETCH.md), [`docs/A2A_BRIDGE_SKETCH.md`](./docs/A2A_BRIDGE_SKETCH.md), [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md), [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md), [`docs/A2A_GATEWAY_ROADMAP.md`](./docs/A2A_GATEWAY_ROADMAP.md), [`docs/A2A_PUSH_DLQ_OPS.md`](./docs/A2A_PUSH_DLQ_OPS.md), [`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md), [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md), [`docs/A2A_SUBJECT_ACL_QUICKREF.md`](./docs/A2A_SUBJECT_ACL_QUICKREF.md), [`docs/A2A_RUNTIME_ENV.md`](./docs/A2A_RUNTIME_ENV.md), [`docs/A2A_TIER1_DECLARATIVE.md`](./docs/A2A_TIER1_DECLARATIVE.md), [`docs/A2A_TIER2_CEL.md`](./docs/A2A_TIER2_CEL.md), [`docs/A2A_TIER3_REDACTION.md`](./docs/A2A_TIER3_REDACTION.md), [`docs/A2A_DOCS_INDEX.md`](./docs/A2A_DOCS_INDEX.md) (navigation hub).

## Phase 0 — perimeter & catalog

- [ ] Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` against a live NATS account server. **Code shipped** in `a2a-auth-callout` (OIDC / mTLS / HMAC API-key verifiers + Account-bound User JWT mint). **Remaining:** account resolver wiring, signing-key material in a secret store, deployment manifest.
- [ ] Enforce `{prefix}.catalog.register.{agent_id}` ACL-exclusive writes in every environment. **Code shipped** — per-role NSC templates + bootstrap script under `scripts/`. **Remaining:** operator deployment automation to push bindings to the registrar User across environments.
- [ ] NSC operator pipelines — secret-store-backed key handling and automated `nsc push` for per-Account provisioning beyond the idempotent bootstrap script. **Remaining:** end-to-end automation; today only the idempotent script ships.

## Phase 1 — policy & audit

- [ ] Tier 1 declarative policies — bundle tables beyond SpiceDB wired into the gateway request path. **Code shipped:** SpiceDB Tier-1 gate (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), federated `SpiceDbImportGate`, declarative Tier-1 evaluator (`A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED`, default off), and reference bundles under `a2a-pack/policies/` (per-method allowlist, per-agent allowlist, time-of-day approximation). **Remaining:** production rollout (operator-signed bundle distribution per the Tier-3 contract once that's deployed; bundle authoring extensions for true time-of-day predicates).
- [ ] Authoritative JWT-derived `caller_id` on gateway decision-site audits. **Code wired:** `resolve_gateway_caller_identity` reads **`X-A2a-Spicedb-Principal`** (JWT `data` claim) with deprecated **`X-A2a-Caller-Id`** fallback; audits populate `caller_id`, `caller_source`. **Pending deployed auth-callout** for production header population on live ingress.

## Phase 2 — streaming & lifecycle

- [ ] Tier 3 signed-bundle deployment. **Code shipped:** manifest schema (`SkillManifest`), `SkillManifestRegistry`, `SkillSelectionPlan`, reference skill catalog under `a2a-pack/skills/` (`pii-regex-redactor`, `secrets-redactor`, `json-path-sanitizer`); Wasmtime preload via `A2A_GATEWAY_POLICY_BUNDLE_DIR` / `A2A_GATEWAY_POLICY_SKILLS` + `{skill}.manifest.json`; authoritative Tier-3 redaction call site (`A2A_GATEWAY_TIER3_REDACTION_ENABLED`) with refusal (`-32802`), closed-fail engine errors (`-32801`), audit `refusal_skill`, structured telemetry; signed-bundle preload verification (`a2a-redaction::signed_bundle`, `A2A_GATEWAY_TIER3_SIGNING_PUBKEY`) + `a2a-sign-bundle` operator CLI. **Remaining:** signing-key custody + signed-bundle distribution pipeline in production environments.

## Phase 3 — push delivery & redaction

- [ ] End-to-end principal propagation into push DLQ `caller_id`. **Code shipped:** `PrincipalCarrier`, `CallerId::from_principal` over `SpiceDbPrincipal.spicedb_subject`, agent `Bridge` / `message/stream` wiring, gateway-side DLQ mirror (env-gated). **Remaining:** deployed auth-callout mint + gateway `X-A2a-Spicedb-Principal` header forward so the populated caller_id surfaces in DLQ subjects (today's `_` fallback stays active without a principal).
- [ ] Exactly-once dedup across agent + mirror DLQ paths ([`A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md)). **Code wired:** `PushIdempotencyKey::derive_dlq`, in-process LRU (`A2A_PUSH_DLQ_DEDUP_LRU_SIZE`), JetStream `Nats-Msg-Id` + `duplicate_window` (`A2A_PUSH_DLQ_DEDUP_WINDOW_SECS`) on agent + gateway mirror publish paths.

## Phase 4 — interop & federation

- [ ] `A2A_BRIDGE_TRANSPORT=nats` production wiring. **Code shipped:** integration harness in `a2a-bridge::nats_transport_harness` (in-process mocks + optional `#[ignore]` live NATS smoke); `stub` remains default for unit tests. **Remaining:** deployed auth-callout mint for production traffic.
- [ ] Operator-signed Account export/import deployment for `a2a.discover.>` cross-Account federation. **Code shipped:** `SpiceDbImportGate` at the SpiceDB-tier import boundary; `SignedDiscoveryExport` envelope + `RealOperatorSignatureGate` keyed by `A2A_DISCOVERY_OPERATOR_KEYS` (with `A2A_DISCOVERY_SIGNATURE_MAX_AGE_SECS` staleness window) running ahead of SpiceDB; `AllowAllOperatorSignatureGate` for labs; reference signer helper. **Remaining:** operator key custody + signed-export distribution pipeline across Accounts.
- [ ] Cross-binding collaboration tests against a live NATS + gateway + bridge. **Remaining:** depends on validating `A2A_BRIDGE_TRANSPORT=nats` + Tier 1 + deployed auth-callout mint.

## Cross-cutting

- [ ] Gateway request path completeness. **Code shipped:** Tier-1 SpiceDB gate (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), Tier-2 CEL evaluator (`A2A_GATEWAY_TIER2_CEL_ENABLED`), authoritative Tier-3 redaction (`A2A_GATEWAY_TIER3_REDACTION_ENABLED`), decision-site audit publish (`trace_id`, `rules_fired`, `rewrites`, `refusal_skill`, `stream_consumer`, `zed_token_snapshot`, `caller_id`, `caller_source`), unary `message.send` deadline (`A2A_GATEWAY_UNARY_DEADLINE_SECS`), JWT-data-claim caller resolution with header-trust fallback. **Remaining:** end-to-end auth-callout verifier in the gateway tier.

---

## Suggested ordering

1. Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` (verifier crate shipped; operator wiring + NSC pipelines remain). Unblocks live JWT-derived `caller_id` population on gateway audits and push DLQ subjects.
2. Stand up signing-key custody and signed-bundle distribution — drives the Tier-3 WASM bundle pipeline and the Tier-1 declarative bundle rollout off the same key material.
3. Stand up operator key custody for `a2a.discover.>` signed exports and run cross-binding collaboration tests against a live NATS + gateway + bridge, then flip `A2A_BRIDGE_TRANSPORT=nats` to production wiring.
