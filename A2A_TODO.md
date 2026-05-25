# A2A over NATS — TODO

Work tracker for the gap between [`A2A_PLAN.md`](./A2A_PLAN.md) and what is in-tree on `yordis/feat-a2a-nats`.

Every item below is open work. Shipped work lives in `A2A_PLAN.md` §Implementation Status — not duplicated here. All architectural decisions are landed (see `A2A_PLAN.md` §Decisions).

Partial items state the in-tree code surface and what remains before the item is fully effective. They stay open until the remaining work lands.

**Design supplements (not duplicate trackers):** [`docs/A2A_AUTH_CALLOUT_DESIGN.md`](./docs/A2A_AUTH_CALLOUT_DESIGN.md), [`docs/A2A_BRIDGE_SKETCH.md`](./docs/A2A_BRIDGE_SKETCH.md), [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md), [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md), [`docs/A2A_GATEWAY_ROADMAP.md`](./docs/A2A_GATEWAY_ROADMAP.md), [`docs/A2A_PUSH_DLQ_OPS.md`](./docs/A2A_PUSH_DLQ_OPS.md), [`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md), [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md), [`docs/A2A_SUBJECT_ACL_QUICKREF.md`](./docs/A2A_SUBJECT_ACL_QUICKREF.md), [`docs/A2A_RUNTIME_ENV.md`](./docs/A2A_RUNTIME_ENV.md), [`docs/A2A_TIER1_DECLARATIVE.md`](./docs/A2A_TIER1_DECLARATIVE.md), [`docs/A2A_TIER2_CEL.md`](./docs/A2A_TIER2_CEL.md), [`docs/A2A_TIER3_REDACTION.md`](./docs/A2A_TIER3_REDACTION.md), [`docs/A2A_DOCS_INDEX.md`](./docs/A2A_DOCS_INDEX.md) (navigation hub).

## Phase 0 — perimeter & catalog

- [ ] Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` against a live NATS account server. **Code shipped** in `a2a-auth-callout` (OIDC / mTLS / HMAC API-key verifiers + Account-bound User JWT mint). **Remaining:** account resolver wiring, signing-key material in a secret store, deployment manifest.
- [ ] Enforce `{prefix}.catalog.register.{agent_id}` ACL-exclusive writes in every environment. **Code shipped** — per-role NSC templates + bootstrap script under `scripts/`. **Remaining:** operator deployment automation to push bindings to the registrar User across environments.
- [ ] NSC operator pipelines — secret-store-backed key handling and automated `nsc push` for per-Account provisioning beyond the idempotent bootstrap script. **Remaining:** end-to-end automation; today only the idempotent script ships.

## Phase 1 — policy & audit

- [ ] Tier 1 declarative policies — bundle tables beyond SpiceDB wired into the gateway request path. **Code shipped:** SpiceDB Tier-1 gate (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), federated `SpiceDbImportGate`, declarative Tier-1 evaluator (`A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED`, default off), and reference bundles under `a2a-pack/policies/` (per-method allowlist, per-agent allowlist, time-of-day approximation). **Remaining:** production rollout (operator-signed bundle distribution per the Tier-3 contract once that's deployed; bundle authoring extensions for true time-of-day predicates).
- [ ] Authoritative JWT-derived `caller_id` on gateway decision-site audits. **Code wired:** `resolve_gateway_caller_identity` verifies **`A2a-Caller-Jwt`** on ingress messages via `MessageCallerIdentitySource`; deprecated **`X-A2a-Spicedb-Principal`** / **`X-A2a-Caller-Id`** apply only when `A2A_GATEWAY_TRUST_CALLER_HEADERS=1` and no verified JWT header is present. **`a2a-bridge`** attaches the minted JWT on publishes to `{prefix}.gateway.>`. **Remaining:** production signing-key custody + publisher rollout.

## Phase 2 — streaming & lifecycle

- [ ] Tier 3 signed-bundle deployment. **Code shipped:** manifest schema (`SkillManifest`), `SkillManifestRegistry`, `SkillSelectionPlan`, reference skill catalog under `a2a-pack/skills/` (`pii-regex-redactor`, `secrets-redactor`, `json-path-sanitizer`); Wasmtime preload via `A2A_GATEWAY_POLICY_BUNDLE_DIR` / `A2A_GATEWAY_POLICY_SKILLS` + `{skill}.manifest.json`; authoritative Tier-3 redaction call site (`A2A_GATEWAY_TIER3_REDACTION_ENABLED`) with refusal (`-32802`), closed-fail engine errors (`-32801`), audit `refusal_skill`, structured telemetry; signed-bundle preload verification (`a2a-redaction::signed_bundle`, `A2A_GATEWAY_TIER3_SIGNING_PUBKEY`) + `a2a-sign-bundle` operator CLI. **Remaining:** signing-key custody + signed-bundle distribution pipeline in production environments.

## Phase 3 — push delivery & redaction

- [ ] End-to-end principal propagation into push DLQ `caller_id`. **Code shipped:** `PrincipalCarrier`, `CallerId::from_principal` over `SpiceDbPrincipal.spicedb_subject`, agent `Bridge` / `message/stream` wiring, gateway-side DLQ mirror (env-gated), gateway-side `A2a-Caller-Jwt` verification surfacing the `SpiceDbPrincipal` for downstream DLQ subjects. **Remaining:** production signing-key custody + publisher rollout so the populated `caller_id` surfaces in DLQ subjects (today's `_` fallback stays active until production publishers attach the JWT header).

## Phase 4 — interop & federation

- [ ] `A2A_BRIDGE_TRANSPORT=nats` production wiring. **Code shipped:** integration harness in `a2a-bridge::nats_transport_harness` (in-process mocks + optional `#[ignore]` live NATS smoke); `stub` remains default for unit tests. **Remaining:** deployed auth-callout mint for production traffic.
- [ ] Operator-signed Account export/import deployment for `a2a.discover.>` cross-Account federation. **Code shipped:** `SpiceDbImportGate` at the SpiceDB-tier import boundary; `SignedDiscoveryExport` envelope + `RealOperatorSignatureGate` keyed by `A2A_DISCOVERY_OPERATOR_KEYS` (with `A2A_DISCOVERY_SIGNATURE_MAX_AGE_SECS` staleness window) running ahead of SpiceDB; `AllowAllOperatorSignatureGate` for labs; reference signer helper. **Remaining:** operator key custody + signed-export distribution pipeline across Accounts.
- [ ] Cross-binding collaboration tests against a live NATS + gateway + bridge. **Remaining:** depends on validating `A2A_BRIDGE_TRANSPORT=nats` + Tier 1 + deployed auth-callout mint.

## Cross-cutting

- [ ] NATS-native publisher mint carrier. **Gap:** `a2a-nats::Client::routing_via_gateway_ingress` publishes to `{prefix}.gateway.*` via `request_with_headers` with only `X-Req-Id` — it has no `MintedUserJwt` in scope, so the gateway falls back to legacy header-trust for these callers. **Decision needed:** add a `MintedUserJwt` carrier on `Client` (re-mint on demand via the existing `BridgeMintAdapter` surface), or document that NATS-native callers must attach `A2a-Caller-Jwt` themselves and provide a helper. Affects `a2a-nats-server` and any direct `Client` user.

---

## Suggested ordering

1. Deploy the auth-callout subscriber on `$SYS.REQ.USER.AUTH` (verifier crate shipped; operator wiring + NSC pipelines remain). Unblocks live JWT-derived `caller_id` population on gateway audits and push DLQ subjects.
2. Stand up signing-key custody and signed-bundle distribution — drives the Tier-3 WASM bundle pipeline and the Tier-1 declarative bundle rollout off the same key material.
3. Stand up operator key custody for `a2a.discover.>` signed exports and run cross-binding collaboration tests against a live NATS + gateway + bridge, then flip `A2A_BRIDGE_TRANSPORT=nats` to production wiring.
