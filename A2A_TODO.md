# A2A over NATS — TODO

Work tracker for the gap between [`A2A_PLAN.md`](./A2A_PLAN.md) and what is in-tree on `yordis/feat-a2a-nats`.

Legend: `[ ]` open · `[~]` partial.

All architectural decisions are landed (see `A2A_PLAN.md` §Decisions). Items below are pure engineering work. Shipped work lives in `A2A_PLAN.md` §Implementation Status — not duplicated here.

**Design supplements (not duplicate trackers):** [`docs/A2A_AUTH_CALLOUT_SKETCH.md`](./docs/A2A_AUTH_CALLOUT_SKETCH.md), [`docs/A2A_BRIDGE_SKETCH.md`](./docs/A2A_BRIDGE_SKETCH.md), [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md), [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md), [`docs/A2A_GATEWAY_ROADMAP.md`](./docs/A2A_GATEWAY_ROADMAP.md), [`docs/A2A_PUSH_DLQ_OPS.md`](./docs/A2A_PUSH_DLQ_OPS.md), [`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md), [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md), [`docs/A2A_SUBJECT_ACL_QUICKREF.md`](./docs/A2A_SUBJECT_ACL_QUICKREF.md), [`docs/A2A_RUNTIME_ENV.md`](./docs/A2A_RUNTIME_ENV.md), [`docs/A2A_DOCS_INDEX.md`](./docs/A2A_DOCS_INDEX.md) (navigation hub).

## Phase 0 — perimeter & catalog

- [~] AgentCard catalog (catalog modules + `a2a-nats-discovery` process provisions KV + discover + registrar; one KV bucket per Account).
  - [~] AgentCard JSON-Schema validation on write (canonical schema in **`a2a-pack`**; enforced on **`KvCatalogStore::put_card`**). **Registrar ingress:** **`{prefix}.catalog.register.{agent_id}`** NATS **`request`** with raw AgentCard JSON body → KV put (`catalog::CatalogRegistrarService` in **`a2a-nats-discovery`**). **ACL-exclusive writes** still land with NSC + dedicated registrar User (operators only).
  - [~] **`KvCatalogStore::get_card`** rejects stale/malformed KV documents against the bundled schema (defense in depth — same schema as **`a2a-pack`**).
  - [ ] Gateway / edge path re-validates AgentCard JSON-Schema on read once the gateway materializes AgentCards outside NATS KV.
- [ ] Auth callout service — NATS subscriber on `$SYS.REQ.USER.AUTH`; OIDC primary, mTLS for service-to-service, API keys transitional. Mints Account-bound User JWT (`sub` external, `aud` = Account, SpiceDB principal in `data`).
- [ ] Subject ACL inside each Account — bound caller User to `a2a.gateway.>` and `_INBOX.{caller_id}.>`; bound gateway User to `a2a.agent.>` + `a2a.task.>` + `a2a.push.>`.
- [~] NSC operator/account provisioning runbook — expanded ACL table (caller / gateway / registrar) + JetStream steps; **[`docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md)**. Landed: idempotent bootstrap script ([`scripts/a2a-nsc-bootstrap.sh`](./scripts/a2a-nsc-bootstrap.sh)), per-role ACL templates ([`scripts/acl-templates/`](./scripts/acl-templates/)), and federated discovery export scaffold ([`scripts/a2a-nsc-export-discovery.sh`](./scripts/a2a-nsc-export-discovery.sh)). Still future: CI/CD integration, secret-store-backed key handling, and automated `nsc push` in operator pipelines.

## Phase 1 — policy & audit

