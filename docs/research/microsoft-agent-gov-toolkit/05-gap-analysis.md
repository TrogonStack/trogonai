# Gap Analysis

Each row is a gap between AGT's stated control set and TrogonStack today. Severity uses CVSS-style buckets: **Critical** = blocks enterprise sale; **High** = needed within first three customer engagements; **Medium** = important but workable around; **Low** = nice-to-have.

Owner crate = where the work lands. ☆ = new crate.

## 1. Identity & Trust

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| ID-01 | No DID-shaped identifier surfaced from NKEY                | High     | `a2a-auth-callout`                       |
| ID-02 | No sponsor binding recorded in mint                        | High     | `a2a-auth-callout`                       |
| ID-03 | No formal trust score / tier engine                        | Critical | ☆ `trogon-trust`                         |
| ID-04 | No reward dimensions or weights                            | Critical | ☆ `trogon-trust`                         |
| ID-05 | No trust decay                                             | High     | ☆ `trogon-trust`                         |
| ID-06 | No KL-divergence regime detection                          | Medium   | ☆ `trogon-trust`                         |
| ID-07 | No network trust propagation                               | Medium   | ☆ `trogon-trust`                         |
| ID-08 | No IATP handshake service                                  | High     | ☆ `trogon-iatp`                          |
| ID-09 | Delegation chain not enforced (scope/TTL/trust narrowing)  | Critical | `a2a-auth-callout`                       |
| ID-10 | SPIFFE / SVID not accepted as identity source              | High     | `a2a-auth-callout`                       |
| ID-11 | No automatic key rotation policy                           | High     | `a2a-auth-callout`                       |
| ID-12 | No per-agent kill switch surface                           | Critical | ☆ `trogon-governance` (or `a2a-auth-callout`) |
| ID-13 | No quarantine state machine                                | High     | ☆ `trogon-governance`                    |

## 2. Policy

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| PO-01 | No AGT-shaped policy document schema                       | High     | `trogon-mcp-gateway::policy`             |
| PO-02 | No folder hierarchy / deny immutability                    | Critical | ☆ `trogon-policy`                        |
| PO-03 | Conflict-resolution mode not selectable per bundle         | High     | ☆ `trogon-policy`                        |
| PO-04 | No OPA / Cedar backend adapter                             | Medium   | ☆ `trogon-policy`                        |
| PO-05 | No signed policy bundles                                   | Critical | ☆ `trogon-policy`                        |
| PO-06 | No canary rollout                                          | High     | ☆ `trogon-policy`                        |
| PO-07 | Approval workflow for sensitive actions absent             | High     | ☆ `trogon-governance`                    |
| PO-08 | Decisions don't carry full BOM                             | Critical | `a2a-nats::audit`                        |
| PO-09 | Policy versioning not surfaced on every decision           | High     | `trogon-mcp-gateway`                     |

## 3. MCP Gateway

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| MG-01 | No response-stage scanning pipeline                        | Critical | `trogon-mcp-gateway`                     |
| MG-02 | No prompt-injection / hidden-instruction scanner for MCP responses | Critical | `a2a-redaction::tier3_sentinel` (extend) |
| MG-03 | No credential-leak / PII / exfil-URL scanners              | Critical | `a2a-redaction`                          |
| MG-04 | No sliding-window rate limiter                             | High     | `trogon-mcp-gateway`                     |
| MG-05 | No 1 MB / schema-validation guard rails                    | High     | `trogon-mcp-gateway`                     |
| MG-06 | No per-server circuit breaker                              | High     | `trogon-mcp-gateway` + `trogon-std`      |
| MG-07 | No signed tool catalog                                     | Critical | `trogon-mcp-gateway::catalog`            |
| MG-08 | No drift detection (8 categories)                          | Critical | ☆ `mcp-drift`                            |
| MG-09 | No tool-poisoning / rug-pull / cross-server / confused-deputy / description-injection scanners | Critical | ☆ `mcp-drift`                            |
| MG-10 | No CVE feed integration                                    | High     | ☆ `mcp-cve-watcher`                      |
| MG-11 | No sensitive-tool elevation flow                           | High     | ☆ `trogon-governance`                    |
| MG-12 | No per-session concurrency cap                             | Medium   | `trogon-mcp-gateway`                     |
| MG-13 | No OAuth2 / mTLS / API-key auth modes for downstream MCP   | High     | `trogon-mcp-gateway`                     |

