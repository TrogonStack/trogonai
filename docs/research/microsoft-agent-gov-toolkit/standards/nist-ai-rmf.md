# NIST AI RMF 1.0 + GenAI Profile (AI 600-1) — Crosswalk

> Sources: *NIST AI RMF 1.0* (NIST AI 100-1, January 2023) and the *Generative AI Profile* (NIST AI 600-1, July 2024). Voluntary framework. Rows reference the four core functions — **GOVERN, MAP, MEASURE, MANAGE** — and key subcategories. Subcategory codes use the published format (e.g. `GOVERN 1.1`).

## GOVERN

| Subcategory          | Outcome                                                                 | TrogonStack control                                                                                  | Owner                                  | Status  | Gap IDs            |
| -------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | -------------------------------------- | ------- | ------------------ |
| GOVERN 1.1           | Policies / processes for trustworthy AI                                 | Signed policy bundles, folder hierarchy with deny immutability, conflict-resolution metadata.        | `trogon-policy`                        | partial | PO-02, PO-03, PO-05 |
| GOVERN 1.4           | Risk management aligned to org strategy                                 | Per-bundle SLOs + error-budget burn freezes risky rollouts.                                          | `trogon-telemetry`                     | gap     | SR-01, SR-02       |
| GOVERN 2.1           | Roles and responsibilities documented                                   | Tenant-scoped operator roles in auth-callout (account-admin vs cross-tenant-auditor).                 | `a2a-auth-callout`                     | gap     | TN-02              |
| GOVERN 4.1           | Diverse stakeholder engagement (incl. impacted communities)             | Out of scope at substrate; surfaced via approval workflow + audit search.                            | `trogon-governance`                    | partial | PO-07, AU-08       |
| GOVERN 5.1           | Mechanisms for AI actor accountability                                  | Sponsor binding on agent mint, approver DID recorded on every privileged decision.                   | `a2a-auth-callout`, `a2a-nats::audit`  | partial | ID-02, AU-03       |
| GOVERN 6.1           | Third-party / supply-chain policies                                     | Signed MCP catalog, drift detection across 8 categories, CVE auto-quarantine.                        | `mcp-drift`, `mcp-cve-watcher`         | gap     | MG-07, MG-08, MG-10 |

## MAP

| Subcategory          | Outcome                                                                 | TrogonStack control                                                                                  | Owner                                  | Status  | Gap IDs            |
| -------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | -------------------------------------- | ------- | ------------------ |
| MAP 1.1              | Context of use established                                              | Data-residency claim on account JWT, tenant policy bundle states allowed regions.                    | `a2a-auth-callout`, `trogon-policy`    | gap     | DP-01              |
| MAP 2.3              | AI capabilities and limits documented                                   | Signed catalog records each tool's stable description / schema / scope.                              | `trogon-mcp-gateway::catalog`          | gap     | MG-07              |
| MAP 3.1              | Benefits / costs / impacts characterised                                | Decision BOM gives per-call attribution to policy bundles + trust snapshot.                          | `a2a-nats::audit`                      | gap     | AU-02              |
| MAP 5.1              | Risks identified incl. third-party                                      | Drift + 6 supply-chain scanners (poisoning, rug-pull, cross-server, confused-deputy, hidden-instr, description-injection). | `mcp-drift`                            | gap     | MG-08, MG-09       |

## MEASURE

| Subcategory          | Outcome                                                                 | TrogonStack control                                                                                  | Owner                                  | Status  | Gap IDs            |
| -------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | -------------------------------------- | ------- | ------------------ |
| MEASURE 1.1          | Approaches / metrics for risk identified                                | SLOs + error budgets + golden-trace diff + KL regime detector.                                       | `trogon-telemetry`, `trogon-trust`     | gap     | SR-01, SR-02, SR-04, ID-06 |
| MEASURE 2.7          | Security and resilience                                                 | Circuit breakers, rate limits, chaos harness, signed artifact pipeline.                              | `trogon-std`, `trogon-mcp-gateway`     | gap     | SR-03, SR-04, SR-05 |
| MEASURE 2.8          | Privacy                                                                 | PII / credential / exfil scanners on gateway response stage.                                         | `a2a-redaction`                        | gap     | MG-03              |
| MEASURE 2.10         | Explainability / interpretability of decisions                          | Decision BOM lists which rules fired, with policy version + signature.                               | `a2a-nats::audit`                      | gap     | AU-02              |
| MEASURE 3.2          | Monitoring for change in risk profile                                   | Trust score + KL divergence + drift events on `governance.drift.*`.                                  | `trogon-trust`, `mcp-drift`            | gap     | ID-06, MG-08       |

## MANAGE

| Subcategory          | Outcome                                                                 | TrogonStack control                                                                                  | Owner                                  | Status  | Gap IDs            |
| -------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | -------------------------------------- | ------- | ------------------ |
| MANAGE 1.3           | Resource allocation reflects risk priorities                            | Ring-based quotas + per-tool rate limits + per-server circuit breakers.                              | `trogon-governance`, `trogon-mcp-gateway` | gap   | EX-06, MG-04, MG-06 |
| MANAGE 2.3           | Procedures to respond to / recover from issues                          | Kill switch, quarantine state, saga compensation, CVE auto-quarantine.                               | `trogon-governance`, `trogon-decider`  | gap     | ID-12, ID-13, EX-05, MG-10 |
| MANAGE 2.4           | Mechanisms for AI users to challenge / appeal                           | Audit search + approval workflow record approver DID.                                                | `trogon-governance`                    | gap     | PO-07, AU-08       |
| MANAGE 4.1           | Post-deployment monitoring                                              | OTel + golden traces + trust telemetry + drift dashboards.                                           | `trogon-telemetry`, `mcp-drift`        | gap     | I3, I4, SR-04      |

## GenAI Profile (AI 600-1) Highlights

| Risk                                       | TrogonStack control                                                                                          | Owner                                | Status  |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------ | ------------------------------------ | ------- |
| Confabulation                              | Response scanners flag hallucinated tool calls, instruction tags; decision BOM ties answer to inputs.        | `a2a-redaction`, `a2a-nats::audit`   | gap     |
| Data privacy / leakage                     | PII / credential scanners on response; egress allowlist (DNS + CIDR).                                        | `a2a-redaction`, `trogon-egress-proxy` | gap   |
| Harmful bias                               | Out of scope at substrate; surfaced via approval workflow + audit search for downstream review.              | `trogon-governance`                  | partial |
| Information security                       | NATS account isolation, per-tenant audit, signed artifacts, hash-chained ledger, E2E wire option.            | substrate-wide                       | partial |
| Information integrity                      | Signed bundles + catalogs + receipts + JCS canonicalisation.                                                 | substrate-wide                       | gap     |
| Intellectual property                      | Egress allowlist + data residency tag + tenant audit isolation.                                              | `trogon-egress-proxy`, `a2a-auth-callout` | gap |
| Obscene / CSAM / NCII                      | Out of scope at substrate; redaction WASM bundle is the customer's pluggable boundary.                       | `a2a-redaction`                      | partial |
| Value chain / component integration        | MCP supply-chain scanners + signed catalog + CVE feed + drift detection.                                     | `mcp-drift`, `mcp-cve-watcher`       | gap     |
