# TODO Backlog — Walk Through One By One

This is the actionable checklist. Order is **dependency-aware** within a phase, then by severity. Every item has a stable ID matching `05-gap-analysis.md`. Strike through (`~~…~~`) as you finish.

> **How to use**: pick the next unchecked item, open a branch `yordis/feat-<id-lower>-<short-name>`, scope the implementation to that item alone, ship a PR titled with the ID. Do not bundle items.

## Phase 0 — Foundations (unblocks everything else)

- [ ] **AU-05** — Partition audit JetStream stream per tenant (one stream per NATS account). Migrate `a2a-nats::audit::emitter` to derive subject from caller account.
- [ ] **TN-01** — Define tenant-scoped audit stream contract (subjects, retention, ACL) in docs + bootstrap script.
- [ ] **PO-05** — Introduce signed policy bundle format (Ed25519 over canonical JSON). Move existing CEL rules into a bundle. Loader refuses unsigned.
- [ ] **SR-05** — Add build-pipeline signing for: policy bundles, MCP catalogs, sentinel WASM, redaction bundles. Single release key, rotated quarterly.
- [ ] **ID-12** — Per-agent kill switch: operator publishes signed event on `governance.kill.<account>.<agent>`; `a2a-auth-callout` invalidates and gateway short-circuits.

## Phase 1 — Identity (foundational for trust + rings)

- [ ] **ID-01** — Surface a stable DID derived from the agent's NKEY: `did:trogon:<nkey-pub-fingerprint>`. Emit in audit, JWT claim, headers.
- [ ] **ID-02** — Record sponsor identity in mint path (`a2a-auth-callout::bridge_mint`). Sponsor JWT signature + DID become claims on minted agent JWT.
- [ ] **ID-09** — Enforce delegation chain narrowing in auth-callout: child JWT scope ⊆ parent scope, TTL ≤ parent TTL, trust ≤ parent trust.
- [ ] **ID-10** — Accept SPIFFE SVID as an upstream identity source. Extend `signing_key_source` + JWT mode.
- [ ] **ID-11** — Automatic key rotation: scheduled re-mint, refusal of long-lived tokens, operator-configurable max TTL per account.
- [ ] **TN-02** — Tenant-scoped operator roles (account-admin vs cross-tenant-auditor) enforced in auth-callout.
- [ ] **DP-01** — Data residency claim on account JWT; gateway routes accordingly.

## Phase 2 — Trust Engine (new crate `trogon-trust`)

- [ ] **ID-03** — Stand up `trogon-trust` crate. Subscribes to audit stream, emits trust deltas to JetStream KV `trust_scores`.
- [ ] **ID-04** — Implement reward dimensions with default weights (policy_compliance .25 / resource_efficiency .15 / output_quality .20 / security_posture .25 / collaboration_health .15). Make weights configurable per account.
- [ ] **ID-05** — Trust decay toward neutral over time, configurable half-life.
- [ ] **ID-13** — Quarantine state machine: trust drop or operator command forces tier=`probationary`, ring=R3, synchronous audit on `governance.quarantine.<account>.<agent>`.
- [ ] **ID-06** — KL-divergence regime change detector; emits alert on `governance.alert.regime_change`.
- [ ] **ID-07** — Network trust propagation (signed-by graph, per-hop decay).

## Phase 3 — Policy Engine (new crate `trogon-policy`)

- [ ] **PO-01** — Define AGT-shaped policy doc schema in `trogon-policy` (metadata / match / conditions / actions / conflict). Closed operator set (eq/ne/gt/lt/gte/lte/in/contains/matches). Pure evaluator.
- [ ] **PO-02** — Folder hierarchy with **deny immutability**: `corporate → tenant → workspace → agent` layering; higher-layer denies are final.
- [ ] **PO-03** — Per-bundle conflict resolution: `DENY_OVERRIDES` (default) / `ALLOW_OVERRIDES` / `PRIORITY_FIRST_MATCH` / `MOST_SPECIFIC_WINS`.
- [ ] **PO-09** — Surface `policy_version` on every decision; gateway pins per-tenant active version.
- [ ] **PO-06** — Canary rollout: percentage or cohort label; resolver picks bundle per request.
- [ ] **PO-07** — Approval workflow service `governance.approval.request` for sensitive actions. Records approver DID in audit.
- [ ] **PO-04** — Backend adapters: SpiceDB (have), OPA, Cedar. Common `Decide` service over NATS.
- [ ] **WI-02** — Policy can mark a route as E2E-opaque; scanners are skipped but metadata still audited.

## Phase 4 — Audit / Compliance Hardening

- [ ] **AU-01** — Hash chain on audit records (`previous_hash` = SHA-256 of prior canonical record). Per-tenant tip recovery on emitter restart.
- [ ] **AU-02** — Decision BOM: extend `AuditEnvelopeFields` with `policy_ids`, `policy_versions`, `bundle_signature`, `conflict_mode`, `rules_fired`, `trust_snapshot`, `catalog_version`.
- [ ] **AU-03** — Tamper-evident fields per record: `arguments_hash`, `result_hash`, `approver_did`, `issued_at`, `started_at`, `completed_at`, gateway Ed25519 `signature`.
- [ ] **AU-09** — RFC 8785 JCS canonicalisation utility in `trogon-std`. Used for every signed payload.
- [ ] **AU-04** — Verifiable receipts: emit `audit.receipt.pre.<tenant>` and `audit.receipt.post.<tenant>`, both signed.
- [ ] **AU-06** — Retention / WORM: JetStream `MaxAge` + `Discard=DiscardOld`; mid-retention deletion requires operator-signed `audit.override` meta-record.
- [ ] **AU-08** — Audit search API `governance.audit.search` (tenant-scoped, paged).
- [ ] **AU-07** — External anchor publication of audit root hash (every N minutes / events).

