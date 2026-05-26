# NATS-Native Translation Of AGT Controls

AGT is process-local Python middleware. TrogonStack is a NATS substrate. This document is how each AGT control re-projects onto subjects, accounts, JetStream, services, and account-resolver mints. It is the implementation north star.

## 0. Architectural Invariants We Preserve

- **The agent process is untrusted.** Every guarantee lives outside the agent — in the gateway, the auth-callout, the decider, the audit stream.
- **A tenant is a NATS account.** Account boundary is the security boundary.
- **A capability is a subject pattern.** Allow/deny decisions are subject-shaped.
- **Audit is JetStream.** Append-only, hash-chained, per-tenant streams.
- **Identity is a NATS user JWT** signed by the account, minted by `a2a-auth-callout`, optionally backed by a SPIFFE SVID or upstream JWT.

## 1. Identity & Trust

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Ed25519 agent keypair                | NATS user NKEY (already ed25519). The NKEY public form is the agent's identifier.                                  |
| `did:mesh:<fingerprint>`             | Derived deterministically from the NKEY public, exposed as a stable string in audit + headers. `did:trogon:<nkey>` |
| Sponsor binding                      | Account JWT mint by `a2a-auth-callout::bridge_mint` records the sponsor (operator JWT or parent agent JWT).        |
| Credential TTL                       | NATS user JWT `exp`. Default ≤ 15 min, configurable per agent.                                                     |
| Trust score                          | New crate `trogon-trust`: subscribes to audit stream, emits `trust.<account>.<agent>` updates.                     |
| Tier (verified_partner / trusted / standard / probationary / untrusted) | Snapshot persisted to JetStream KV `trust_tiers`; consulted by gateway via local cache.            |
| KL regime change                     | `trogon-trust` periodic job; emits alert on subject `governance.alert.regime_change`.                              |
| IATP handshake                       | NATS request/reply on `iatp.handshake.<peer>` — challenge/response exchanged as signed payloads.                   |
| Delegation chain                     | Each downstream JWT MUST carry a `delegated_from` claim and `scope ⊆ parent_scope`; verified by auth-callout.      |
| SPIFFE SVID                          | New trust source in `a2a-auth-callout/src/signing_key_source` + `jwt` — SVID treated as upstream identity claim.   |
| Kill switch                          | Operator publishes signed revoke to `governance.kill.<account>.<agent>`; auth-callout invalidates active sessions. |
| Quarantine                           | Trust tier forced to `probationary`; ring forced to R3; published on `governance.quarantine.<account>.<agent>`.    |

## 2. Policy

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Policy document schema               | Extend `trogon-mcp-gateway::policy` with AGT-shaped bundle: metadata / match / conditions / actions / conflict.    |
| Operator set                         | CEL satisfies most; add native AGT-shaped DSL adapter that compiles to CEL or to OPA/Cedar.                        |
| Folder hierarchy + deny immutability | Bundles are layered: `corporate → tenant → workspace → agent`. Deny in a higher layer is final.                    |
| Conflict resolution                  | Bundle metadata names one of the four modes; evaluator honours.                                                    |
| Pluggable backend                    | New crate `trogon-policy` with traits for SpiceDB / OPA / Cedar adapters. Gateway calls `Decide` over NATS service `policy.decide`. |
| Signed bundles                       | Reuse `a2a-redaction::signed_bundle` pattern: Ed25519-signed tarball, version-tagged, published over object-store. |
| Canary                               | Subject-scoped activation: `policy.bundle.<id>.canary` vs `…active`; gateway resolves per-tenant pinned version.   |
| Decision recording                   | Every decision posts to `audit.policy.<tenant>` with policy IDs + versions + matched rules.                        |

## 3. MCP Gateway

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Dual-stage pipeline                  | `trogon-mcp-gateway` already mediates request; add response-scan stage before publishing reply onto caller's subj. |
| Deny / allow lists                   | Existing SpiceDB allow/deny is the substrate. Surface AGT-shaped tool IDs via subject convention `mcp.<srv>.<tool>`. |
| Sensitive-tool elevation             | New service `governance.elevation.request` (NATS service); responds with signed elevation token; gateway gates on it. |
| Rate limit (sliding 100/300s)        | Token-bucket in gateway, keyed by `(account, agent, tool)`. State in JetStream KV `rate_limits` for HA.            |
| Schema validation                    | Validate against signed catalog (`tool_catalog.signed`) before forwarding to MCP server.                            |
| Response scanners                    | Run on the gateway reply path; can also be a separate `tier3-sentinel`-style WASM bundle.                          |
| Hidden-instruction scanner           | Extend `a2a-redaction::tier3_sentinel` to handle MCP responses, not only A2A.                                      |
| CVE feed                             | Sidecar service `mcp-cve-watcher` subscribes to OSV API → writes JetStream KV `mcp_cve` → gateway consults.        |
| Drift detection                      | Catalog updates emit diff onto `governance.drift.<server>`; reviewer service or human approves before activation.  |
| Circuit breaker                      | Gateway-local; state replicated to JetStream KV for HA failover.                                                   |
| Catalog signing                      | Catalog stored in object store with Ed25519 signature; loader refuses unsigned.                                    |

