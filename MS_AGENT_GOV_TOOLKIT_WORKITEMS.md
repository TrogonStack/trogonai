# AGT Adoption Work Items for TrogonAi

> Tracker for everything worth copying from Microsoft's Agent Governance Toolkit (AGT, `tmp/agent-governance-toolkit`, MIT) into TrogonAi (Apache-2.0).
> Derived from `MS_AGENT_GOV_TOOLKIT.md` section 24 and the per-domain analyses in `.trogonai/analysis/agt-vs-trogonai/`.
> AGT is MIT-licensed: algorithms, rule corpora, spec documents, and scripts may be ported or vendored with attribution.

**Legend**: Value/Effort rated H/M/S (high/medium/small). Check items off as they land. Each item is intended to become one issue or proposal.

## Contents

- [A. MCP security gateway](#a-mcp-security-gateway)
- [B. Policy tooling (Tier-2 CEL)](#b-policy-tooling-tier-2-cel)
- [C. Supply chain and repo hygiene](#c-supply-chain-and-repo-hygiene)
- [D. Documentation genres](#d-documentation-genres)
- [E. Reliability, kill switch, audit](#e-reliability-kill-switch-audit)
- [Explicit non-goals](#explicit-non-goals)

---

## A. MCP security gateway

Our MCP path (`rsworkspace/crates/mcp-nats`, `mcp-nats-server`, `mcp-nats-stdio`) is a pure rmcp-over-NATS transport with zero tool scanning. This is AGT's most directly importable capability area.

- [x] **WI-01: Create an `mcp-gateway` crate mirroring `a2a-gateway`'s shape** (Value: H, Effort: L)
  - Landed: `crates/mcp-gateway` with `GatewayRuntime`, scan-at-discovery on `tools/list`, two-stage `tools/call` interception, verdict/audit/telemetry wiring, binary entrypoint. Follow-up: `WasmtimeSubstrate` reuse assessed as non-viable in current form; policy sandboxing integration remains open.
  - Why: today nothing inspects MCP traffic; `mcp-nats` deliberately stays policy-free the same way `a2a-nats` defers to `a2a-gateway`.
  - Shape: standalone NATS-addressed service subscribing to `mcp.server.{server_id}.>` (and optionally the client direction), intercepting `tools/list` responses (scan-at-discovery) and `tools/call` request/response pairs. Reuse the existing `WasmtimeSubstrate` (`crates/a2a-gateway/src/policy/wasmtime_substrate.rs`) rather than inventing new sandboxing.
  - AGT references: `MCPGateway` two-stage design (`intercept_tool_call` / `intercept_tool_response`) in `agent-governance-python/agent-os/src/agent_os/mcp_security.py`; spec `docs/specs/MCP-SECURITY-GATEWAY-1.0.md`.
  - Depends on: WI-02, WI-03, WI-04 supply the detection content.

- [x] **WI-02: Port typosquat and rug-pull detection as pure Rust functions** (Value: H, Effort: S)
  - What: typosquat check (Levenshtein distance 1-2, min tool-name length 4) and rug-pull fingerprinting (SHA-256 over tool description plus schema, versioned fingerprints) from AGT's `MCPSecurityScanner.scan_tool()` / `check_rug_pull`.
  - These are dependency-free pure functions; cheapest port in this whole file. Callable at `tools/list` response time before forwarding to the caller.

- [x] **WI-03: MCP tool schema drift detection** (Value: M, Effort: M)
  - What: fingerprint tool schemas deterministically and flag changes; borrow AGT's 8-value `DriftType` taxonomy (`SCHEMA_CHANGED`, `PARAMETER_REMOVED`, `TYPE_CHANGED`, ...).
  - Note: `a2a-pack` already does read-time AgentCard JSON Schema validation for A2A; there is no MCP equivalent over `mcp.server.{server_id}.tools.list`.

- [x] **WI-04: Hidden-instruction and description-injection scanning at discovery** (Value: H, Effort: M)
  - Conformance suite in `crates/mcp-gateway/tests/conformance.rs`: 7 MUSTs covered at library level plus MUST-18/19 at the service layer; remaining gateway-scope MUSTs enumerated there as open.
  - What: port the remaining two `MCPSecurityScanner` checks (hidden Unicode/instructions, description injection).
  - Acceptance: write a Rust conformance suite against the MUST checklist in `MCP-SECURITY-GATEWAY-1.0.md` (22 MUSTs, 127 tests upstream) without importing Python.

- [x] **WI-05: CVE feed gate for registered MCP servers** (Value: M, Effort: M)
  - What: OSV API lookup with 1-hour cache, fail-closed when unreachable, checked pre-dispatch; modeled on AGT's `McpCveFeed` (`agent_os/mcp_cve_feed.py`).

- [x] **WI-06: Vendor AGT's detection test corpora as fixtures** (Value: M, Effort: S)
  - 280/280 injection rows and 28/28 ASI scenarios vendored with measured baselines asserted (combined recall 0.27, FP 0.17). Fuzz harness documented in `fixtures/FUZZING.md` rather than adding a cargo-fuzz crate to the workspace.
  - What: `benchmarks/prompt-injection/` (280 labelled rows: 110 attack, 170 benign) and the OWASP-ASI-tagged red-team scenarios in `tests/redteam/test_asi.py` are data, not code. Use them to benchmark false-positive/recall rates of WI-02/WI-04 detectors. The `agent-governance-python/fuzz/fuzz_mcp_security.py` ClusterFuzzLite target is a template for a `cargo-fuzz` harness against the new scanner.

## B. Policy tooling (Tier-2 CEL)

The Tier-2 CEL evaluator (`crates/a2a-gateway/src/policy/tier2_cel/evaluator.rs`) is sound but has no authoring/testing toolchain around it.

- [x] **WI-07: `tier2-cel-test` fixture-replay CLI** (Value: H, Effort: S)
  - What: a binary consuming YAML/JSON fixtures `{id, input: {request, caller, agent, task, headers}, expected_verdict: Allow|Deny{rule}}`, exit 1 on any mismatch, gating CI on policy changes.
  - Port of AGT's `agt test` pattern (`schemas/fixture_schema.json`); reuse `trogon-decider-test`'s existing suite format and human/TAP dual output (`rsworkspace/cli/trogon-decider-test`). Estimated days, not weeks.

- [x] **WI-08: Lint pass for `.cel` bundles** (Value: M, Effort: S)
  - Landed as the `lint` subcommand of `cli/tier2-cel-test` (unbound variables, duplicate, contradictory, unreachable rules).
  - What: parse each `.cel` file, verify it references only the five bound variables (`request`, `caller`, `agent`, `task`, `headers`), warn on duplicate/contradictory/unreachable rules.
  - Mirrors `agt lint-policy` (`agent_compliance/lint_policy.py`) value without any Rego/Cedar machinery. Could live as a `trogon-decider-test`-style subcommand or standalone binary.

- [x] **WI-09: Dynamic policy conditions (`Tier2DynamicContext`)** (Value: M, Effort: M)
  - Landed as `.dynamic.toml` sidecars per rule (time window, day-of-week, token/cost per window), process-local counters per spec scope. Follow-up: audit-metadata enrichment of `AuditEnvelope` (spec section 4) not yet wired.
  - What: first-class temporal/budget primitives (time window, day-of-week, token/cost per window) evaluated alongside CEL rather than encoded inside CEL expressions.
  - Follow the additive design of `docs/specs/DYNAMIC-POLICY-CONDITIONS-1.0.md` (172 lines, deliberately minimal); keep counters process-local in v1 as the spec honestly scopes, durable quota accounting later via JetStream KV if needed.

- [x] **WI-10: Resource limits on `Tier2EvaluationContext`** (Value: M, Effort: S)
  - What: explicit numeric caps on `headers`/`params` size and nesting depth, breach fails closed.
  - Closes a DoS-shaped gap; reference constants in ACS `policy-engine/core/src/limits.rs` (1 MiB snapshot, depth 64, 256 KiB output, etc.).

- [x] **WI-11: ATR threat-rule corpus ingestion** (Value: M, Effort: L)
  - Scoped as proposal per this item: `docs/proposals/atr-threat-rule-ingestion.md`. Recommendation: no YAML-to-CEL compiler; hand-translate the narrow binding overlap and vendor CVE payloads as fixtures.
  - What: translate the ATR taxonomy (10 categories, 419 rules; curated 108-rule set with 99.6% precision / 96.9% recall claims and CVE regression tests) into CEL predicates over existing bindings.
  - AGT references: `examples/atr-import/`, `examples/atr-community-rules/`. The rule content is MIT-reusable text; the Rego/YAML compiler is not directly reusable. Decision needed first: whether Tier-2 wants a declarative-YAML-to-CEL compile step at all. Scope as a proposal, not a drop-in.

## C. Supply chain and repo hygiene

We SHA-pin actions and enforce crate licenses, but have no SBOM, no CODEOWNERS, no SECURITY.md, and no dependency freshness gating (Dependabot ships one grouped monthly PR unchecked).

- [x] **WI-12: Cargo-adapted supply-chain checks in CI** (Value: H, Effort: M)
  - What: adapt three stdlib-only AGT scripts to `Cargo.lock`/crates.io in a new `supply-chain.yml` workflow:
    - `scripts/check_lockfile_integrity.py`: verify pinned hashes against the registry.
    - `scripts/check_dependency_confusion.py`: guard the `trogon-*`/`trogonai-*` name namespace against registry squatting.
    - `scripts/check_release_age.py`: flag dependencies published under 7 days ago.
  - The `_supply_chain_common.py` helper pattern (registry timeout, safe-version regex, deadline budget) transfers; the registry API shapes differ.

- [x] **WI-13: Repo security baseline files** (Value: H, Effort: S)
  - What: add CODEOWNERS, SECURITY.md (reporting process and SLAs), CycloneDX SBOM generation (`scripts/generate_sbom.py` / `diff_sbom.py` as reference), and OSSF Scorecard workflow. Verified absent from this repo today.

- [x] **WI-14: Deterministic CI workflow generation** (Value: M, Effort: M)
  - Generates `sbom.yml`, `scorecard.yml`, `supply-chain.yml` from `.github/ci/workflows.toml` with `--check` drift gate (`ci-workflows-check.yml`); the 7 legacy workflows are listed as unmanaged.
  - What: generate `.github/workflows/*.yml` from a TOML source of truth with a `--check` CI gate and a single pinned-action registry, so every workflow shares one SHA-pin table instead of hand-pinning independently.
  - Reference: AGT's `scripts/ci/generate_workflows.py` plus `.github/ci/{workflows.toml,actions.toml}`. Note: upstream currently generates only one workflow (`policy-engine-ci.yml`) from the TOML; the other ~40 workflow files are hand-written. The pattern is what transfers, not a mature at-scale deployment.

- [x] **WI-15: File-level license header check** (Value: S, Effort: S)
  - Report-only for now (zero of 1477 `.rs` files carry headers today); `--enforce` flag exists for a future retrofit.
  - What: complement the existing `enforce-cargo-license` composite action (manifest-level) with a per-file header check; reference `scripts/check_license_headers.py`.

## D. Documentation genres

Documentation-only imports: zero code dependency, high signaling value.

- [x] **WI-16: RFC 2119 spec for the identity/auth guarantees** (Value: M, Effort: M)
  - `docs/specs/TROGON-IDENTITY-TRUST-1.0.md`, extracted from code with a non-guarantees section.
  - What: a normative spec with conformance checklist for what `a2a-auth-callout` and `trogon-aauth-verify` already guarantee in code (algorithm allow-list of ES256/ES384/EdDSA with JWK compatibility checks, replay windows, RFC 9421-style proof of possession over canonical NATS envelopes, opaque `DenialCategory` responses, duplicate security-header rejection).
  - Model: `docs/specs/AGENTMESH-IDENTITY-TRUST-1.0.md` structure (Terminology, Failure Semantics, Security Considerations, numbered MUSTs, Worked Examples).

- [x] **WI-17: Compliance mapping documents** (Value: M, Effort: M)
  - `docs/compliance/owasp-agentic-top10-mapping.md` (3 Full, 4 Partial, 3 Gap). NIST AI RMF skipped deliberately: mostly org-process subcategories with no code evidence available.
  - What: map TrogonAi's gateway/identity/redaction stack against OWASP Agentic Top 10 (and optionally NIST AI RMF) using AGT's document shape: framework item to Full/Partial/Gap with a named crate as evidence, plus the "internal self-assessment, not a certification" disclaimer. Reference: `docs/compliance/*.md`.

- [x] **WI-18: Adopt the three-tier audit conformance vocabulary** (Value: S, Effort: S)
  - `docs/compliance/audit-conformance-assessment.md`: currently between Level 1 and 2. Notable findings: `audit_ingress` module is unwired dead code; audit `trace_id` is a random UUID, not the OTel trace id.
  - What: assess `trogon-telemetry`/`trogon-semconv` against AUDIT-COMPLIANCE-1.0's tiers (Level 1 structured logging, Level 2 event sink SPI + OTel correlation, Level 3 tamper evidence + Decision BOM) and publish the gap statement. Feeds WI-20.

## E. Reliability, kill switch, audit

No kill switch, circuit breaker, SLO object, or tamper-evident audit exists in `rsworkspace/crates` today. Build these on NATS primitives, not as ports of AGT's in-process Python objects.

- [x] **WI-19: Kill switch as a decider aggregate with subject-permission enforcement** (Value: H, Effort: L)
  - Domain core and KV projection landed in `crates/trogon-kill-switch`. Enforcement wiring (stream provisioning, projection consumer, auth-callout integration) specified in `docs/proposals/kill-switch-enforcement.md` and still open.
  - Design (from section 24.5): a `KILL_SWITCH_EVENTS` JetStream stream recording `AgentKilled`/`AgentRevived` events (borrow AGT's `KillReason` taxonomy: behavioral drift, rate limit, ring breach, manual), current state projected into a NATS KV bucket the way ARD projects its catalog, enforcement by revoking publish/consume on the killed agent's subject prefix.
  - Structurally stronger than AGT's callback-based `KillSwitch.kill()`, which trusts the target agent to run its own termination callback.

- [x] **WI-20: Tamper-evident audit chain over JetStream** (Value: M, Effort: L)
  - `crates/trogon-audit-chain`: chain carried in `Trogon-Chain-*` headers per ADR 0013, publisher wrapper and replay verifier. External tip anchoring remains future work; residual rewind risk documented in the README.
  - Design: incremental SHA-256 hash chain (each event plus previous hash) stored as message headers or a companion KV keyed by stream sequence; `verify_chain()` as a stream-replay check. Achieves AGT's Level-3 audit target on a durable replicated log, where AGT's shipped commitment engine is in-memory only.

- [x] **WI-21: SLO-as-code semantics** (Value: S, Effort: M)
  - Concept port per this item: `docs/proposals/slo-as-code.md`, recommending a new `trogonai-slo-spec` crate with cycle-safe `extends` resolution in TOML.
  - What: borrow the YAML-with-inheritance semantics of `agent_sre/slo/spec.py` (`resolve_inheritance()`, cycle-safe parent merge) but express in TOML via `trogon-service-config`/confique per ADR 0007. Concept port only; the Python code does not transfer.

- [x] **WI-22: Fault taxonomy for chaos testing the gateway ingress** (Value: S, Effort: S)
  - `docs/testing/gateway-ingress-fault-checklist.md` plus three new deterministic ingress tests; 8 of 12 fault types documented as out of scope for a stateless ingestion boundary.
  - What: use AGENT-SRE-GOVERNANCE-1.0's 12-value `FaultType` taxonomy as the checklist for `trogon-gateway` webhook ingress tests (timeout injection, replay/dedup-window abuse, malformed payloads). The AGT chaos engine itself is Python-only and does not port.

---

## Explicit non-goals

Documented so they are not relitigated per item:

- **AGT's Python runtime objects** (`CostGuard`, `DeltaEngine`, `SLO`/`ErrorBudget`, `CircuitBreaker`): threading.Lock/in-memory designs; idiomatic rewrites on JetStream primitives beat FFI bridges in every case above.
- **Rego/Cedar dual-backend policy model**: two new policy runtimes duplicating what the single CEL evaluator (hot-reload, sorted-path determinism) already covers here; the additive dynamic-conditions layer (WI-09) captures the benefit more cheaply.
- **Numeric trust scoring** (0-1000, decay, tiers): a wholly new stateful subsystem; SpiceDB relational authorization is binary by design. If ever wanted, it needs its own proposal, not a work item here.
- **AGT's `action/` composite GitHub Actions as-is**: Python-runtime-coupled; copy the exit-code and JSONL-receipt contracts (see WI-12), not the actions.
