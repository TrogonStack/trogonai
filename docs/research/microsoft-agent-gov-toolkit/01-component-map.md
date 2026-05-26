# AGT ↔ TrogonStack Component Map

This document maps every named AGT component to its TrogonStack counterpart (existing or missing). It is the source of truth for `05-gap-analysis.md` and `06-todo-backlog.md`.

Legend: ✅ implemented · 🟡 partial / scaffolded · ❌ missing · 🚫 deliberately not adopted (NATS-native replaces it).

## 1. Policy Engine (`AGENT-OS-POLICY-ENGINE-1.0`)

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| Policy doc schema (`metadata/match/conditions/actions`) | `rsworkspace/crates/trogon-mcp-gateway/src/policy/` (CEL-shaped)         | 🟡     |
| Operators (`eq/ne/gt/lt/gte/lte/in/contains/matches`)   | CEL covers these; no native AGT-shaped DSL                                | 🟡     |
| Actions (`allow/deny/audit/block`)     | SpiceDB `PermissionChecker` returns allow/deny; `audit` channel separate | 🟡     |
| Folder-level hierarchy + deny immutability invariant    | Not modeled — single policy bundle today                                  | ❌     |
| Conflict resolution (DENY_OVERRIDES / ALLOW_OVERRIDES / PRIORITY_FIRST_MATCH / MOST_SPECIFIC_WINS) | SpiceDB resolves at relation level; no explicit operator selection         | ❌     |
| OPA / Cedar backend pluggability       | SpiceDB only; no OPA/Cedar adapter                                       | ❌     |
| Fail-closed default                    | Gateway denies on PDP error (`authz.rs`)                                 | ✅     |
| Policy versioning + canary             | None                                                                     | ❌     |

## 2. Identity & Trust (`AGENTMESH-IDENTITY-TRUST-1.0`)

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| Ed25519 agent keypair                  | NATS account NKEY (ed25519) per agent via `a2a-auth-callout`             | ✅     |
| `did:mesh:` / `did:agentmesh:` DIDs    | Agents are NATS subjects, not DIDs                                       | 🚫→❌  |
| Sponsor binding (parent agent signs creation) | No equivalent; account mint is operator-signed only                       | ❌     |
| Credential TTL (15 min) + rotation     | NATS user JWTs minted with TTL via `a2a-auth-callout::bridge_mint`        | 🟡     |
| Trust tiers (verified_partner ≥900 / trusted ≥700 / standard ≥500 / probationary ≥300 / untrusted 0) | Not modeled                                                              | ❌     |
| Reward dimensions (policy / efficiency / quality / security / collaboration) | Not modeled                                                              | ❌     |
| Trust decay over time                  | Not modeled                                                              | ❌     |
| KL-divergence regime change detection  | Not modeled                                                              | ❌     |
| Network trust propagation              | Not modeled                                                              | ❌     |
| IATP handshake (challenge-response)    | NATS auth-callout + JWT serves auth, not relational trust                | ❌     |
| Delegation chain with monotonic narrowing | JWT issuance is flat                                                     | ❌     |
| Trust ceiling (delegate ≤ delegator)   | Not modeled                                                              | ❌     |
| SPIFFE / SVID interop                  | JWT mode supports JWKS, not SPIFFE                                       | 🟡     |
| Key rotation policy                    | Manual                                                                   | ❌     |

## 3. MCP Security Gateway (`MCP-SECURITY-GATEWAY-1.0`)

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| Dual-stage pipeline (request → response) | `trogon-mcp-gateway::gateway` request path only                          | 🟡     |
| Tool deny / allow lists                | Subject-level allow/deny via SpiceDB                                     | ✅     |
| Sensitive-tool elevation prompt        | Not implemented                                                          | ❌     |
| Sliding-window rate limit (100 / 300 s) | Not implemented                                                          | ❌     |
| Response scanning (instruction tags / credential leak / PII / exfil URLs) | a2a-redaction WASM bundle redacts but does not classify as threats        | 🟡     |
| Tool-poisoning detector                | Not implemented                                                          | ❌     |
| Rug-pull / schema-drift detector       | Not implemented                                                          | ❌     |
| Cross-server attack detector           | Not implemented                                                          | ❌     |
| Confused-deputy detector               | Not implemented                                                          | ❌     |
| Hidden-instruction detector            | a2a-redaction tier-3 sentinel (`tier3_sentinel.rs`) partially overlaps    | 🟡     |
| Description-injection detector         | Not implemented                                                          | ❌     |
| HMAC request signing                   | NATS account JWT auth instead                                            | 🚫     |
| Session auth (1h TTL, 10 concurrent)   | Per-session JWT, no concurrency cap                                      | 🟡     |
| Auth modes (oauth2/mtls/api_key/bearer/none) | JWT-only; OAuth2 device-flow planned                                     | 🟡     |
| CVE feed (OSV API)                     | Not implemented                                                          | ❌     |
| Trust-gated MCP (1 MB arg, schema validate, circuit breaker) | Schema validation partial; circuit breaker absent                         | 🟡     |
| Drift detection (8 categories: name/desc/input-schema/output-schema/auth/scope/transport/owner) | Not implemented                                                          | ❌     |
| Tool catalog freshness / signing       | `trogon-mcp-gateway::catalog` exists but unsigned                        | 🟡     |

