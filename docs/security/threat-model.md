# TrogonStack — STRIDE Threat Model

This document is the STRIDE threat model for the security-critical crates and the trust boundaries between them. It is the authoritative artefact for gap **TM-01** in `docs/research/microsoft-agent-gov-toolkit/05-gap-analysis.md`.

Scope: every crate that holds a security guarantee — i.e. one the agent process cannot satisfy by itself. We deliberately omit crates that are pure data plumbing (e.g. transport adapters) unless they sit on a trust boundary.

STRIDE = Spoofing, Tampering, Repudiation, Information disclosure, Denial of service, Elevation of privilege.

## 0. Trust Boundaries

```
+--------------+   user JWT     +-----------------+   account JWT   +------------------+
|   external   | -------------> |  a2a-auth-      | --------------> |  NATS account    |
|  identity    |  SPIFFE/OIDC   |  callout        |   sponsor-bound |  (tenant)        |
+--------------+                +-----------------+                 +------------------+
                                                                            |
                                                                            | subject ACL
                                                                            v
+------------------+    request    +---------------------+   forward     +-----------+
|   agent process  | ------------> | trogon-mcp-gateway  | ------------> |  MCP srv  |
|   (UNTRUSTED)    |   on subject  | (mediator)          |               +-----------+
+------------------+               +---------------------+                     |
        ^                              |        ^                              |
        | response                     | decide |                              v
        +------------------------------+        |                       +------------+
                                                v                       | a2a-       |
                                          +----------+                  | redaction  |
                                          | trogon-  |                  | (scanner)  |
                                          | policy   |                  +------------+
                                          +----------+                         |
                                                |                              | scrubbed
                                                v                              v
                                          +-------------+                +-----------+
                                          | a2a-nats::  |<---------------|  reply    |
                                          | audit       |  receipt(post) +-----------+
                                          +-------------+
```

Trust boundaries (crossable only with authentication):
1. external identity ↔ `a2a-auth-callout` — must authenticate.
2. `a2a-auth-callout` ↔ NATS account — JWT minted and signed.
3. agent process ↔ `trogon-mcp-gateway` — account-bound NATS user JWT presented per request.
4. gateway ↔ downstream MCP server — server auth mode (oauth2 / mtls / api_key / bearer).
5. gateway response → caller (audited).
6. gateway / decider → audit stream (signed receipts).

The agent process is **on the untrusted side** of boundary 3. This is a deliberate inversion of AGT's "trusted middleware in-process" model.

## 1. `a2a-auth-callout`

**Role**: trust-boundary 1+2 enforcer. Mints account-bound NATS user JWTs given an upstream identity (operator JWT, SPIFFE SVID, OIDC token). Records sponsor binding.

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | Forged upstream identity (JWT replay, SVID injection).                                  | Validate signing key source; pin issuer; nonce; clock skew bound; reject untrusted JWKS.                    | ID-10, ID-11             |
| S      | Sponsor impersonation when minting child agent.                                         | Sponsor JWT signature recorded in mint, replayable from audit.                                              | ID-02                    |
| T      | Tampered minted JWT (claim injection — `ring`, `trust`, `scope`, `delegated_from`).      | Allowlist of mintable claims; reject unknown; sign with account NKEY only.                                  | ID-09, EX-02             |
| T      | Delegation chain widened (child scope > parent scope).                                  | Monotonic narrowing invariant: child scope ⊆ parent scope; TTL ≤ parent; trust ≤ parent.                    | ID-09                    |
| R      | Mint with no record.                                                                    | Every mint emits an audit record (sponsor DID, scope, TTL, ring).                                           | AU-02, AU-03             |
| I      | Sensitive claims leaked (e.g. residency, trust score) to wrong tenant.                  | Claims allowlist + per-tenant signing key isolation.                                                        | TN-02                    |
| D      | Mint flood from compromised upstream IdP.                                               | Rate-limit mints per upstream principal; alert on burst.                                                    | MG-04 (pattern reuse)    |
| E      | Operator-role escalation (account-admin gains cross-tenant-auditor).                    | Role allowlist enforced at mint; cross-tenant grant requires explicit operator-signed event.                | TN-02                    |
| E      | Kill-switch bypass (revoked agent re-mints).                                            | Mint path consults `governance.kill.*` and quarantine state before issuing JWT.                             | ID-12, ID-13             |