- [ ] Tier 1 declarative policies wired into the gateway request path.
- [ ] SpiceDB integration — gateway client to org-standard cluster; `BulkCheckPermission` for catalog shaping; per-method resource tuples; owner tuples on task lifecycle; ZedToken cache per session.
- [~] Audit emitter (RPC envelopes + streamed `TaskLifecycleEnvelope` shipped; see plan).
  - [ ] Gateway decision sites (`rules_fired`, allow/deny, rewrites — land with auth callout + Tier 1).
  - [~] **`AuditEnvelope` JSON** carries optional **`trace_id`**, **`rules_fired`**, **`rewrites`**, **`stream_consumer`** (`AuditEnvelopeFields`; bridge defaults empty). Gateway/path wiring to populate stays future.

## Phase 2 — streaming & lifecycle

- [ ] Policy substrate — single Wasmtime runtime in the gateway hosting Tier 2 (CEL compiled to WASM at bundle build) and Tier 3 (WASM redaction).
- [ ] Streaming back-pressure — gateway pull consumer with flow control; `A2A_EVENTS` policy `retention=interest, discard=old` (so agents never block on publish). Ops/design: [`docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](./docs/A2A_STREAMING_BACKPRESSURE_OPS.md).
- [ ] `message/send` 30s gateway deadline; longer work transitions to `message/stream`.

## Phase 3 — push delivery & redaction

- [~] Push dispatcher (HTTP/`subject:`/`jetstream:` shipped).
  - [~] `pushNotificationAuthenticationInfo` — Bearer/Basic/jwt→Bearer ship; Digest deferred.
  - [ ] Exactly-once opt-in flag on `PushNotificationConfig` (double-ack + idempotency keys; default remains at-least-once) — see [`docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md`](./docs/A2A_PUSH_EXACTLY_ONCE_SKETCH.md).
  - [~] Cross-process DLQ — per-Account **`A2A_PUSH_DLQ`** JetStream stream (**`provision_streams`**); agent **`Bridge`** **`message/stream`** terminal push path **JetStream-publishes** JSON failures (**`schema`=`a2a.push.dlq/v1`**) to **`{prefix}.push.dlq.{caller_id}.{task_id}`** (default **`caller_id`** segment **`_`** via **`Config`** until auth propagation). **`a2a-gateway`** ingress does **not** own DLQ. Remaining: optional gateway-side duplication, richer **`caller_id`** from minted User JWT everywhere.
- [ ] Tier 3 WASM redaction over `Message.parts` / `Artifact.parts`, skill-id keyed.

## Phase 4 — interop & federation

- [ ] `a2a-bridge` crate (HTTPS↔NATS sidecar) — terminates HTTPS auth, calls auth-callout, obtains per-request User in caller's tenant Account; publishes on `a2a.gateway.{agent_id}.{method}`; maps SSE↔JetStream consumer on `a2a.task.{task_id}.events.>`. Symmetric inbound HTTPS-agent registration path.
- [ ] Federated discovery — operator-signed Account export contract for `a2a.discover.>`; SpiceDB gating at the import boundary. Sketch: [`docs/A2A_FEDERATED_DISCOVERY_SKETCH.md`](./docs/A2A_FEDERATED_DISCOVERY_SKETCH.md).
- [ ] Cross-binding collaboration tests (`a2a-bridge` prerequisite).

## Cross-cutting

- [~] `a2a-gateway` — ingress relay ships; structured tracing fields on ingress forward path; auth callout / policy / audit hooks pending.

---

## Suggested ordering

1. Flesh NSC automation (beyond [`docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md)) + Account-internal subject ACL templates.
2. Auth callout service — mint Account-bound User JWTs.
3. Flesh `a2a-gateway` — auth-callout integration + decision-site audit hooks.
4. SpiceDB + Tier 1 policy.
5. CEL Tier 2 + WASM Tier 3 (shared Wasmtime substrate in gateway).
6. Hardened push — **`A2A_PUSH_DLQ` agent publishes shipped** (**`push::dlq`**); exactly-once opt-in, JWT-derived **`caller_id`** everywhere, optional gateway-side DLQ mirroring outstanding.
7. `a2a-bridge` + federated discovery exports.