## 4. Execution Hypervisor

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Rings R0–R3                          | Encoded as JWT claim `ring`; auth-callout sets initial value from trust tier; gateway forces R3 when quarantined.  |
| Ring → allowed action classes        | Mapped in `trogon-policy`; policy match keys on `ring` claim.                                                      |
| Elevation tokens                     | Short-lived JWT minted by `governance.elevation` service with `elevation_to` and `expires_at`; gateway honours.    |
| Saga orchestration + compensation    | Extend `trogon-decider` with explicit saga semantics: each step has a compensating decider; orchestrator service `governance.saga` runs them. |
| Action classification                | New claim `action_class` on outbound calls (`read` / `write` / `external` / `privileged`); inferred at gateway or asserted by agent. |
| Quotas per ring                      | Token bucket + concurrency cap by ring; persisted in JetStream KV.                                                 |

## 5. Audit & Compliance

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Append-only log                      | Existing `a2a-nats::audit::emitter` publishes to JetStream stream `audit.<tenant>`.                                |
| Hash chain                           | Each emitted record carries `previous_hash` (SHA-256 of canonical prior record). Emitter holds in-memory tip; recovers from JetStream on restart. |
| Decision BOM                         | Extend `AuditEnvelopeFields` to include `policy_ids`, `policy_versions`, `bundle_signature`, `conflict_mode`, `rules_fired`, `trust_snapshot`, `catalog_version`. |
| Tamper-evident fields                | Add `arguments_hash` (SHA-256 of canonical JSON args), `result_hash`, `approver_did`, `signature` (gateway Ed25519). |
| Verifiable receipts                  | Two records per action: `audit.receipt.pre.<tenant>` and `audit.receipt.post.<tenant>`, both Ed25519-signed, JCS-canonicalised. |
| Per-tenant isolation                 | One audit stream per account; account-scoped consumers; no cross-account queries.                                  |
| WORM / retention                     | JetStream `MaxAge` + `Discard=DiscardOld` disallowed mid-retention; delete-before-expiry needs operator-signed `audit.override` event. |
| External anchoring                   | Root hash published every N minutes to a public anchor (e.g. via signed event on `audit.root.<tenant>`).            |
| Search API                           | New service `governance.audit.search` over NATS request/reply with tenant-scoped paging.                            |

## 6. SRE Governance

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| SLOs                                 | Declared in policy bundle metadata; evaluated against `trogon-telemetry` metrics; emit budget-burn events.         |
| Circuit breakers                     | See §3. Shared library `trogon-std::breaker`.                                                                       |
| Chaos hooks                          | New service `governance.chaos` injects faults onto designated subjects in non-prod tenants; verifies audit + denial. |
| Golden traces                        | OTel traces stored as signed reference artifacts; new diff tool `governance.trace.diff`.                            |
| Artifact signing                     | Build pipeline signs policy bundles, catalogs, sentinel WASM, golden traces with a release key.                     |

## 7. Wire / E2E

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| X3DH + Double Ratchet                | Optional payload encryption layer between two agents. Implemented above NATS as message-format conventions; gateway sees opaque bytes; metadata still policed. |
| ChaCha20-Poly1305                    | Same.                                                                                                              |

E2E remains opt-in: it removes the gateway's ability to scan content, so policy must explicitly mark such routes.

## 8. Threat Model & Tenancy

| AGT control                          | NATS-native form                                                                                                   |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| STRIDE per component                 | New `docs/security/threat-model.md` modelled per crate (gateway, auth-callout, decider, audit, redaction).         |
| Tenant isolation                     | NATS account = tenant. Audit stream per account. Policy bundle per account. Egress policy per account. Redaction bundle per account. |
| Operator role scoping                | Operator JWT scopes (account-admin / cross-tenant-auditor) enforced at auth-callout.                                |
| Data residency                       | Account-level claim `residency=<region>`; gateway routes only to MCP servers tagged for that region.               |
| Egress allowlist                     | Subject allowlist per agent; outbound HTTP via signed egress proxy when needed.                                    |

## 9. Standards Mapping

Crosswalks live in `docs/research/microsoft-agent-gov-toolkit/standards/` (future): OWASP Agentic Top 10, NIST AI RMF, EU AI Act high-risk obligations, SOC 2 CC controls, ISO 42001 clauses. Each row points to the TrogonStack control that satisfies it.