## 2. `trogon-mcp-gateway`

**Role**: trust-boundary 3+4+5 enforcer. Mediates every MCP call between agent and downstream server. Owns the two-stage pipeline.

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | Caller spoofs identity by stripping `caller_id` header.                                 | Identity derived from validated JWT, not from headers; `IdentitySource::LegacyHeader` deprecated path only. | (in `authz.rs` today)    |
| S      | Caller spoofs subject (publishes to another agent's reply subject).                     | Subject pattern enforced by NATS account ACL; reply subjects scoped per-session.                            | TN-01                    |
| T      | Tool argument tampering by intermediate node.                                           | `arguments_hash` recorded pre-execution; signed pre-receipt; gateway signs with Ed25519.                    | AU-03, AU-04             |
| T      | Tool catalog swap (rug-pull) between calls.                                             | Signed catalog + drift detection; refuse unsigned; recompute schema hash per call.                          | MG-07, MG-08             |
| T      | Response payload tampered after scan, before delivery.                                  | Gateway signs post-receipt over scanned payload hash; caller can verify.                                    | AU-04                    |
| R      | Decision made with no record (allow path silently shortcuts audit).                     | Every allow and deny emits to `audit.<tenant>`; circuit-breaker fast-fails also recorded.                   | AU-01, AU-02             |
| R      | Audit gap when gateway crashes mid-call.                                                | Pre-receipt emitted before forward; post-receipt or error-receipt always emitted; emitter holds hash tip.   | AU-01                    |
| I      | Tool response leaks credentials / PII / exfil URL.                                      | Response scanners (credential leak, PII, exfil URL, instruction tag, hidden instruction).                   | MG-01, MG-02, MG-03      |
| I      | Tool description leaks unsafe instructions to the LLM.                                  | Catalog-time scanners: TOOL_POISONING, DESCRIPTION_INJECTION, HIDDEN_INSTRUCTION.                           | MG-08, MG-09             |
| I      | Cross-server attack via tool descriptions referencing other servers.                    | CROSS_SERVER_ATTACK scanner; signed catalog isolates servers.                                               | MG-09                    |
| I      | Confused-deputy: tool accepts caller-supplied principal it does not authenticate.       | CONFUSED_DEPUTY scanner at catalog time; gateway never forwards `caller_principal` claims to MCP.            | MG-09                    |
| D      | Per-tool flood from a single agent.                                                     | Sliding-window rate limit per `(account, agent, tool)`; per-ring concurrency cap.                            | MG-04, EX-06             |
| D      | Downstream MCP server failure cascades.                                                 | Per-server circuit breaker; CVE auto-quarantine; chaos-tested.                                              | MG-06, MG-10, SR-03, SR-04 |
| D      | Oversize argument exhausts gateway memory.                                              | 1 MB arg cap; schema validation upfront.                                                                    | MG-05                    |
| E      | Sensitive tool invoked without elevation.                                               | Sensitive-tool list forces elevation token check; sponsor signature required.                                | MG-11, EX-04             |
| E      | Privilege escalation via tool chaining (R3 agent triggers privileged action via tool).  | Action classification on call; ring caps allowed action classes; policy checked against effective ring.    | EX-01, EX-03             |

## 3. `a2a-redaction` (incl. `tier3_sentinel`, `signed_bundle`, `wasm` runner)

**Role**: scan + scrub agent inputs and gateway responses. Holds the privacy and prompt-injection guarantee.

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | Forged redaction bundle.                                                                | Bundle signed with release key; loader refuses unsigned; signature recorded in audit.                       | SR-05                    |
| T      | Bundle tampered post-signing.                                                           | Signature verified at load; hash recorded; periodic re-verify.                                              | SR-05                    |
| T      | Tier-3 sentinel bypassed by Unicode reorder / zero-width chars.                          | Normalisation pass before scanning; explicit zero-width strip; bidi flag.                                   | MG-02                    |
| R      | Scrub silently drops content.                                                           | Scrub events emit redaction-category + original-hash to audit (never original value).                       | AU-02                    |
| I      | Bundle code itself leaks scanned content.                                               | WASM sandbox: no network, no FS, no host-time read; only `categorise(input) -> verdict` interface.          | (existing wasm bundle)   |
| I      | Sentinel exfiltrates via verdict channel.                                               | Verdict schema is a closed enum; reject extra fields.                                                       | MG-02                    |
| D      | Pathological input causes scanner to OOM / loop.                                        | WASM fuel cap + memory cap per call; timeout; fall back to deny on scanner timeout.                         | MG-05                    |
| E      | Sentinel mis-classifies privileged content as benign.                                   | Multi-scanner consensus on high-risk categories; chaos harness re-tests against golden bad inputs.          | SR-04                    |

## 4. `a2a-nats::audit`

**Role**: trust-boundary 6 — the ledger. Holds the non-repudiation guarantee.

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | Forged audit record published by compromised gateway pretending to be auditor.          | Emitter writes only to `audit.<tenant>` it has account JWT for; consumers can verify chain + signature.     | AU-01, AU-03             |
| T      | Mid-stream record rewrite.                                                              | Hash chain (`previous_hash`); root anchored externally; tamper detected on replay.                          | AU-01, AU-07             |
| T      | Record reorder to hide a deny.                                                          | Monotonic sequence in JetStream + chained hash; reorder breaks chain.                                       | AU-01                    |
| R      | Decision with no BOM → cannot reconstruct outcome.                                       | Decision BOM (policy versions, signatures, rules fired, trust snapshot) required on every governance event. | AU-02                    |
| R      | Pre-receipt without matching post-receipt.                                              | Emitter pairs pre/post on action ID; alarms on dangling pre-receipts.                                       | AU-04                    |
| I      | Cross-tenant audit join (auditor reads another tenant's logs).                          | One stream per account; account-scoped consumers; no operator-level join without signed grant.              | AU-05, TN-01, TN-02      |
| I      | Audit record leaks sensitive arguments verbatim.                                        | Record `arguments_hash`, not arguments; redacted preview only when policy allows.                           | AU-03                    |
| D      | Audit stream filled to denial-of-service the gateway (back-pressure).                   | Per-tenant rate-limit on emit; circuit-break audit-required actions with operator alert if overflow.        | AU-05                    |
| E      | Operator deletes audit before retention.                                                | Mid-retention delete requires signed override + meta-record on the same chain.                              | AU-06                    |

## 5. `trogon-decider` (and saga extension)

**Role**: command-to-event boundary; orchestrates multi-step privileged actions.

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | Saga step issued by a peer with insufficient ring.                                      | Saga orchestrator checks effective ring + elevation token on every step before dispatching.                  | EX-01, EX-04, EX-05      |
| T      | Compensation step skipped / reordered.                                                  | Compensators registered alongside step; replay-safe via JetStream stream; idempotency keys per step.        | EX-05                    |
| T      | Non-compensable step run as if compensable.                                             | Manifest flag `compensable: false`; orchestrator refuses to start a saga with non-compensable middle step unless explicitly approved. | EX-05    |
| R      | Step succeeded but no event recorded.                                                   | `evaluate_decision` always produces `(state, events)` tuple; events durably appended before ack.            | AU-01                    |
| I      | Event payload leaks privileged data to lower-ring downstream consumers.                 | Event subjects scoped by ring; redaction bundle runs at boundary.                                           | MG-03                    |
| D      | Replay storm exhausts decider on restart.                                               | JetStream durable consumer + checkpointing; back-off; circuit-break replay.                                 | SR-03                    |
| E      | Decider authority abused to issue privileged events bypassing policy.                   | Every decider event passes through gateway-style policy check before publication.                            | PO-01..PO-09             |

## 6. `trogon-policy` (planned crate; PO-01..09)

**Role**: PDP — the authority on whether an action is allowed.

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | Forged policy bundle activated.                                                         | Bundle signed by release key; loader refuses unsigned; per-tenant pinning of active version.                | PO-05, SR-05             |
| T      | Bundle tampered post-signing.                                                           | Signature verified at load; bundle hash recorded in every decision BOM.                                     | PO-05, AU-02             |
| T      | Hierarchy widened (child policy relaxes parent deny).                                   | Deny immutability invariant enforced at load: child deny ⊇ parent deny; explicit error on violation.        | PO-02                    |
| T      | Conflict-resolution mode swapped silently.                                              | Mode is bundle metadata; surfaced on every decision; canary diff highlights mode changes.                   | PO-03, PO-06             |
| R      | Decision made with no `policy_version`.                                                 | PDP refuses to return a decision without versioned bundle; gateway refuses such decisions.                  | PO-09                    |
| I      | Policy text leaks PII or business rules.                                                | Out of scope at substrate; bundle distribution is operator's responsibility.                                | —                        |
| D      | PDP slow / hung.                                                                        | Fail-closed timeout: deny on PDP timeout; per-decision latency budget; circuit-break.                        | (default deny)           |
| E      | Backend (OPA/Cedar) returns allow that policy hierarchy should deny.                    | Backend result re-checked against deny immutability before being returned by `Decide`.                       | PO-02, PO-04             |

## 7. `a2a-bridge`, `a2a-gateway`, `a2a-nats-server`

These three sit on trust-boundary 1 (external → bus) for the A2A path. Same STRIDE matrix as `trogon-mcp-gateway` applies, with two extra concerns:

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | External A2A peer impersonates trusted agent.                                           | IATP challenge-response handshake; signed pre-credential; SVID accepted.                                    | ID-08, ID-10             |
| I      | Bridge re-publishes external traffic onto internal subjects without sanitisation.       | Redaction bundle runs at ingress; subject rewrites recorded in audit (`AuditSubjectRewrite`).               | MG-03, AU-02             |

## 8. Operator Surface

The operator plane sits outside any single crate but it is the most privileged trust boundary. Its threats:

| STRIDE | Threat                                                                                  | Mitigation                                                                                                  | Gap IDs                  |
| ------ | --------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------ |
| S      | Operator key compromise.                                                                | HSM-backed signing where possible; quarterly rotation; rotation event signed by predecessor key.            | SR-05                    |
| T      | Operator override of policy without trace.                                              | Override emits a meta-audit record signed by operator key.                                                  | AU-06                    |
| R      | Operator action with no record.                                                          | All operator subjects (`governance.kill.*`, `governance.quarantine.*`, `audit.override`) are audited.        | AU-01, AU-02             |
| I      | Operator reads tenant audit it should not.                                              | Cross-tenant auditor role; explicit per-tenant grant; access events themselves audited.                     | TN-02                    |
| E      | Account-admin escalates to cross-tenant scope.                                          | Role allowlist; cross-tenant grant requires multi-operator signed event.                                    | TN-02                    |

## 9. Closed-By-Design Risks

Threats we deliberately accept (with justification):

- **E2E-encrypted routes are gateway-opaque.** Customer opts in per route. Documented in `02-security-mechanisms.md` §7 and `04-nats-native-translation.md` §7.
- **Agent process is hostile.** We provide no guarantee about the agent's internal state. All guarantees hold at the gateway boundary.
- **The bus operator can observe traffic metadata.** We do not provide metadata-private routing; we provide per-tenant subject isolation. Customers needing metadata privacy must run their own bus.

## 10. Re-Review Triggers

Re-run this threat model when any of the following lands:

- New crate crosses any of the six trust boundaries.
- A new claim is added to the auth-callout allowlist.
- A new audit field is added to `AuditEnvelopeFields`.
- A new policy backend is wired into `trogon-policy`.
- An E2E wire change (X3DH / Double Ratchet) is shipped.
- A new operator-level subject is introduced under `governance.*`.
