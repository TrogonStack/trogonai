# EU AI Act — Crosswalk

> Source: Regulation (EU) 2024/1689 (the EU AI Act). Scope here is the obligations most likely to apply when a TrogonStack-hosted system is classified as **high-risk** under Article 6, or when a provider is offering a **General-Purpose AI (GPAI) model** under Chapter V. Always validate classification with counsel before claiming compliance.

## High-Risk System Obligations (Chapter III, Section 2)

| Article | Obligation                                              | TrogonStack control                                                                                                       | Owner                                          | Status  | Gap IDs            |
| ------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------- | ------- | ------------------ |
| Art. 9  | Risk management system across lifecycle                  | Per-bundle SLOs + error-budget gating + KL regime detector + signed policy versioning.                                    | `trogon-policy`, `trogon-telemetry`, `trogon-trust` | partial | PO-05, SR-01, SR-02, ID-06 |
| Art. 10 | Data and data governance                                 | Out of scope at substrate; data flows are policed by egress + residency + redaction at agent boundary.                    | `trogon-egress-proxy`, `a2a-redaction`         | partial | DP-01, DP-02, MG-03 |
| Art. 11 | Technical documentation                                  | Signed policy bundles + signed catalogs + threat model + standards crosswalks (this doc set).                              | docs + build pipeline                          | partial | SR-05              |
| Art. 12 | Automatic recording of events (logs)                     | Hash-chained per-tenant audit stream; pre/post Ed25519 receipts; retention / WORM controls; external anchoring.            | `a2a-nats::audit`                              | gap     | AU-01, AU-04, AU-05, AU-06, AU-07 |
| Art. 13 | Transparency to deployers (instructions + capabilities)  | Signed catalog records each tool's schema, scope, owner, transport — published artifact.                                  | `trogon-mcp-gateway::catalog`                  | gap     | MG-07              |
| Art. 14 | Human oversight                                          | Sensitive-tool elevation flow; approval workflow with recorded approver DID; kill switch + quarantine.                    | `trogon-governance`                            | gap     | MG-11, PO-07, ID-12, ID-13 |
| Art. 15 | Accuracy, robustness, cybersecurity                      | Circuit breakers, rate limits, schema validation, response scanners, signed artifact pipeline, chaos harness.             | `trogon-mcp-gateway`, `a2a-redaction`, `governance-chaos` | gap | MG-01..06, SR-03, SR-04, SR-05 |
| Art. 16 | Provider obligations (incl. QMS by Art. 17)              | Build pipeline signs every artifact; release key rotation; out-of-band root publication.                                  | build pipeline                                 | gap     | SR-05, AU-07       |
| Art. 17 | Quality management system                                | Documented in `06-todo-backlog.md` phase plan + standards crosswalks; substrate provides traceability hooks only.          | process                                        | partial | —                  |
| Art. 26 | Deployer obligations (use as instructed, human oversight, log retention) | Audit retention controls + signed receipts give the deployer the evidence pack they need.                | `a2a-nats::audit`                              | gap     | AU-04, AU-06       |

## GPAI Obligations (Chapter V)

These apply when TrogonStack is hosting or fronting a general-purpose model. Most are upstream provider obligations, not substrate obligations — but the substrate must not impede evidence collection.

| Article | Obligation                                                                  | TrogonStack control                                                                                                   | Owner                                  | Status  |
| ------- | --------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | ------- |
| Art. 53 | Technical documentation + info to downstream providers                       | Signed catalog + signed policy bundle + standards crosswalks.                                                         | docs + build pipeline                  | partial |
| Art. 54 | Copyright compliance policy                                                  | Out of scope at substrate; surfaced via per-tenant redaction bundle + egress allowlist.                                | `a2a-redaction`, `trogon-egress-proxy` | partial |
| Art. 55 | Systemic-risk obligations (model eval, incident reporting, cybersecurity)    | Trust + KL regime detector + drift + CVE feed + per-tenant audit + signed receipts give the evidence pack.            | substrate-wide                         | gap     |

## Fundamental Rights Impact Assessment (FRIA, Art. 27)

Out of scope at the substrate level — a deployer obligation. TrogonStack supports it by surfacing the artefacts FRIA requires: data residency, audit retention, human-in-the-loop records, approval chain.

## Practical Implications For The Backlog

The audit hardening cluster (AU-01..AU-09) is the single most leveraged area for the AI Act: it dominates Articles 12, 14 (oversight evidence), 15 (incident forensics), 26 (deployer logs), and 55 (incident reporting). Closing it unlocks claims under five separate obligations.
