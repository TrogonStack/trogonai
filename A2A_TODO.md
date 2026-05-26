# A2A over NATS — TODO

Closed tracker for the gap between [`A2A_PLAN.md`](./A2A_PLAN.md) and what is in-tree on `yordis/feat-a2a-nats`. All in-tree code work has shipped, and every operator-surface item below is exercised end-to-end by `make smoke` / `make smoke-full` (compose stacks under [`devops/docker/compose/`](./devops/docker/compose/) + the `a2a-smoke-test` crate). Local docker compose is the proof of correctness for this branch — production fleet rollout is out of scope for this tracker.

**Design supplements (not duplicate trackers):** [`docs/A2A_AUTH_CALLOUT_DESIGN.md`](./docs/A2A_AUTH_CALLOUT_DESIGN.md), [`docs/A2A_BRIDGE_SKETCH.md`](./docs/A2A_BRIDGE_SKETCH.md), [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md), [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md), [`docs/A2A_GATEWAY_ROADMAP.md`](./docs/A2A_GATEWAY_ROADMAP.md), [`docs/A2A_PUSH_DLQ_OPS.md`](./docs/A2A_PUSH_DLQ_OPS.md), [`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md), [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md), [`docs/A2A_SUBJECT_ACL_QUICKREF.md`](./docs/A2A_SUBJECT_ACL_QUICKREF.md), [`docs/A2A_RUNTIME_ENV.md`](./docs/A2A_RUNTIME_ENV.md), [`docs/A2A_TIER1_DECLARATIVE.md`](./docs/A2A_TIER1_DECLARATIVE.md), [`docs/A2A_TIER2_CEL.md`](./docs/A2A_TIER2_CEL.md), [`docs/A2A_TIER3_REDACTION.md`](./docs/A2A_TIER3_REDACTION.md), [`docs/A2A_DOCS_INDEX.md`](./docs/A2A_DOCS_INDEX.md) (navigation hub).

## Phase 0 — perimeter & catalog

- [x] Auth-callout subscriber on `$SYS.REQ.USER.AUTH`. `a2a-auth-callout` runs against the smoke NATS server (`compose.a2a.smoke.yml` service `a2a-auth-callout`) with OIDC / mTLS / HMAC API-key verifiers and Account-bound User JWT mint; bootstrap supplies signing-key material and account resolver wiring.
- [x] `{prefix}.catalog.register.{agent_id}` ACL-exclusive writes. Bootstrap applies per-role NSC bindings from `scripts/acl-templates/{caller,gateway,registrar}.acl` via `scripts/a2a-nsc-bootstrap.sh`; smoke exercises registrar-only writes end-to-end.
- [x] NSC operator pipelines. `scripts/a2a-nsc-bootstrap.sh` runs idempotently inside the `a2a-bootstrap` service and materializes per-Account bindings for caller / gateway / registrar.

## Phase 1 — policy & audit

- [x] Tier 1 declarative policies. SpiceDB Tier-1 (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`), federated `SpiceDbImportGate`, declarative Tier-1 (`A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED`) + reference bundles under `a2a-pack/policies/` (including clock-aware time-of-day) all exercised in `make smoke-full`.
- [x] Authoritative JWT-derived `caller_id` on gateway decision-site audits. `resolve_gateway_caller_identity` verifies `A2a-Caller-Jwt` on ingress; `a2a-bridge` attaches the minted JWT on publishes; `make smoke` asserts `A2A_SMOKE_EXPECTED_CALLER_ID=smoke-caller-1`.

## Phase 2 — streaming & lifecycle

- [x] Tier 3 signed-bundle deployment. Manifest schema, registry, selection plan, reference skill catalog under `a2a-pack/skills/`, Wasmtime preload via `A2A_GATEWAY_POLICY_BUNDLE_DIR` / `A2A_GATEWAY_POLICY_SKILLS`, authoritative redaction call site (`A2A_GATEWAY_TIER3_REDACTION_ENABLED`), signed-bundle preload verification (`A2A_GATEWAY_TIER3_SIGNING_PUBKEY`) + `a2a-sign-bundle` operator CLI all wired in bootstrap; smoke-full preloads a signed bundle.

## Phase 3 — push delivery & redaction

- [x] End-to-end principal propagation into push DLQ `caller_id`. `PrincipalCarrier`, `CallerId::from_principal`, agent `Bridge` / `message/stream` wiring, gateway DLQ mirror, and JWT verification on ingress validated in smoke compose with live auth-callout mint.

## Phase 4 — interop & federation

- [x] `A2A_BRIDGE_TRANSPORT=nats` wiring. Bootstrap sets `A2A_BRIDGE_TRANSPORT=nats`; `a2a-bridge` service exercises the path against the live auth-callout mint.
- [x] Operator-signed Account export/import for `a2a.discover.>` cross-Account federation. `SignedDiscoveryExport` envelope + `RealOperatorSignatureGate` keyed by `A2A_DISCOVERY_OPERATOR_KEYS` (bootstrap-provisioned in smoke-full) running ahead of SpiceDB; reference signer helper ships.
- [x] Cross-binding collaboration tests. `make smoke-full` exercises NATS + gateway + bridge + auth-callout + spicedb together via the `a2a-smoke-test` `full` profile.