## 4. Execution Hypervisor

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| EX-01 | No ring model (R0–R3)                                      | Critical | ☆ `trogon-governance`                    |
| EX-02 | Ring not represented as JWT claim                          | Critical | `a2a-auth-callout`                       |
| EX-03 | No action classification on calls                          | High     | `trogon-mcp-gateway`                     |
| EX-04 | No elevation flow with TTL ≤ 3600 s                        | Critical | ☆ `trogon-governance`                    |
| EX-05 | No saga orchestration / compensation                       | High     | `trogon-decider` (extend)                |
| EX-06 | No per-ring quotas                                         | High     | `trogon-mcp-gateway`                     |
| EX-07 | No quarantine subject contract                             | High     | ☆ `trogon-governance`                    |

## 5. Audit & Compliance

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| AU-01 | No hash-chain on audit events                              | Critical | `a2a-nats::audit`                        |
| AU-02 | No decision BOM payload                                    | Critical | `a2a-nats::audit`                        |
| AU-03 | No tamper-evident fields (args/result hash, signature)     | Critical | `a2a-nats::audit`                        |
| AU-04 | No pre/post verifiable receipts                            | Critical | `a2a-nats::audit`                        |
| AU-05 | Audit not partitioned per tenant                           | Critical | `a2a-nats::audit`                        |
| AU-06 | No retention / WORM enforcement                            | High     | `a2a-nats::audit` + ops                  |
| AU-07 | No external anchor publication                             | Medium   | ☆ `trogon-governance`                    |
| AU-08 | No audit search API                                        | High     | ☆ `governance-audit-search`              |
| AU-09 | No JCS canonicalisation for receipts                       | High     | `a2a-nats::audit`                        |

## 6. SRE Governance

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| SR-01 | No declared SLOs per agent / tool                          | High     | `trogon-telemetry` + policy              |
| SR-02 | No error budgets                                           | High     | `trogon-telemetry`                       |
| SR-03 | No circuit-breaker primitive in `trogon-std`               | High     | `trogon-std`                             |
| SR-04 | No chaos / golden-trace harness                            | Medium   | ☆ `governance-chaos`                     |
| SR-05 | Build pipeline does not sign policy / catalog / sentinel artifacts | Critical | ops / build                              |

## 7. Wire / E2E

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| WI-01 | No optional X3DH / Double Ratchet payload encryption       | Medium   | ☆ `trogon-wire-e2e`                      |
| WI-02 | Policy cannot mark a route as E2E (gateway-opaque)         | Medium   | `trogon-policy`                          |

## 8. Threat Model / Tenancy / Data

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| TM-01 | No published STRIDE threat model per crate                 | High     | docs                                     |
| TN-01 | No tenant-scoped audit stream contract                     | Critical | `a2a-nats::audit`                        |
| TN-02 | No tenant-scoped operator roles                            | High     | `a2a-auth-callout`                       |
| TN-03 | No tenant kill switch                                      | High     | ☆ `trogon-governance`                    |
| DP-01 | No data residency tagging on accounts / routes             | High     | `a2a-auth-callout` + policy              |
| DP-02 | No egress allowlist (DNS/CIDR) for outbound HTTP from agents | High   | ☆ `trogon-egress-proxy`                  |

## 9. Standards & Compliance Crosswalks

| ID    | Gap                                                        | Sev      | Owner crate                              |
| ----- | ---------------------------------------------------------- | -------- | ---------------------------------------- |
| CS-01 | No OWASP Agentic Top 10 crosswalk                          | High     | docs                                     |
| CS-02 | No NIST AI RMF crosswalk                                   | High     | docs                                     |
| CS-03 | No EU AI Act high-risk mapping                             | High     | docs                                     |
| CS-04 | No SOC 2 CC crosswalk                                      | High     | docs                                     |
| CS-05 | No ISO 42001 crosswalk                                     | Medium   | docs                                     |

## Severity Roll-Up

- **Critical**: 22 gaps. These block any enterprise pilot.
- **High**: 28 gaps. These are needed for the first paying customer.
- **Medium**: 9 gaps. These are programme-level investments.

Total: **59 distinct gaps** to drive through `06-todo-backlog.md`.
