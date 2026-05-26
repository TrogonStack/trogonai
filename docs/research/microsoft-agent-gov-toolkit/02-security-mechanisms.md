# AGT Security Mechanisms — Normative Catalogue

This is a compact, evidence-grade restatement of every security primitive AGT defines, organised so it can be re-implemented under NATS without re-reading the source specs.

## 1. Policy Decisions

**Document shape** — one policy is one document with `metadata`, `match`, `conditions`, `actions`. Folder paths form a hierarchy; child folders may restrict but never relax a parent `deny` (the *deny immutability* invariant).

**Operators** (closed set): `eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `in`, `contains`, `matches` (regex). No arithmetic, no I/O — evaluator must be total and pure.

**Actions** (closed set): `allow` (terminal), `deny` (terminal), `audit` (continue, emit record), `block` (terminal, structured error).

**Conflict resolution** — choose one of `DENY_OVERRIDES` (default), `ALLOW_OVERRIDES`, `PRIORITY_FIRST_MATCH`, `MOST_SPECIFIC_WINS`. The choice is part of policy metadata, not engine config — so reviewers can read a policy and predict its outcome.

**Fail-closed** — any PDP error MUST be treated as `deny`. Timeout, malformed input, missing tenant context, signature failure — all deny.

**Pluggable backends** — OPA/Rego and Cedar are first-class. The engine wraps backend decisions back into AGT's action vocabulary so callers don't see backend differences.

**Versioning** — every decision records `policy_version`. Canary rollout is a deployment concern, but the receipt MUST identify which version decided.

## 2. Identity & Trust

**Cryptographic root** — Ed25519 keypairs. Public key fingerprint becomes the DID (`did:mesh:<fingerprint>` or `did:agentmesh:<…>`).

**Sponsor binding** — every agent creation event is signed by a sponsor (parent agent or operator). The signature plus sponsor DID is part of the agent's birth certificate and is replayable from audit.

**Credential lifecycle** — short-lived tokens. AGT spec uses 15-minute TTL by default. Rotation is mandatory and automated.

**Trust score** — integer 0–1000 partitioned into tiers:

| Tier               | Range    | Implication                                        |
| ------------------ | -------- | -------------------------------------------------- |
| `verified_partner` | 900–1000 | May execute privileged sagas, no second approval   |
| `trusted`          | 700–899  | Default ring R1, low oversight                     |
| `standard`         | 500–699  | Default ring R2, sensitive ops elevation required  |
| `probationary`     | 300–499  | Default ring R3, all writes audited synchronously  |
| `untrusted`        | 0–299    | Read-only or denied                                |

**Reward dimensions** — score is a weighted sum (defaults): `policy_compliance` 0.25, `resource_efficiency` 0.15, `output_quality` 0.20, `security_posture` 0.25, `collaboration_health` 0.15. Operators may rebalance weights but MUST publish them with the trust policy.

**Decay** — score decays toward 500 (neutral) when no signal is observed; prevents stale "trusted" tokens.

**Regime change detection** — KL divergence between recent reward distribution and historical distribution; alert when threshold crossed. Used to flag compromise or capability drift.

**Network propagation** — when agent A signs work by agent B, a fraction of B's trust delta propagates to A's reputation graph. Decays per hop.

**IATP handshake** — challenge-response between two agents. Each side signs a nonce containing the other's DID and capability scope. Successful handshake mints a session credential bound to both.

**Delegation chains** — when A delegates capability X to B, the delegated credential MUST carry a strictly narrower scope, a trust ceiling ≤ A's tier, and a TTL ≤ A's session. Monotonic narrowing is a hard invariant.

**SPIFFE / SVID** — AGT supports interop. A SPIFFE-issued SVID can stand in for a DID credential at trust boundaries.

## 3. MCP Gateway

**Dual-stage pipeline** — every tool call is intercepted twice: pre-execution (decide) and post-execution (response scan). Both stages emit audit records and may transition the call to `block`.

**Pre-execution ladder** (terminate on first match):

1. Deny list (tool name or fully-qualified ID).
2. Allow list (if non-empty, callee must be present).
3. Sensitive-tool check → forces elevation request.
4. Sliding-window rate limit (default 100 calls / 300 s per agent / per tool).
5. Schema validation (input ≤ 1 MB; required fields; type check).
6. Circuit breaker (if downstream MCP server unhealthy, fail fast).
7. Policy engine (§1).

**Post-execution scanners** (any match → block + audit):

- `instruction_tags` — `<system>`, `<instructions>`, `[INST]`, `<<SYS>>` etc. embedded in tool output.
- `credential_leak` — heuristics for AWS keys, GCP keys, GitHub PATs, OAuth tokens.
- `pii` — pluggable detector; emits category, not value.
- `exfil_urls` — URLs in output not on caller's egress allowlist.
- `hidden_instructions` — zero-width chars, Unicode-bidi reorder, base64-decoded payloads matching prompt-injection signatures.
- `description_injection` — checks that tool description shown to LLM matches signed catalog description.

**Security scanners** (catalog-time, not call-time):

- `TOOL_POISONING` — description changed in a way that biases model toward unsafe action.
- `RUG_PULL` — tool name/description stable but underlying schema or behaviour changed.
- `CROSS_SERVER_ATTACK` — one MCP server's tool description references another server's tools (orchestration via prompt).
- `CONFUSED_DEPUTY` — tool accepts caller-supplied principals it does not authenticate.
- `HIDDEN_INSTRUCTION` — catalog-side variant of the runtime scanner.
- `DESCRIPTION_INJECTION` — text in description trying to alter LLM behaviour.

**HMAC signing** — gateway↔server traffic HMAC-SHA256 signed with rotating shared secret; nonce-based anti-replay (24 h window).

**Session model** — 1-hour TTL, max 10 concurrent sessions per agent, refresh requires re-authentication if trust score dropped a tier.

**Auth modes** — `oauth2`, `mtls`, `api_key`, `bearer`, `none`. `none` MUST require explicit operator opt-in per server.

**CVE feed** — gateway subscribes to OSV API for MCP-server CVEs; affected servers auto-quarantined.

**Drift categories** (catalog snapshots compared between versions): name, description, input_schema, output_schema, auth, scope, transport, owner. Any drift triggers re-approval workflow.

## 4. Execution Hypervisor

**Rings** — R0 (operator-only), R1 (trusted), R2 (standard), R3 (probationary). Each ring carries: max concurrent actions, action classes allowed, side-effect quota, audit synchronicity.

**Action classification** — `read`, `write`, `external_call`, `privileged` (e.g. key rotation, policy edit). Rings declare which classes they may emit.

**Elevation** — agent requests temporary ring promotion. Default TTL 300 s, max 3600 s. Requires sponsor approval signature; auto-revoked on any policy violation.

**Saga orchestration** — multi-step actions are compensable. Each step is atomic and emits a compensation handle. Failure of step N runs `compensate(N-1), compensate(N-2), …`. Compensation is best-effort; non-compensable steps MUST be flagged as such in the saga manifest.

**Kill switch** — single signed operator command revokes all active credentials for an agent or tenant; in-flight calls return synthetic `block`.

**Quarantine** — soft state. Agent retains credentials but every action is forced to R3 and synchronously audited. Triggered by KL-divergence alert, drift detection, or operator command.

## 5. Audit & Compliance

**Hash chain** — every audit record contains the SHA-256 of the previous record. Chain is per-tenant and per-stream. Periodic root signed by operator and published externally for anchoring.

**Decision BOM** — for each policy decision, audit captures: policy IDs evaluated, policy versions, signature digests, backend used, conflict-resolution mode, every rule that fired, the resolved action, and timing.

**Tamper-evident fields** (required on every action audit record): `arguments_hash`, `result_hash`, `approver_did`, `policy_version`, `issued_at`, `started_at`, `completed_at`, `previous_hash`, `signature`.

**Verifiable receipts** — pre-execution receipt signed by gateway, post-execution receipt signed again with result hash. Canonicalised via RFC 8785 JCS so signature is reproducible.

**Per-tenant isolation** — audit streams MUST be separable per tenant, no cross-tenant joins without explicit operator action.

**Retention / WORM** — append-only with operator-defined retention. Deletion before retention expiry requires a signed override and emits a meta-audit record.

## 6. SRE Governance

**SLOs** — declared per agent and per critical tool: availability, p95 latency, error rate.

**Error budgets** — drive deployment gating. Budget burn alerts and freezes risky rollouts.

**Circuit breakers** — per dependency, half-open probes after cooldown.

**Chaos hooks** — periodic injected failures (latency, error, timeout) against synthetic agents; verifies governance still emits correct denials and audits.

**Golden traces** — known-good OTel traces stored per critical workflow. Live traces compared structurally; deviation surfaces in regression dashboards.

**Artifact signing** — policy bundles, tool catalogs, drift baselines, golden traces — all signed; gateway refuses unsigned bundles.

## 7. Wire / E2E

**X3DH** — three-party initial key agreement (identity / signed pre-key / one-time pre-key) yielding shared secret for an asynchronous first message.

**Double Ratchet** — symmetric ratchet on every message; new DH pair per round-trip provides forward + future secrecy.

**ChaCha20-Poly1305** — payload AEAD. Header carries DH ratchet pub key, message counter, prev-chain length.

Applies between two agents who want true confidentiality even from the gateway. Not the default — opt-in for sensitive workflows.

## 8. Threat Model Highlights (STRIDE)

| Threat                       | AGT control                                           |
| ---------------------------- | ----------------------------------------------------- |
| Spoofing                     | DIDs + Ed25519 + IATP handshake                       |
| Tampering                    | Hash chain audit, signed catalogs/policies            |
| Repudiation                  | Verifiable receipts, sponsor binding                  |
| Information disclosure       | Response scanners, E2E wire, per-tenant audit         |
| Denial of service            | Rate limits, circuit breakers, kill switch            |
| Elevation of privilege       | Rings, elevation TTL, monotonic delegation narrowing  |
| Prompt injection             | Hidden-instruction + description-injection detectors  |
| Tool supply chain compromise | Drift detection, CVE feed, catalog signing            |

## 9. Tenant Isolation

K8s namespace + dedicated trust store + dedicated audit stream + data-residency tags + egress allowlists (DNS + CIDR). AGT treats tenant boundary as a security boundary, not just a logical one.
