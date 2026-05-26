# SOC 2 Trust Services Criteria — Crosswalk

> Source: AICPA *Trust Services Criteria* (2017, revised). Covers the Common Criteria (CC1–CC9) plus the Availability (A), Confidentiality (C), Processing Integrity (PI), and Privacy (P) categories. Audit scope is set by the customer's report scope.

## Common Criteria

### CC1 — Control Environment

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC1.1     | Documented standards crosswalks + threat model + Diátaxis-organised docs.           | docs                                  | partial | —       |
| CC1.4     | Tenant-scoped operator roles in auth-callout; sponsor binding on agent creation.    | `a2a-auth-callout`                    | gap     | TN-02, ID-02 |

### CC2 — Communication and Information

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC2.1     | Decision BOM + hash-chained audit + audit search API → internal communication of control results. | `a2a-nats::audit`                     | gap     | AU-01, AU-02, AU-08 |
| CC2.2     | Signed policy bundles published to tenant operators; canary rollout signals.       | `trogon-policy`                       | gap     | PO-05, PO-06 |

### CC3 — Risk Assessment

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC3.1     | STRIDE threat model per crate; documented in `docs/security/threat-model.md`.       | docs                                  | partial | TM-01   |
| CC3.2     | KL regime detector + drift + CVE feed surface changes to risk profile.              | `trogon-trust`, `mcp-drift`, `mcp-cve-watcher` | gap | ID-06, MG-08, MG-10 |

### CC4 — Monitoring Activities

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC4.1     | OTel + golden-trace diff + audit search + drift dashboards.                         | `trogon-telemetry`, `mcp-drift`       | gap     | SR-04, AU-08 |
| CC4.2     | Chaos harness verifies that governance still emits denials/audits under fault.      | `governance-chaos`                    | gap     | SR-04   |

### CC5 — Control Activities

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC5.1     | Two-stage gateway (deny→allow→sensitive→rate-limit→schema→breaker→policy).         | `trogon-mcp-gateway`                  | partial | MG-01..06 |
| CC5.2     | Build pipeline signs every artifact; loaders refuse unsigned.                       | build pipeline                        | gap     | SR-05   |
| CC5.3     | Per-bundle conflict resolution metadata; deny immutability enforced by hierarchy.   | `trogon-policy`                       | gap     | PO-02, PO-03 |

### CC6 — Logical and Physical Access

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC6.1     | NATS account isolation per tenant; short-lived user JWTs minted with sponsor binding. | `a2a-auth-callout`                  | partial | ID-02, ID-11 |
| CC6.2     | Ring R0–R3 model + elevation flow with TTL ≤ 3600 s + sponsor signature required.   | `trogon-governance`                   | gap     | EX-01, EX-04 |
| CC6.3     | Delegation chain monotonic narrowing (scope ⊆ parent, TTL ≤ parent, trust ≤ parent). | `a2a-auth-callout`                   | gap     | ID-09   |
| CC6.6     | Egress allowlist (DNS + CIDR) + signed egress proxy.                                | `trogon-egress-proxy`                 | gap     | DP-02   |
| CC6.7     | Per-tenant audit isolation; redaction bundle per tenant.                            | `a2a-nats::audit`, `a2a-redaction`    | gap     | AU-05, TN-01 |
| CC6.8     | Signed catalog + drift detection prevents unauthorised tool change.                 | `mcp-drift`                           | gap     | MG-07, MG-08 |

### CC7 — System Operations

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC7.1     | Drift detection + CVE feed + golden-trace diff → continuous monitoring.             | `mcp-drift`, `mcp-cve-watcher`, `governance-chaos` | gap | MG-08, MG-10, SR-04 |
| CC7.2     | Hash-chained audit + per-tenant streams → anomaly detection inputs.                 | `a2a-nats::audit`                     | gap     | AU-01, AU-05 |
| CC7.3     | Incident response: kill switch + quarantine + CVE auto-quarantine + saga compensation. | `trogon-governance`, `trogon-decider`, `mcp-cve-watcher` | gap | ID-12, ID-13, EX-05, MG-10 |
| CC7.4     | External anchor publication of audit root; chaos harness for resilience evidence.   | `trogon-governance`, `governance-chaos` | gap   | AU-07, SR-04 |
| CC7.5     | Recovery: per-server circuit breaker + saga compensation + JetStream replay.        | `trogon-mcp-gateway`, `trogon-decider` | gap    | MG-06, EX-05 |