## Phase 5 — MCP Gateway Hardening

- [ ] **MG-07** — Signed tool catalog. Loader refuses unsigned. Cache locally + KV `tool_catalog`.
- [ ] **MG-05** — Argument-size cap (1 MB default) + schema validation against signed catalog.
- [ ] **MG-04** — Sliding-window rate limit (default 100 / 300 s per `(account, agent, tool)`), state in JetStream KV `rate_limits`.
- [ ] **MG-06** — Per-server circuit breaker (`trogon-std::breaker`), state replicated in KV `circuit_breakers`.
- [ ] **MG-01** — Response-stage scanning pipeline; replies pass through scanners before being published to the caller.
- [ ] **MG-02** — Hidden-instruction scanner on MCP responses (extend `a2a-redaction::tier3_sentinel`).
- [ ] **MG-03** — Credential-leak / PII / exfil-URL scanners in `a2a-redaction`.
- [ ] **MG-11** — Sensitive-tool elevation flow (NATS service `governance.elevation.request`). Gateway gates sensitive tools on a valid elevation token.
- [ ] **MG-12** — Per-session concurrency cap (default 10).
- [ ] **MG-13** — Downstream auth modes: OAuth2 (with device-flow), mTLS, API-key, bearer.

## Phase 6 — MCP Supply Chain (new crates `mcp-drift`, `mcp-cve-watcher`)

- [ ] **MG-08** — Drift detection across 8 categories (name / description / input_schema / output_schema / auth / scope / transport / owner). New crate `mcp-drift`. Emits to `governance.drift.<server>`; activation requires re-approval.
- [ ] **MG-09** — Scanners: TOOL_POISONING, RUG_PULL, CROSS_SERVER_ATTACK, CONFUSED_DEPUTY, HIDDEN_INSTRUCTION, DESCRIPTION_INJECTION. Live in `mcp-drift`.
- [ ] **MG-10** — CVE feed: `mcp-cve-watcher` polls OSV API, writes KV `mcp_cve`, gateway auto-quarantines affected servers.

## Phase 7 — Execution Hypervisor (new crate `trogon-governance`)

- [ ] **EX-01** — Ring model R0–R3 with action-class capabilities and per-ring quotas defined in policy.
- [ ] **EX-02** — JWT claim `ring`; auth-callout sets initial value from trust tier; quarantine forces R3.
- [ ] **EX-03** — Action classification on outbound calls (`read` / `write` / `external` / `privileged`).
- [ ] **EX-04** — Elevation flow: short-lived elevation JWT minted by `governance.elevation`, default TTL 300 s, max 3600 s, sponsor signature required.
- [ ] **EX-06** — Per-ring concurrency + side-effect quotas; enforced in gateway.
- [ ] **EX-07** — Quarantine subject contract documented and wired.
- [ ] **EX-05** — Saga orchestration + compensation in `trogon-decider`. Each step has a compensator; non-compensable steps explicitly flagged.

## Phase 8 — Trust Handshake & Inter-Agent (new crate `trogon-iatp`)

- [ ] **ID-08** — IATP handshake service: challenge-response over NATS request/reply (`iatp.handshake.<peer>`); both peers sign a nonce + scope; success mints a paired session credential.

## Phase 9 — SRE Governance

- [ ] **SR-03** — Add circuit-breaker primitive to `trogon-std`.
- [ ] **SR-01** — SLOs declared in policy bundle metadata; evaluated against `trogon-telemetry` metrics.
- [ ] **SR-02** — Error budgets + burn alerts; freeze rollouts on budget burn.
- [ ] **SR-04** — Chaos + golden-trace harness (`governance-chaos`).

## Phase 10 — Data Protection

- [ ] **DP-02** — Egress allowlist + signed egress proxy (`trogon-egress-proxy`); per-agent DNS/CIDR allowlists.
- [ ] **WI-01** — Optional payload E2E (X3DH + Double Ratchet) for sensitive agent↔agent routes.

## Phase 11 — Standards Crosswalks (docs)

- [ ] **TM-01** — STRIDE threat model per crate (gateway / auth-callout / decider / audit / redaction / trust / policy).
- [ ] **CS-01** — OWASP Agentic Top 10 crosswalk.
- [ ] **CS-02** — NIST AI RMF crosswalk.
- [ ] **CS-03** — EU AI Act high-risk obligations crosswalk.
- [ ] **CS-04** — SOC 2 CC crosswalk.
- [ ] **CS-05** — ISO 42001 crosswalk.

## Cross-Cutting Requirements

- Every new subject MUST be documented in `docs/a2a/` (or a new `docs/governance/` set) with: producer, consumer, payload schema, retention.
- Every new JWT claim MUST be added to `a2a-auth-callout`'s claim allowlist and documented.
- Every new audit field MUST extend `AuditEnvelopeFields` and ship a migration note.
- Every new policy operator or evaluator change MUST keep evaluation pure and total.
- Every new artifact (bundle, catalog, WASM, baseline) MUST be signed by the release pipeline.

---

**Total checklist items: 59.** Walk them one by one.