## 4. Hypervisor / Execution Control (`AGENT-HYPERVISOR-EXECUTION-CONTROL-1.0`)

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| 4 execution rings (R0–R3)              | Not modeled                                                              | ❌     |
| Ring assignment by trust score         | Not modeled                                                              | ❌     |
| Privilege elevation (300 s default, 3600 s max) | Not modeled                                                              | ❌     |
| Saga orchestration / compensation      | `trogon-decider` is a decider framework but not saga-shaped              | 🟡     |
| Kill switch                            | NATS account disable is global; per-agent kill not surfaced              | 🟡     |
| Quarantine state                       | Not modeled                                                              | ❌     |
| Action classification (read/write/external/privileged) | Not modeled                                                              | ❌     |
| Resource quotas per ring               | Not modeled                                                              | ❌     |

## 5. Audit / Compliance (`AUDIT-COMPLIANCE-1.0`)

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| Append-only audit log                  | `a2a-nats::audit` emits JetStream events                                 | ✅     |
| Merkle hash chain (tamper-evident)     | Not implemented                                                          | ❌     |
| Decision BOM (bill-of-materials per decision) | `AuditEnvelope` includes rules_fired / zed_token but not full BOM         | 🟡     |
| Tamper-evident fields (`arguments_hash`, `approver_did`, `policy_version`, `issued_at`, `completed_at`) | Partial (subject rewrites, tier1/tier3 decision, caller id)               | 🟡     |
| Verifiable compliance receipts (Ed25519, pre+post sig, RFC 8785 JCS) | Not implemented                                                          | ❌     |
| Per-tenant audit isolation             | All audit on single subject prefix today                                 | ❌     |
| Audit retention policy / WORM          | Driven by JetStream config; not formalised                               | 🟡     |
| Audit search API                       | Not implemented                                                          | ❌     |

## 6. SRE Governance (`AGENT-SRE-GOVERNANCE-1.0`)

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| SLOs per agent / per tool              | `trogon-telemetry` collects metrics; SLO targets not declared            | 🟡     |
| Error budgets                          | Not modeled                                                              | ❌     |
| Circuit breakers                       | Not implemented                                                          | ❌     |
| Chaos engineering hooks                | Not implemented                                                          | ❌     |
| Golden traces                          | OTel traces emitted; no "golden" baseline comparison                     | 🟡     |
| Artifact signing (policy bundles, catalogs) | `a2a-redaction::signed_bundle` exists for redaction; not generalised      | 🟡     |

## 7. Wire Protocol (`AGENTMESH-WIRE-1.0`)

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| X3DH initial key agreement             | NATS TLS + account JWT instead                                           | 🚫     |
| Double Ratchet forward secrecy         | Not applicable to bus traffic; relevant for agent↔agent payload secrecy  | 🚫→🟡 |
| ChaCha20-Poly1305 payload encryption   | TLS terminates at NATS; end-to-end agent encryption not offered          | ❌     |

## 8. Threat Model & Tenancy

| AGT Concept                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| STRIDE table per component             | Not authored                                                             | ❌     |
| K8s namespace isolation                | Out of scope (NATS account boundary is our isolation)                    | 🚫     |
| Trust store separation per tenant      | NATS account == tenant; OK                                               | ✅     |
| Audit log per tenant                   | See §5 — not implemented                                                 | ❌     |
| Data residency controls                | Not modeled                                                              | ❌     |
| Egress allowlists                      | Subject-level allow lists, but no DNS/CIDR egress policy                 | ❌     |

## 9. Standards Alignment

| AGT Mapping                            | TrogonStack                                                              | Status |
| -------------------------------------- | ------------------------------------------------------------------------ | ------ |
| OWASP Agentic Top 10 (ASI-01…10)       | Not mapped                                                               | ❌     |
| NIST AI RMF                            | Not mapped                                                               | ❌     |
| EU AI Act high-risk obligations        | Not mapped                                                               | ❌     |
| SOC 2 control crosswalk                | Not mapped                                                               | ❌     |
| ISO 42001                              | Not mapped                                                               | ❌     |

The compliance crosswalk is the single most under-invested area. Enterprises will not buy without it.