### CC8 — Change Management

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC8.1     | Signed policy bundles + canary rollout + approval workflow before activation.       | `trogon-policy`, `trogon-governance`  | gap     | PO-05, PO-06, PO-07 |

### CC9 — Risk Mitigation

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| CC9.1     | Per-tool rate limits + per-ring quotas + circuit breakers.                          | `trogon-mcp-gateway`, `trogon-std`    | gap     | MG-04, MG-06, EX-06, SR-03 |
| CC9.2     | Vendor / supply-chain: signed catalog + drift + CVE feed.                           | `mcp-drift`, `mcp-cve-watcher`        | gap     | MG-07, MG-08, MG-10 |

## Category — Availability (A1)

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| A1.1      | SLOs + error budgets declared in policy bundle.                                     | `trogon-policy`, `trogon-telemetry`   | gap     | SR-01, SR-02 |
| A1.2      | Circuit breakers + chaos harness + JetStream durability.                            | `trogon-std`, `governance-chaos`      | gap     | SR-03, SR-04 |
| A1.3      | Recovery: saga compensation + JetStream replay + kill-switch/quarantine isolation.  | `trogon-decider`, `trogon-governance` | gap     | EX-05, ID-12, ID-13 |

## Category — Confidentiality (C1)

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| C1.1      | NATS account isolation + per-tenant redaction bundle + optional X3DH/Double-Ratchet E2E for sensitive routes. | `a2a-redaction`, `trogon-wire-e2e` | partial | WI-01 |
| C1.2      | Audit search restricted to tenant scope; cross-tenant queries require explicit operator grant. | `a2a-auth-callout`, `governance-audit-search` | gap | TN-02, AU-08 |

## Category — Processing Integrity (PI1)

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| PI1.1     | Schema validation against signed catalog; 1 MB arg cap; tier-3 sentinel scrub.       | `trogon-mcp-gateway`, `a2a-redaction` | partial | MG-05, MG-07 |
| PI1.2     | Pre/post Ed25519 receipts canonicalised via RFC 8785 JCS.                            | `a2a-nats::audit`                     | gap     | AU-04, AU-09 |
| PI1.3     | Decision BOM ties every output to inputs, policy, trust snapshot.                    | `a2a-nats::audit`                     | gap     | AU-02   |
| PI1.4     | Result hash recorded; replay-safe.                                                   | `a2a-nats::audit`                     | gap     | AU-03   |
| PI1.5     | Hash-chained audit; tamper-evident.                                                  | `a2a-nats::audit`                     | gap     | AU-01   |

## Category — Privacy (P)

P-series controls are largely deployer obligations; the substrate provides hooks:

| Criterion | TrogonStack control                                                                | Owner                                 | Status  | Gap IDs |
| --------- | ---------------------------------------------------------------------------------- | ------------------------------------- | ------- | ------- |
| P1 / P2   | Notice / consent are deployer responsibilities; substrate records consent artefacts in audit. | `a2a-nats::audit`                     | partial | AU-02 |
| P3        | PII / credential / exfil scanners on gateway response stage.                        | `a2a-redaction`                       | gap     | MG-03   |
| P4        | Data residency claim + region-routed gateways.                                      | `a2a-auth-callout`, `trogon-policy`   | gap     | DP-01   |
| P5        | Retention / WORM on audit streams; signed override required to delete early.        | `a2a-nats::audit`                     | gap     | AU-06   |
| P6        | Disclosure of incidents → CVE auto-quarantine + signed receipts; deployer obligation. | `mcp-cve-watcher`, `a2a-nats::audit` | partial | MG-10, AU-04 |
| P7        | Quality of personal data — deployer obligation.                                     | —                                     | —       | —       |
| P8        | Monitoring + enforcement: audit search + drift dashboards.                          | `governance-audit-search`, `mcp-drift` | gap   | AU-08, MG-08 |
