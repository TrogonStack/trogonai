# OWASP Agentic Security — Crosswalk

> Source: OWASP GenAI Security Project — *Agentic AI Threats and Mitigations* (the threat catalogue AGT abbreviates as ASI-01..10). The list has been extended in newer revisions of the OWASP whitepaper; we cover the first ten here and treat the rest as follow-up tasks. Treat this crosswalk as a snapshot — re-check against the latest OWASP publication before any compliance attestation.

Each row identifies the threat, the TrogonStack control(s) that mitigate it, the owning crate, and gap IDs (from `../05-gap-analysis.md`) that must close before the row is fully `met`.

| ID      | Threat                              | Primary TrogonStack control                                                                                                | Owner crate(s)                              | Status   | Gap IDs that must close |
| ------- | ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- | -------- | ----------------------- |
| ASI-01  | Memory poisoning                    | Signed catalogs + drift detection; agent memory persisted to JetStream KV per-tenant; hash-chained audit catches injection. | `mcp-drift`, `a2a-nats::audit`              | partial  | MG-07, MG-08, AU-01     |
| ASI-02  | Tool misuse                         | Two-stage gateway (deny→allow→sensitive→rate-limit→schema→breaker→policy); response scanners detect post-hoc misuse.       | `trogon-mcp-gateway`, `a2a-redaction`       | partial  | MG-01, MG-04, MG-05, MG-11 |
| ASI-03  | Privilege compromise                | Ring model R0–R3; elevation TTL ≤ 3600 s with sponsor signature; delegation monotonic narrowing.                            | `trogon-governance`, `a2a-auth-callout`     | gap      | EX-01, EX-02, EX-04, ID-09 |
| ASI-04  | Resource overload                   | Per-tool sliding-window rate limit; per-ring quotas; circuit breakers per downstream MCP server.                            | `trogon-mcp-gateway`, `trogon-std`          | gap      | MG-04, MG-06, EX-06, SR-03 |
| ASI-05  | Cascading hallucination attacks     | Response scanners (hidden-instruction, instruction-tag, description-injection); policy can mark route E2E-opaque + scan-skip explicit. | `a2a-redaction::tier3_sentinel`             | partial  | MG-01, MG-02, MG-09     |
| ASI-06  | Intent breaking / goal manipulation | Trust score + KL regime change detector; auto-quarantine on alarm; sponsor binding + decision BOM for forensic replay.      | `trogon-trust`, `trogon-governance`         | gap      | ID-03, ID-06, ID-13, PO-08 |
| ASI-07  | Misaligned / deceptive behaviour    | Reward dimensions include `security_posture` + `policy_compliance`; golden-trace diff flags behaviour drift.               | `trogon-trust`, `governance-chaos`          | gap      | ID-04, SR-04            |
| ASI-08  | Repudiation / untraceability        | Hash-chained per-tenant audit; pre/post Ed25519 receipts; decision BOM with policy versions + bundle signature.            | `a2a-nats::audit`                           | gap      | AU-01, AU-02, AU-03, AU-04, AU-05 |
| ASI-09  | Identity spoofing / impersonation   | NATS user JWTs signed by account, minted by auth-callout with sponsor binding; IATP challenge-response for peer auth.       | `a2a-auth-callout`, `trogon-iatp`           | partial  | ID-01, ID-02, ID-08, ID-09 |
| ASI-10  | Overwhelming human-in-the-loop      | Sensitive-tool elevation flow surfaces approvals; approval workflow service rate-limits requests; trust-tier-aware policy can suppress noise. | `trogon-governance`                         | gap      | MG-11, PO-07            |

## Notes On Coverage

- **Memory poisoning (ASI-01)** is only partially in scope because TrogonStack does not own agent memory — agents do. Our mitigation is at the seams (signed inputs, hash-chained audit, drift detection on the tools that feed memory). Customers should layer their own memory governance.
- **Cascading hallucination (ASI-05)** is materially reduced by stripping prompt-injection vectors at the gateway response stage; it cannot be eliminated. The honest claim is "we prevent injection-driven cascades; we cannot prevent native model errors".
- **HITL fatigue (ASI-10)** is paradoxically a trust + policy problem, not just a UX problem. Trust-tier-aware policy is the lever that prevents low-stakes calls from being escalated.

## Beyond The Top 10

OWASP's whitepaper lists additional agentic threats (RCE / code attacks, agent communication poisoning, rogue agents in multi-agent systems, human-as-vector attacks, human manipulation). These are tracked as Phase 11 follow-ups in `06-todo-backlog.md` — covered jointly by the IATP handshake (ID-08), signed wire payloads (WI-01), and the egress proxy (DP-02).
