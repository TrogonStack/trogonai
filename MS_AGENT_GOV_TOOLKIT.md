# Microsoft Agent Governance Toolkit (AGT)

> A comprehensive reference of the project, its capabilities, and its ideas.
> Source: `tmp/agent-governance-toolkit` (microsoft/agent-governance-toolkit, version 5.0.0, MIT License, Public Preview).
> Generated from a full-repository survey on 2026-07-02. All claims were verified against files in the repository; repo-relative paths are cited throughout.
> Section 24 additionally draws on a source-level survey of the TrogonAi platform (this repository, `rsworkspace/`) to relate the two systems.

## Table of contents

1. [What the Agent Governance Toolkit is](#1-what-the-agent-governance-toolkit-is)
2. [Architecture and core concepts](#2-architecture-and-core-concepts)
3. [Package ecosystem and the agt CLI](#3-package-ecosystem-and-the-agt-cli)
4. [Agent OS](#4-agent-os)
5. [Agent Mesh](#5-agent-mesh)
6. [Agent Runtime and Agent Sandbox](#6-agent-runtime-and-agent-sandbox)
7. [Agent SRE](#7-agent-sre)
8. [Agent Compliance](#8-agent-compliance)
9. [Agent Hypervisor and Agent Discovery](#9-agent-hypervisor-and-agent-discovery)
10. [Agent Marketplace and Agent Lightning](#10-agent-marketplace-and-agent-lightning)
11. [RAG governance and MCP governance](#11-rag-governance-and-mcp-governance)
12. [Agent Control Specification (ACS) policy engine](#12-agent-control-specification-acs-policy-engine)
13. [Language SDKs and VS Code extension](#13-language-sdks-and-vs-code-extension)
14. [Coding-agent surfaces](#14-coding-agent-surfaces)
15. [Framework integrations](#15-framework-integrations)
16. [Formal specifications](#16-formal-specifications)
17. [Standards compliance](#17-standards-compliance)
18. [Security posture and red-teaming](#18-security-posture-and-red-teaming)
19. [Examples and demos catalog](#19-examples-and-demos-catalog)
20. [Documentation, tutorials, workshop, case studies, AGT Studio](#20-documentation-tutorials-workshop-case-studies-agt-studio)
21. [Repository self-governance and supply-chain security](#21-repository-self-governance-and-supply-chain-security)
22. [Ideas, proposals, and roadmap](#22-ideas-proposals-and-roadmap)
23. [Project governance, community, and licensing](#23-project-governance-community-and-licensing)
24. [Relationship to TrogonAi (this repository)](#24-relationship-to-trogonai-this-repository)

---

## 1. What the Agent Governance Toolkit is

Agent Governance Toolkit (AGT) is Microsoft's open source project (`microsoft/agent-governance-toolkit`, MIT License) providing "Policy enforcement, identity, sandboxing, and SRE for autonomous AI agents" (README.md). Its tagline is "Ship agents to production without losing sleep," positioned as "One `pip install`, any framework."

### The problem

README.md and docs/index.md frame the project around three questions that OAuth scopes and IAM roles do not answer:

1. **"Is this action allowed?"** An agent with access to `send_email` and `query_database` should not be able to `drop_table`. Scopes control which services an agent can reach, not what it does once connected.
2. **"Which agent did this?"** In a multi-agent system, five agents might share a single API key; "an agent did it" is not an incident response.
3. **"Can you prove what happened?"** Auditors and regulators need tamper-evident records of every decision: what policy was active, what the agent requested, and why it was allowed or denied.

### The deterministic-enforcement thesis

AGT's core argument is that prompt-level safety ("please follow the rules") is not a control surface but "a polite request to a stochastic system." README.md cites OWASP LLM01:2025 ("it is unclear if there are fool-proof methods of prevention for prompt injection"), Andriushchenko et al. (ICLR 2025, arXiv:2404.02151) reporting a 100% attack success rate on GPT-4o/GPT-3.5/Claude 3/Llama-3 via adaptive attacks evaluated against JailbreakBench (Chao et al., NeurIPS 2024, arXiv:2404.01318), Microsoft's own AI Red Teaming Agent formalization of Attack Success Rate (ASR), and the Microsoft Security blog "Lessons from Red Teaming 100 Generative AI Products" ("mitigations do not eliminate risk entirely").

AGT's answer is to not try to win inside the prompt: "Every tool call, message send, and delegation is intercepted in deterministic application code *before* the model's intent reaches the wire." Actions the AGT kernel denies are asserted to be "structurally impossible," not merely unlikely: "the difference between asking an agent to behave and making it incapable of misbehaving."

This is explicitly framed as **action governance**, not reasoning/content governance, a distinction that recurs throughout the project's documented limitations and competitive positioning (see sections 2 and 18).

### Two-line quickstart

The canonical entry point, shown in docs/quickstart.md, README.md, and docs/index.md, wraps any existing tool function in a policy-checked call:

```bash
pip install agent-governance-toolkit[full]
```

```python
from agentmesh.governance import govern

safe_tool = govern(my_tool, policy="policy.yaml")   # every call checked, logged, enforced
```

`safe_tool` evaluates the YAML policy on every call, logs the decision to an audit trail, and raises `GovernanceDenied` if blocked. An example `policy.yaml`:

```yaml
apiVersion: governance.toolkit/v1
name: production-policy
default_action: allow
rules:
  - name: block-destructive
    condition: "action.type in ['drop', 'delete', 'truncate']"
    action: deny
    description: "Destructive operations require human approval"

  - name: require-approval-for-send
    condition: "action.type == 'send_email'"
    action: require_approval
    approvers: ["security-team"]
```

Demonstrated behavior:

```python
>>> safe_tool(action="read", table="users")
{'table': 'users', 'rows': 42}

>>> safe_tool(action="drop", table="users")
GovernanceDenied: Action denied by policy rule 'block-destructive':
  Destructive operations required human approval
```

The example sets `default_action: allow` explicitly, which is fail-open unless covered by explicit deny rules. This is notable because a pending breaking change (tracked in BREAKING_CHANGES.md, detailed in section 2) moves the *implicit* (omitted) default from allow to deny across the Python, TypeScript, and .NET SDKs precisely because relying on an unset default was found unsafe. The quickstart's own examples remain correct since they declare `default_action` explicitly with covering deny rules.

### Version and release posture

The repository's `VERSION` file reads `5.0.0`, matching the top CHANGELOG.md entry `[5.0.0] - 2026-06-25`, described as "BREAKING: Monorepo-wide v5 alignment": all first-party Python, TypeScript, .NET, and Rust packages were bumped from `4.1.0` to `5.0.0` together with the top-level VERSION file, docs/ARCHITECTURE.md banner, and Claude Code plugin/marketplace manifests, and internal cross-package version caps widened from `<5.0` to `<6.0`. This aligns the release line with documentation describing the Agent Control Specification (ACS, see section 12) as the "AGT 5.0 policy layer." The independently versioned `policy-engine/` ACS engine is not part of this alignment: it stays at `0.3.1-beta` with its own Go module tag.

Despite the 5.0.0 version number, AGT is explicitly **not GA**. README.md and the CHANGELOG.md banner state: "Public Preview: production-quality public preview releases. May have breaking changes before GA." Releases are described as "Microsoft-signed and production-quality but may have breaking changes before GA." Version bumps track internal monorepo alignment and breaking API changes, not GA maturity.

The version history runs from 1.0.0 (2026-03-04, first Microsoft-org publish) through 1.1.0, 2.0.x, 2.1.0-2.3.0 ("Official Microsoft-Signed Public Preview," ESRP release signing), 3.0.x (Rust and Go SDKs added, security audit remediation), 3.1.x (unified `agt` CLI, end-to-end encrypted messaging via the Signal protocol), 3.2.x (AgentMesh Wire Protocol v1.0), 4.0.0 (2026-06-01, "BREAKING: Python package consolidation" from 45 packages into 5 distributions), to the current 5.0.0. Some documentation has not kept pace with this cadence: docs/ROADMAP.md still headers itself "Current Release: v3.7.0 (Public Preview)," a stale figure relative to both VERSION and CHANGELOG.md (see roadmap detail in section 22).

### Official distribution channels

README.md's "Official Sources" section warns that the only official channels are the GitHub repository, the microsoft.github.io documentation site, the PyPI user `agentgovtoolkit`, `@microsoft/agent-governance-sdk` on npm, `Microsoft.AgentGovernance.*` on NuGet, and the `agent-governance`/`agent-governance-mcp` crates on crates.io (though other project documents give the crate names as `agentmesh`/`agentmesh-mcp`, a naming inconsistency carried across the repository's own docs and detailed in section 3). The statement is explicit: "The project team does not maintain or endorse any third-party websites, packages, or documentation sites claiming to be official," with a request to report suspicious impersonation via SECURITY.md. Full per-language package names, install commands, and the Python package-consolidation matrix are covered in section 3.

### Tour of the monorepo

The repository is organized as a polyglot monorepo built around a shared set of governance primitives re-implemented across five language ecosystems. At its center is the policy engine (exposed at the Python `agent_os.policies` layer and, since 5.0.0, backed by the standalone Rust-based Agent Control Specification runtime in `policy-engine/`), surrounded by packages covering agent identity and trust scoring (AgentMesh), execution sandboxing and privilege rings (Agent Runtime and Agent Hypervisor), operational reliability (Agent SRE), compliance verification (Agent Compliance), plugin lifecycle governance (Agent Marketplace), reinforcement-learning training governance (Agent Lightning), and shadow-AI discovery (Agent Discovery). These are mirrored, with varying feature parity, in TypeScript (`agent-governance-typescript/`), .NET (`agent-governance-dotnet/`), Rust (`agent-governance-rust/`), and Go (`agent-governance-golang/`) source trees, alongside coding-agent surfaces for Claude Code, GitHub Copilot CLI, and OpenCode, formal specifications and conformance tests, ADRs documenting architectural decisions, and an extensive set of docs, tutorials, and adopter case studies. The full package ecosystem and CLI are detailed in section 3; architecture and ADRs in section 2; individual subsystems in sections 4 through 12; language SDKs and editor tooling in section 13; and repository governance, roadmap, and community structure in sections 21 through 23.

---

## 2. Architecture and core concepts

### Enforcement thesis

The Agent Governance Toolkit (AGT) argues that prompt-level safety ("please follow the rules") is not a control surface but a polite request to a stochastic system. Its README cites OWASP LLM01:2025, Andriushchenko et al. (ICLR 2025, arXiv:2404.02151, 100% attack success rate against GPT-4o, GPT-3.5, Claude 3, and Llama-3 via adaptive attacks on JailbreakBench), and Microsoft's own "Lessons from Red Teaming 100 Generative AI Products" ("mitigations do not eliminate risk entirely"). AGT's answer is to intercept every tool call, message send, and delegation in deterministic application code before the model's intent reaches the wire, so that denied actions are "structurally impossible," not merely unlikely. This is framed explicitly as action governance, not reasoning or content governance, a distinction repeated throughout the docs (see sections 2 and 18).

### Enforcement pipeline and layering

`docs/ARCHITECTURE.md` describes AGT as "deterministic application-layer interception": every agent action is evaluated against policy before execution, at sub-millisecond latency, composing with container/VM isolation for defense-in-depth. The System Architecture diagram (version-stamped "AGENT GOVERNANCE TOOLKIT v5.0.0") shows the core pipeline as Agent Action -> POLICY CHECK -> Allow/Deny at "< 0.1 ms", built from paired subsystems (see sections 4-10 for detail on each):

- **Agent OS Engine**: Policy Engine, Capability Model, Governance Gate, `GovernanceEventSink`, Decision BOM.
- **AgentMesh**: Zero-Trust Identity, Ed25519/SPIFFE certs, Trust Scoring (0-1000), Wire Protocol (A2A/MCP), Delegation Chains.
- **Agent Runtime**: Execution Rings 0-3, Resource Limits, Runtime Sandboxing, Termination Control.
- **Agent SRE**: SLO Engine and Error Budgets, Replay and Chaos Testing, Progressive Delivery, Circuit Breakers.
- **Agent Hypervisor**: Execution Audit, Delta Engine, Commitment Anchoring, Merkle Chain Logs.
- **Agent Lightning**: RL Training Governance, Violation Penalties, Reward Shaping, Training Checkpoints.
- **Agent Marketplace**: Plugin Discovery, Signing and Verification, Trust Scoring.
- **MCP Security Gateway**: Tool-Call Policy Checks, Trust Verification, Rate Limiting.
- **Framework Adapters**: LangChain, CrewAI, AutoGen, OpenAI, ADK, smolagents.

In short, an agent's action passes through identity and trust checks (AgentMesh), a deterministic policy decision (Agent OS), and is recorded in a tamper-evident audit trail (Agent Hypervisor / audit compliance), with SRE and runtime layers governing execution behavior and blast radius. `docs/modern-agent-architecture-overview.md` presents a simplified variant of this diagram using an explicit "operating-system analogy": Kernel maps to the Agent OS Policy Engine, the User/Kernel boundary to the Capability Model, process isolation to Privilege Rings, `SIGKILL` to the Kill Switch, audit logs to a hash-chained Flight Recorder, and a Certificate Authority to AgentMesh Identity (Ed25519), with an explicit caveat that this is "an architectural analogy, not a claim of OS-level isolation." Minor phrasing divergence exists between the two diagrams for Agent Runtime ("Resource Limits... Termination Control" versus "Saga Orchestration," "Kill Switch"), suggesting the two docs were not kept in lockstep.

`docs/ARCHITECTURE.md`'s Component Specifications table maps ten components to formal spec identifiers under `docs/specs/` (see section 16): `AGENT-OS-POLICY-ENGINE-1.0`, `AGENTMESH-IDENTITY-TRUST-1.0`, `AGENT-HYPERVISOR-EXECUTION-CONTROL-1.0`, `AGENTMESH-TRUST-COORDINATION-1.0`, `AGENT-SRE-GOVERNANCE-1.0`, `MCP-SECURITY-GATEWAY-1.0`, `AGENT-LIGHTNING-FAST-PATH-1.0`, `FRAMEWORK-ADAPTER-CONTRACT-1.0`, `AUDIT-COMPLIANCE-1.0`, `AGENTMESH-WIRE-1.0`.

### Security boundary

`ARCHITECTURE.md`'s Security Model & Boundaries table pairs each enforcement capability with a recommended defense-in-depth composition, for example: "intercepts and evaluates every agent action before execution" pairs with "add container isolation (Docker, gVisor, Kata)"; "provides cryptographic agent identity (Ed25519)" pairs with "add external PKI for certificate lifecycle management"; "maintains append-only audit logs with Merkle chains" pairs with "add external append-only sink (Azure Monitor, write-once storage)." The document is explicit that the POSIX metaphor (kernel, signals, syscalls) used elsewhere is "an architectural pattern," not literal OS isolation: **the actual enforcement boundary is the Python interpreter**, and the policy engine and agent share the same process boundary. The recommended production posture is to run each agent in a separate container, with the governance middleware inside providing application-level enforcement and the container boundary providing OS-level isolation. This caveat is echoed in the project overview material: "AGT enforces governance at the application middleware layer, not at the OS kernel level," part of a recommended four-layer defense-in-depth stack (Model Safety Layer, AGT Governance Layer, Application Layer, Infrastructure Layer), with AGT stated to be "one layer in a defense-in-depth strategy, not the entire strategy."

Trust scoring runs on a 0-1000 scale with five tiers: 900-1000 Verified Partner, 700-899 Trusted, 500-699 Standard, 300-499 Probationary, 0-299 Untrusted; new agents default to 500 (Standard). Benchmark figures such as sub-millisecond policy evaluation are measured on a 30-scenario internal test suite covering OWASP Agentic Top 10 categories; `ARCHITECTURE.md` is explicit that results are scoped to that suite, not a universal guarantee. A separate limitations document narrows this further: the published "<0.1ms" figure measures only the policy engine's deterministic rule evaluation. In a multi-agent mesh, full overhead also includes Ed25519 signature verification (1-3ms per message), trust score lookup (<1ms), IATP handshake on first contact (10-50ms), and network round-trip (1-10ms), yielding an expected 5-50ms per governed inter-agent interaction, dominated by cryptographic verification and network latency rather than the policy engine itself.

### Key architectural documents

- **`docs/ARCHITECTURE.md`** (121 lines): Overview, System Architecture, Component Specifications, Security Model & Boundaries, Trust Score Algorithm, Benchmark Methodology. States each major component has a formal RFC 2119 specification with conformance tests.
- **`docs/modern-agent-architecture-overview.md`** (349 lines): a technical decision-maker-facing overview covering the governance stack, the OS analogy, five core capabilities (deterministic policy enforcement, zero-trust identity, execution sandboxing, Agent SRE, MCP security scanning), OWASP Agentic Top 10 coverage (claimed 10/10), regulatory alignment (EU AI Act, Colorado AI Act SB 24-205), and framework compatibility (claimed "20+ agent frameworks").
- **`docs/a365-agt-reference-architecture.md`** (233 lines): positions Microsoft Agent 365 (A365) as providing foundational agent lifecycle management (Entra-based identity, Purview DLP, Defender threat signals) while AGT is "the runtime governance layer that complements A365," covering per-action policy and behavioral governance, framed explicitly as augmentation, not replacement, unified via a shared OpenTelemetry observability layer. Four integration patterns are documented: a .NET extension (`Microsoft.AgentGovernance.Extensions.Microsoft.Agents`, shipping today), an MCP Governance Proxy between the A365 MCP Registry and MCP servers, OpenTelemetry unified observability (`agent_os.StatelessKernel(enable_tracing=True)`), and shift-left CI/CD scanning via a GitHub Action.

### Architectural documentation gaps

Cross-document review surfaces stale or inconsistent figures that readers should treat as documentation debt rather than functional defects:

- `ARCHITECTURE.md` states "25 Architecture Decision Records" while 32 numbered ADRs (0001-0032) plus a template exist on disk in `docs/adr/`; `docs/ROADMAP.md` separately claims 25, and `README.md` elsewhere claims 29, three mutually inconsistent counts.
- `docs/adr/index.md` lists only 11 of the 32 numbered ADRs (0001-0009, 0027, 0028), omitting 0010-0026 and 0029-0032; it has not been kept current as new ADRs were added.
- A package-name inconsistency exists between docs: `modern-agent-architecture-overview.md` and `ARCHITECTURE.md` reference `pip install agent-governance-toolkit[full]`, while `a365-agt-reference-architecture.md`'s deployment checklist says `pip install agent-compliance`, a distinct package name (see section 3).
- Performance figures ("<0.1ms per policy evaluation," "47,000 ops/sec," "0.00% violation rate" versus "26.67%" for prompt-based safety) recur across the marketing-oriented docs but are the project's own self-reported benchmark claims from the 30-scenario suite, not third-party validated.

### Architecture Decision Records

ADRs live under `docs/adr/` (33 files: `0000-template.md` plus `0001` through `0032`, no gaps) and follow a lightweight MADR-style structure (Context, Decision, Consequences) defined by the template: `# ADR 0000: Short Decision Title`, `- Status: proposed`, `- Date: YYYY-MM-DD`. Two template eras exist: ADRs 0001-0016 and 0026-0032 use the `- Status:` / `- Date:` metadata format; ADRs 0017-0025 use an older `## Status` heading instead (all nine of that range are marked "Accepted"). Every ADR has a discoverable status.

| ADR | Title | Status | Decision (one line) |
|---|---|---|---|
| 0001 | Use Ed25519 for agent identity | Accepted | Standardize agent identity on Ed25519 with JWK/DID/SPIFFE interoperability instead of defaulting to RSA. |
| 0002 | Four execution rings instead of RBAC | Accepted | Use four execution rings as the primary runtime privilege model; RBAC and scoped capabilities remain complementary. |
| 0003 | IATP trust handshake within 200ms SLA | Accepted | Set a 200ms service-level target for the trust handshake gate. |
| 0004 | Policy evaluation deterministic, out of LLM control loop | Accepted | Keep enforcement-time policy evaluation deterministic (YAML/JSON, Rego/Cedar); never let an LLM decide allow/deny. |
| 0005 | Liveness attestation for TrustHandshake | Proposed | Add opt-in liveness attestation as a gate, decomposing trust into three independent properties. |
| 0006 | Constitutional constraint layer as community extension | Proposed | Add a veto-only, post-evaluation critic-agent check hooked into GovernanceGate; never able to execute actions itself. |
| 0007 | External JWKS federation for cross-org identity | Proposed | Add an opt-in `IdentityProvider` for JWKS federation alongside SPIFFE/SVID and Entra modules. |
| 0008 | Cross-org policy federation above identity | Proposed | Bilateral policy evaluation with intersection semantics, hash-chained attestations, asymmetric policy propagation. |
| 0009 | RFC 9334 (RATS) architecture alignment | Accepted | Add a backward-compatible `agentmesh.trust.endorsement` module aligning AGT with RATS attestation roles. |
| 0010 | TEE keystore with SEV-SNP attestation | Proposed | Add optional TEE-bound identity via `TEEKeyStore`, `AttestationCollector`, and an attestation verifier. |
| 0011 | Additive policy check contract | Proposed | Add a new `agent_os.policies.decision` module (`ViolationCategory`, `PolicyCheckResult`) without breaking existing shapes. |
| 0012 | Cost governance via observability policies | Accepted | Implement cost governance in agent-sre with tiered budgets and post-action enforcement. |
| 0013 | Fail closed on policy evaluation errors | Accepted | Any unhandled evaluation exception yields immediate deny with an audit entry marked `error: true`. |
| 0014 | Parent deny rules immutable in policy merge | Accepted | Child `override: true` on a parent deny rule is silently dropped with a warning; parent deny stands. |
| 0015 | Pluggable external policy backends via protocol | Accepted | Define `ExternalPolicyBackend` as a runtime-checkable `Protocol` returning a `BackendDecision`. |
| 0016 | Trust ceiling propagation for delegated agents | Accepted | A parent's trust score is a hard ceiling on child trust, via `min(initial_score, ceiling)`. |
| 0017 | Merkle chain for audit tamper evidence | Accepted | SHA-256 hash chain (`previous_hash`/`entry_hash` per `AuditEntry`) chosen over blockchain for simplicity/latency. |
| 0018 | Reconstructible Decision BOM over pre-built | Accepted | `DecisionBOMBuilder` queries protocol-based signal sources on demand rather than requiring pre-built reporting. |
| 0019 | OTel BatchSpanProcessor pattern for event sink | Accepted | `GovernanceEventProcessor` uses a bounded queue (1024, drop-on-full), background drain (2000ms), batch cap (100). |
| 0020 | Circuit breaker for event sink delivery | Accepted | Each sink gets an independent breaker: 5 consecutive failures trips OPEN, 60s cooldown to HALF_OPEN. |
| 0021 | CloudEvents envelope for mesh audit | Accepted | Adopt CloudEvents v1.0 for `AuditEntry.to_cloudevent()` export over a custom schema or raw OTel LogRecord. |
| 0022 | Compliance framework auto-mapping | Accepted | `ComplianceEngine` auto-maps actions to controls across EU AI Act, SOC2, HIPAA, GDPR. |
| 0023 | Append-only delta engine for hypervisor audit | Accepted | `DeltaEngine` hash-chains VFS changes into per-turn `SemanticDelta` records. |
| 0024 | RL training governance with violation penalties | Accepted | `GovernedEnvironment` subtracts a severity-scaled penalty from the RL reward per violation. |
| 0025 | Structural typing for sink/source protocols | Accepted | Use `typing.Protocol` with `@runtime_checkable`, not ABC inheritance, for extension points. |
| 0026 | Azure Functions PDP behind AI Gateway for Foundry agents | Proposed | Azure API Management as PEP, an Azure Function as PDP, for governing Foundry prompt-based agents. |
| 0027 | Dual-stack migration for MCP `2026-07-28` | Proposed | Stateless-first, dual-stack support for MCP `2026-07-28` while preserving `2025-11-25` compatibility. |
| 0028 | AGT Studio unified UI | Proposed | Single first-class UI launched via `agt ui`, backed by a local `agt serve` sidecar, packaged also as a VS Code/Cursor webview. |
| 0029 | Policy distribution and registries with verifiable trust | Proposed | Content-addressed signed policy bundles, pluggable resolvers (local/HTTPS/OCI/Git), mandatory `agt-policies.lock`. |
| 0030 | Action-bound, fail-closed approval protocol | Proposed | `require_approval` is a suspended decision; execution stays impossible until a terminal approval resolution. |
| 0031 | Optional embedding evidence backend for prompt-injection detection | Proposed | Pluggable, default-off `DetectionEvidenceBackend` (Python `Protocol` and Rust `trait`). |
| 0032 | AGT emits TRACE v0.1 Trust Records | Accepted | One TRACE v0.1 Trust Record per session at close via `TRACEAuditSink`, Phase 1 software-only (Level 0). |

### Known limitations (architectural)

`docs/LIMITATIONS.md` states its purpose plainly: "Transparency is a feature. This document describes what AGT does not do." Points most relevant to the architecture: AGT is action governance, not reasoning governance, and cannot detect malicious workflows composed of individually-permitted actions; audit logs record attempts and allow/deny decisions, not real-world outcomes; the enforcement boundary is the shared Python process, not the OS kernel or hardware; and if no policies are loaded, the default action is `allow`, so misconfiguration can silently leave agents ungoverned (a gap a pending breaking change moves toward fail-closed by default). A companion "What AGT Is Not" framing summarizes the boundary: runtime action governance, not model safety or content moderation; deterministic policy enforcement, not probabilistic guardrails; application-layer middleware, not OS kernel or hardware isolation; a framework-agnostic library, not a managed cloud service.

---

## 3. Package ecosystem and the agt CLI

### 3.1 The v4.0.0/5.0.0 Python consolidation

Historically the Python side of AGT shipped 45 separate packages. `docs/package-consolidation/AUDIT.md` inventories all 45 (11 published to PyPI with real download numbers, e.g. `agent-sandbox` about 87K/mo, `agent-governance-toolkit` about 63K/mo, `agent-os-kernel` about 59K/mo, `agent-sre` about 46K/mo, `agentmesh-runtime` about 37K/mo; the remaining 34 unpublished or negligible). CHANGELOG.md records this consolidation as **4.0.0** (2026-06-01, "BREAKING: Python package consolidation," 45 packages into 5 distributions), followed by **5.0.0** (2026-06-25, "BREAKING: Monorepo-wide v5 alignment") bumping all first-party Python, TypeScript, .NET, and Rust packages from 4.1.0 to 5.0.0 and widening internal cross-package version caps from `<5.0` to `<6.0`. The independently versioned `policy-engine/` ACS engine (stays at `0.3.1-beta`) and its separately tagged Go module are not rolled into the 5.0.0 alignment.

Two documents describe the consolidation and disagree slightly on naming. `docs/package-consolidation/` (PROPOSAL.md, AUDIT.md, MIGRATION.md, dated 2026-05-23, GitHub issue #2482) is a Microsoft-authored RFC proposing a 3-phase rollout (stub packages, then deprecation warnings, then stub removal after about 2 minor versions / 6 months), stating the plan "requires a community feedback period... minimum review window of 7 days." `docs/package-migration.md`, a later document framed around AAIF/foundation-hosting naming, is the canonical record of the current, actually-implemented state and is more authoritative for present-day package identity. It is explicit that `agt-policies` and `agent-control-specification` are kept as their own canonical packages, outside this consolidation, because they belong to the separate ACS/policy-engine effort (see section 12).

The four real consolidated distributions live in `agent-governance-python/agent-governance-toolkit-core/`, `-cli/`, `-integrations/`, `-protocols/`. Each directory contains only a `pyproject.toml` and `README.md` (no local `src/`); each builds via hatchling with `[tool.hatch.build.targets.wheel.force-include]` rules that pull real source trees from sibling directories into the wheel. Current version is 5.0.0 for all four.

| Distribution | PyPI name | Consolidates |
|---|---|---|
| Core | `agent-governance-toolkit-core` | `agent-os-kernel`, `agentmesh-primitives`, `agentmesh-runtime`, `agent-hypervisor`, `agentmesh-platform` |
| CLI | `agent-governance-toolkit-cli` | `agent-sre`, `agent-sandbox`, `agentmesh-mcp-trust` |
| Integrations | `agent-governance-toolkit-integrations` | 18 framework adapters from `agentmesh-integrations/` |
| Protocols | `agent-governance-toolkit-protocols` | `agent-mcp-governance`, `a2a-protocol`, `mcp-receipt-governed`, `mcp-trust-proxy` |

CLI, Integrations, and Protocols each require only `agent-governance-toolkit-core>=4.1.0,<6.0` as their mandatory dependency (the floor still reads 4.1.0, accepting either the pre- or post-bump core); Core's mandatory dependencies are pydantic, pyyaml, rich, cryptography, pynacl, httpx, aiohttp, structlog, click, python-dateutil, jsonschema, and agentrust-trace.

**Core** force-includes `agent_os` (kernel), `cmvk`, `caas`, `emk`, `iatp`, `amb_core`, `atr`, `agent_control_plane`, `agent_os_observability`, `nexus` (individual files/schemas only, not the whole module dir), `mcp_kernel_server`, `agent_primitives`, `agent_runtime`, `hypervisor`, and `agentmesh`. Its documented invariant is import-path stability: `from agent_os.kernel import GovernanceKernel`, `from agent_primitives.failures import FailureType`, `from agent_runtime.supervisor import Supervisor`, `from hypervisor.session import SharedSession`, and `from agentmesh.identity import AgentIdentity` are unchanged pre- and post-consolidation. Optional extras: `cmvk`, `iatp`, `amb`, `observability`, `mcp`, `nexus`, `api`, `blockchain`, `otel`, `redis`, `server`, `storage`, `langchain`, `django`, `websocket`, `grpc`, a `full` bundle, and a `dev` extra. `[project.scripts]` contribute `agent-os`, `mcp-scan`, `hypervisor`, and `agentmesh`; none of these is the unified `agt` command (that lives in `agent-compliance`, see 3.3).

**CLI** force-includes `agent_sre`, `agent_sandbox`, and `mcp_trust_server`. Extras: `docker`, `hyperlight`, `policy`, `mcp`, `api`, `otel`, a long tail of observability-vendor extras (`langfuse`, `arize`, `langchain`, `llamaindex`, `braintrust`, `helicone`, `datadog`, `sentry`, `langsmith`, `wandb`, `mlflow`, `agentops`), and `full`. Its only console script is `agent-sre` (SLOs, error budgets, chaos testing). `agent-compliance`'s `agt red-team` commands lazily import `agent_sre.chaos.adversarial` and `agent_sre.chaos.engine` from this distribution at runtime, so the CLI distribution and the `agt` meta-CLI are integrated via optional import, not a build-time dependency edge.

**Integrations** carries no mandatory framework dependency beyond core; every adapter is an optional extra force-included from `agentmesh-integrations/`. Extras with pinned versions: `langchain` (langchain-core>=1.2.11,<2.0), `crewai` (>=0.100.0,<1.0), `openai-agents` (>=0.0.3,<1.0), `langgraph` (>=0.2.0,<1.0), `llamaindex` (llama-index-core>=0.12,<0.13), `haystack` (haystack-ai>=2.0,<3.0), `pydantic-ai` (>=0.0.10,<1.0), `adk` (google-adk>=0.1.0,<1.0), `cedarling` (cedarpy>=4.0.0,<5.0), `openshell` (pyyaml). Extras with no listed dependency (vendored or zero-dep): `flowise`, `langflow`, `avp`, `nostr-wot`, `structural-authz`, `audit-export`. Corresponding module names include `langchain_agentmesh`, `crewai_agentmesh`, `openai_agents_agentmesh`, `langgraph_trust`, `llama_index`, `haystack_agentmesh`, `pydantic_ai_governance`, `adk_agentmesh`, `cedarling_agentmesh`, `openshell_agentmesh`, `agentmesh_avp`, `agentmesh_nostr_wot`, `structural_authz_agentmesh`, `audit_accountability_export`, `flowise_agentmesh`, `langflow_agentmesh`, `template_agentmesh`. `a2a-protocol`, `mcp-receipt-governed`, and `mcp-trust-proxy` under `agentmesh-integrations/` are explicitly not force-included here; they belong to the protocols distribution instead. The only console script is `openshell-agentmesh`.

**Protocols** force-includes `agent_mcp_governance`, `a2a_agentmesh`, `mcp_receipt_governed`, and `mcp_trust_proxy`, covering "MCP governance primitives, A2A protocol adapter, MCP receipt governance, and the MCP trust proxy." No console scripts; only a `dev` extra (pytest, pytest-asyncio, ruff).

Legacy pre-consolidation package names remain installable as stub packages that redirect to the consolidated distributions rather than being dead: `agent-os-kernel`, `agentmesh-platform`, `agentmesh-runtime`, `agent-sre`, `agent-discovery`, `agent-hypervisor`, `agentmesh-marketplace`, `agentmesh-lightning`. The verified concrete instance is `agent-primitives` (distribution name `agentmesh_primitives`, path `agent-governance-python/agent-primitives/agent_primitives/`), whose `pyproject.toml` description literally reads "Deprecated, use agent-governance-toolkit-core instead." It uses setuptools (not hatchling), depends on `agent-governance-toolkit-core>=4.1.0,<6.0` (the very distribution that vendors its source via force-include) plus `pydantic`, and its `__init__.py` unconditionally emits a `DeprecationWarning` pointing at `MIGRATION.md` before re-exporting `FailureType`, `FailureSeverity`, `FailureTrace`, `AgentFailure` from `.failures`. Its in-code `__version__ = "3.2.2"` is stale relative to the `5.0.0` in `pyproject.toml`, a live example of the version-skew problem `AUDIT.md` warns about, and its README still claims "Zero Agent OS Dependencies (only depends on pydantic)," which the added core dependency now contradicts.

A separate, currently empty, and easily confused set of directories, `agt-core/`, `agt-cli/`, `agt-protocols/`, `agt-integrations/`, each contains only a `pyproject.toml` declaring the same long-form `name` as its real counterpart (e.g. `agt-core/pyproject.toml` declares `name = "agent-governance-toolkit-core"`, version 5.0.0) but `packages = []` and no force-include rules, producing an empty wheel. These are tracked in git with no distinct commit history and no references elsewhere; they read as orphaned scaffolding duplicating the real distributions' `name=` metadata, a naming-collision risk if ever built and published under the same PyPI project name, and should not be documented as real, separate distributions.

The one populated `agt-*`-named package is `agt-policies` (real `src/agt/` code, tests, README), the Python wrapper over the Agent Control Specification engine; it is covered in section 12 together with ACS.

Documentation has not fully caught up with the consolidation: `docs/PACKAGE-FEATURE-MATRIX.md` ("Last updated: April 2026," predating the 5.0.0 bump) still calls out "Replay Debugging" as `agent-sre` and "20+ Framework Adapters" as `agentmesh-integrations` by pre-consolidation names, and the `agt doctor` command's own health-check package list (below) still enumerates pre-consolidation names (`agent_os_kernel`, `agentmesh_platform`, `agentmesh_runtime`, `agent_sre`, `agentmesh_marketplace`, `agentmesh_lightning`, `agent_hypervisor`) rather than the four consolidated distributions.

### 3.2 The agt unified CLI

There is exactly one `agt` console-script CLI in the codebase, defined in the `agent-compliance` package (distribution name `agent-governance-toolkit-compliance`, version 5.0.0, "Public Preview, Unified installer and runtime policy enforcement for the Agent Governance Toolkit"), not in `agt-cli`, `agent-governance-toolkit-cli`, or `agt-policies`. Its `[project.scripts]` define six entry points from one package:

- `agent-governance-toolkit`, `agent-governance`, `agent-compliance`: all three alias the older argparse-based CLI (`cli/main.py`), exposing only `verify`, `integrity`, `lint-policy`.
- `agt`: the modern Click-based unified CLI, module `agent_compliance/cli/agt.py`.
- `agt-contributor-check`, `agt-credential-audit`: standalone argparse scripts for supply-chain/reputation analysis.

Extras on `agent-compliance` are dependency-forwarders: `core`/`integrations`/`cli`/`protocols` (each `>=4.1.0,<6.0`), per-framework passthroughs to integrations extras, legacy-compat extras (`kernel`/`mesh`/`runtime` to core, `sre` to cli, `cedar` to cedarpy), and `full` installing all four.

`agt` global options: `--json`, `--verbose/-v`, `--quiet/-q`, `--no-color`; version resolved via `importlib.metadata.version("agent-governance-toolkit-compliance")`. Subcommands:

- `agt verify [--badge] [--evidence PATH]`: runs `GovernanceVerifier().verify()` or `.verify_evidence(...)`, prints a `GovernanceAttestation` summary/JSON/badge, exits 1 if not passed. A hidden no-op `--strict` flag is kept for backward compatibility ("strict is now the default").
- `agt integrity [--manifest PATH] [--generate OUTPUT_PATH]`: wraps `IntegrityVerifier`; `--generate` writes a hash manifest (files plus function bytecode hashes), otherwise verifies against one.
- `agt lint-policy PATH [--strict]`: wraps `lint_path`; `--strict` turns warnings into a failing exit code.
- `agt test POLICY_PATH FIXTURE_PATH`: replays policy fixtures via `policy_test.replay`, exits 1 on any verdict mismatch (CI gating on policy changes).
- `agt doctor`: checks installed versions of a fixed, pre-consolidation-named package list (see above), lists discovered `agt.commands` plugins, and checks for `agentmesh.yaml`, `policies/`, `integrity.json` in the current directory.
- `agt red-team` (Click subgroup): `scan PATH [--min-grade {A,B,C,D,F}] [--json] [--strict]` (static prompt-defense scan); `list-playbooks [--json]` (lazily imports `agent_sre.chaos.adversarial.BUILTIN_PLAYBOOKS`, errors with an install hint if `agent-sre` isn't installed); `attack [--target NAME] [--playbook ID] [--threshold N] [--json]` (runs adversarial playbooks, fails below threshold, default 70); `report --prompt-dir DIR [--target NAME] [--output PATH] [--json] [--min-grade ...]` (combines prompt-defense, 40 percent weight, and adversarial testing, 60 percent weight, into a letter-graded report).
- `agt cred` (Click subgroup): credential vault persisted to `$AGT_VAULT_PATH` (default `./.agt/vault.bin`), encrypted with a Fernet key from `$AGT_VAULT_KEY`, backed by `agent_os.credential_vault.CredentialVault` (a runtime dependency on core). Subcommands: `genkey`, `add NAME VALUE [--type TYPE]` (value can be `-` for stdin, never echoed), `list [--json]` (names/type/version only), `rotate NAME NEW_VALUE`, `remove NAME`.

`agt` discovers third-party subcommands via the `agt.commands` entry-point group; a broken plugin logs a warning rather than crashing the CLI, via a custom `AgtGroup` (`click.Group` subclass). `agt-contributor-check` and `agt-credential-audit` are standalone argparse scripts for GitHub-side supply-chain and reputation analysis, each scoring findings into a LOW/MEDIUM/HIGH verdict (exit codes 0/1/2): `contributor_check.py` scores account-shape, repo-theme concentration, issue-spray patterns, fork bursts, batch-naming, and feature-overlap-with-AGT signals; `credential_audit.py` detects "credential laundering" (citing merged PRs from a target repo as credentials in issues filed elsewhere).

### 3.3 Full per-language install matrix

| Language | Package | Command |
|---|---|---|
| Python | `agent-governance-toolkit` (PyPI) | `pip install agent-governance-toolkit[full]` |
| TypeScript | `@microsoft/agent-governance-sdk` | `npm install @microsoft/agent-governance-sdk` |
| Copilot CLI | `@microsoft/agent-governance-copilot-cli` | `npx @microsoft/agent-governance-copilot-cli install` |
| Claude Code | `@microsoft/agent-governance-claude-code` | `claude --plugin-dir ./agent-governance-claude-code`, or as a plugin marketplace |
| OpenCode | `@microsoft/agent-governance-opencode` | `npm install @microsoft/agent-governance-opencode` |
| .NET | `Microsoft.AgentGovernance` (NuGet) | `dotnet add package Microsoft.AgentGovernance` |
| .NET MCP | `Microsoft.AgentGovernance.Extensions.ModelContextProtocol` | `dotnet add package Microsoft.AgentGovernance.Extensions.ModelContextProtocol` |
| Rust | `agent-governance` (crates.io) | `cargo add agent-governance` |
| Go | `agent-governance-toolkit` (module) | `go get github.com/microsoft/agent-governance-toolkit/agent-governance-golang` |

Prerequisites: Python 3.10+ (`agent-governance-toolkit-core` itself requires `>=3.11`), Node.js 18+/npm 9+, .NET 8+, Go 1.25+, Rust 1.70+. Optional Azure integration variables: `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_CLIENT_SECRET`.

Naming discrepancy: `docs/PACKAGE-FEATURE-MATRIX.md` and `docs/FAQ.md` give the Rust crate name as `agentmesh` (`cargo add agentmesh`) plus a standalone `agentmesh-mcp` crate, differing from the root README's `agent-governance`/`agent-governance-mcp` names; both appear in the repo's own documentation, indicating an in-progress naming transition rather than an error in one source. Similarly, `docs/INDEPENDENCE.md`'s adapter table labels a row `langchain-agentmesh` but gives its pip command as `pip install agentmesh-langchain`.

### 3.4 Integration tiers

`docs/integration-tiers.md` defines three depths of integration, framed around how much OWASP mitigation coverage each requires in exchange for how much app-code change:

| Tier | Effort | Owner | Self-assessed OWASP ASI coverage |
|---|---|---|---|
| Tier 0: Sidecar / Proxy / CLI | Zero app-code changes; deploy a container, mount policy YAML, call HTTP endpoints | Platform/infra team | About 3/11 partial |
| Tier 1: SDK Init | Import SDK, initialize classes, call at key agent-loop points; about 10-50 LOC | Agent developer | About 6/11 full |
| Tier 2: Deep Integration | Middleware pipelines, decorators, behavior monitors, memory guards wired into the runtime; 100+ LOC, framework-specific | Agent developer plus platform team | About 8/11 full, 3/11 partial |

Tier 0 deploys `ghcr.io/microsoft/agentmesh/governance:latest` as a sidecar with HTTP endpoints (`/api/v1/detect/injection`, `/api/v1/execute`, `/health`, `/ready`, `/api/v1/metrics`) and the `agent-os mcp-scan` CLI, but provides no transparent interception (an agent bypassing orchestration bypasses governance), no trust scoring/identity, and no tamper-evident audit chain. Tier 1 adds `TrustManager`, `AgentIdentity`, `PolicyEvaluator`, `MCPSessionAuthenticator`, `CredentialRedactor`, `AuditLogger` (hash-chain), `TokenBudgetTracker`, `RateLimiter`. Tier 2 adds `AgentBehaviorMonitor`, `ExecutionRing`, `KillSwitch`, a `GovernanceMiddleware` pipeline, `DriftDetector`, `MemoryGuard`, `RogueDetector`, framework adapters, and `MCPGateway` (5-stage pipeline). Documented limitations of the tier model itself: transparent interception (a service-mesh-style Istio/Envoy proxy) is on the roadmap but not shipped; several detection modules (`PromptInjectionDetector`, `TokenBudgetTracker`, `RateLimiter`, `ScopeGuard`, `SupplyChainGuard`) exist as standalone utilities not auto-wired into the `BaseIntegration` lifecycle; and framework adapter enforcement depth varies (Microsoft Agent Framework/Semantic Kernel deepest, others may need manual wiring for Tier 2).

---

## 4. Agent OS

Agent OS (`agent-governance-python/agent-os`) is the Python governance kernel of the toolkit: application-level middleware that intercepts and validates agent tool/action calls before execution, enforcing declarative or programmatic policies deterministically rather than relying on LLM prompt compliance. Per its `README.md` and `ARCHITECTURE.md`, it explicitly disclaims true OS-kernel isolation: agents run in the same process, direct stdlib calls (`subprocess`, `open`) bypass it, and container isolation is recommended for defense in depth ("Known Architectural Limitations").

### Packaging status and caveat

The package publishes to PyPI as `agent_os_kernel`, currently version 5.0.0 (`agent-governance-python/agent-os/pyproject.toml`), and is **deprecated**: the `pyproject.toml` description reads "Deprecated. Previously published as agent-os-kernel. Install agent-governance-toolkit-core instead." The published wheel is a dependency-only stub whose sole runtime dependency is `agent-governance-toolkit-core>=4.1.0,<6.0`; importing `agent_os` triggers a `DeprecationWarning` (`src/agent_os/__init__.py`, lines 14-20) pointing to `docs/package-consolidation/MIGRATION.md`.

Despite the deprecation, the full `src/agent_os` source tree and its tests remain actively linted, type-checked, and exercised by CI via `pip install -e ".[dev]"`. A comment block in `pyproject.toml` explains that a prior refactor (PR #2794) had stripped `[project.optional-dependencies]` entirely, silently breaking `pytest tests/` collection because dependencies like `fastapi`, `pynacl`, `cryptography`, and `aiohttp` became unavailable; the current `dev` extra restores them. The code documented here is therefore a real, tested, importable module tree (installable from source via `pip install -e .`), even though the PyPI-published `agent_os_kernel` artifact no longer ships it directly. Requires Python >=3.11 (the README badge says 3.9+, but `requires-python` is `>=3.11`). Build backend is `hatchling`; optional extras are only `dev` (test/lint/type tooling) and `embedding` (`fastembed>=0.3.0,<1.0`, used by the optional embedding-based prompt-injection signal). The README's advertised extras (`[cmvk]`, `[iatp]`, `[observability]`, `[nexus]`, `[full]`) are not present in this `pyproject.toml`, so those install commands describe an aspirational/historical packaging surface rather than what is currently declared.

### Directory layout

- `src/agent_os/`: the core Python package.
- `modules/`: separately packaged kernel modules in a claimed four-layer architecture: Layer 1 primitives (`primitives`, `cmvk`, `emk`, `caas`), Layer 2 infrastructure (`amb`, `iatp`, `atr`), Layer 3 framework (`control-plane`, described as "THE KERNEL," and `observability`), Layer 4 intelligence (`mcp-kernel-server`), plus an unlayered experimental `nexus`.
- `extensions/`: IDE/assistant integrations (VS Code, JetBrains, Cursor, Chrome, GitHub CLI, MCP server).
- `examples/`, `docs/`, `tests/`, `notebooks/`, `papers/`, `templates/policies/`, `benchmarks/`, `charts/agent-os/` (Helm chart).

### Module inventory and public API

`src/agent_os/` contains 43 flat modules, including `stateless.py` (`StatelessKernel`), `base_agent.py` (`BaseAgent`/`ToolUsingAgent`), `agents_compat.py` (AGENTS.md parser), `semantic_policy.py`, `prompt_injection.py`/`prompt_injection_embedding.py`, `mcp_auth_enforcement.py`, `mcp_cve_feed.py`, `mcp_gateway.py`, `mcp_message_signer.py`, `mcp_protocols.py`, `mcp_response_scanner.py`, `mcp_security.py`, `mcp_session_auth.py`, `mcp_sliding_rate_limiter.py`, `credential_redactor.py`, `credential_vault.py`, `audit_logger.py`, `circuit_breaker.py`, `content_governance.py`, `context_budget.py`, `egress_policy.py`, `escalation.py`, `event_bus.py`, `event_sink.py`, `execution_context_policy.py`, `github_enterprise.py`, `shift_left_metrics.py`, `sandbox.py`/`sandbox_provider.py`, `secure_codegen.py`, `security_skills.py`, `trust_root.py`, and subpackages `cli/`, `exporters/`, `integrations/`, `policies/`, `server/`.

`__init__.py` re-exports over 140 names via `__all__`, in commented blocks:

- **Metadata**: `__version__` ("5.0.0"), `AVAILABLE_PACKAGES` (importability of `agent_control_plane`, `agent_primitives`, `cmvk`, `caas`, `emk`, `amb_core`, `atr`, `agent_kernel`, `mute_agent`), `check_installation()`.
- **Control Plane (optional)**: guarded by `try/except ImportError` on the separately-distributed `agent_control_plane` package (PyPI `agent-control-plane`). When installed, re-exports `AgentControlPlane`, `create_control_plane`, `AgentSignal`, `SignalDispatcher`, `AgentKernelPanic`, `SignalAwareAgent`, `kill_agent`, `pause_agent`, `resume_agent`, `policy_violation`, `AgentVFS`, `VFSBackend`, `MemoryBackend`, `FileMode`, `create_agent_vfs`, `KernelSpace`, `AgentContext`, `ProtectionRing`, `SyscallType`, `SyscallRequest`, `SyscallResult`, `KernelState`, `user_space_execution`, `create_kernel`, `PolicyEngine`, `PolicyRule` (distinct from `agent_os.policies.schema.PolicyRule`), `FlightRecorder`, `ExecutionEngine`, `ExecutionStatus`. The README calls this "the KERNEL," requiring `pip install agent-os-kernel[full]`. This is where control-plane concepts such as protection rings, syscalls, a virtual filesystem (VFS), and POSIX-style signals live.
- **Stateless API** (always available, pydantic-only): `StatelessKernel`, `ExecutionContext`, `ExecutionRequest`, `ExecutionResult`, `StatelessMemoryBackend`, `stateless_execute`.
- **Base Agent classes**: `BaseAgent`, `ToolUsingAgent`, `AgentConfig`, `AuditEntry`, `PolicyDecision`, `TypedResult`.
- **Semantic policy engine**: `SemanticPolicyEngine`, `IntentCategory`, `IntentClassification`, `PolicyDenied`.
- **Prompt injection detection**: `PromptInjectionDetector`, `InjectionType`, `ThreatLevel`, `DetectionResult`, `DetectionConfig`.
- **MCP security**: `MCPSecurityScanner`, `MCPThreatType`, `MCPSeverity`, `MCPThreat`, `ToolFingerprint`, `ScanResult`, `CredentialRedactor`, `CredentialPattern`, `CredentialMatch`, plus credential vault types (`CredentialVault`, `CredentialInjector`, `CredentialHandle`, `CredentialProfile`, `CredentialRecord`, `VaultAuditEvent`, `DenyReceipt`), MCP session/nonce/rate-limit/audit stores (`MCPSessionStore`, `MCPNonceStore`, `MCPRateLimitStore`, `MCPAuditSink`, and `InMemory*` variants), `MCPResponseScanner`, `MCPSessionAuthenticator`, `MCPMessageSigner`, `MCPSlidingRateLimiter`.
- **Other re-exports**: LlamaFirewall integration (`LlamaFirewallAdapter`, `FirewallMode`, `FirewallVerdict`), Context Budget Scheduler (`ContextScheduler`, `ContextWindow`, `BudgetExceeded`), Content Governance (`ContentQualityEvaluator`, `QualityGate`), Execution Context Policy (`ContextualPolicyEngine`, `EnforcementLevel`), GitHub Enterprise integration (`EnterpriseGovernanceManager`, `GovernanceTier`), shift-left metrics (`ShiftLeftTracker`), audit/event sink types (`GovernanceAuditLogger`, `OTelLogsBackend`, `GovernanceEvent`, `GovernanceEventSink`, `GovernanceEventProcessor`).

`agent_os.policies.PolicyEvaluator`, `PolicyDocument`, `PolicyRule`, and related schema types are not re-exported at the top-level package; they must be imported directly from `agent_os.policies`.

### The declarative policy engine (`agent_os/policies/`)

This subpackage (24 files) is the standalone YAML/JSON-driven governance engine. Schema types (`schema.py`): `PolicyOperator` (`eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `in`, `not_in`, `matches`, `contains`), `PolicyAction` (`allow`, `deny`, `audit`, `block`), `DynamicConditionType` (`time_window`, `day_of_week`, `token_count_per_window`, `cost_per_window`), `DynamicCondition`, `PolicyCondition` (`field`, `operator`, `value`), `PolicyRule` (`name`, `condition`, `action`, `priority`, `message`, `dynamic_condition`, `override`), `PolicyDefaults` (`action: DENY` by default, fail-closed, explicitly matching the TS and .NET SDKs; `max_tokens=4096`, `max_tool_calls=10`, `confidence_threshold=0.8`, plus sandbox-only fields `max_cpu`, `max_memory_mb`, `timeout_seconds`, `network_default="deny"`), `SandboxMounts`, and `PolicyDocument` (`version`, `name`, `rules`, `defaults`, `inherit`, `scope`, `network_allowlist`, `tool_allowlist`, `sandbox_mounts`, with `from_yaml`/`to_yaml`/`from_json`/`to_json`). A companion `policy_schema.json` (draft-07) formally mirrors this model and adds an `a2a_conversation_policy` property not present in the Pydantic schema. A separate, richer vocabulary in `docs/policy-schema.md` targets the `.agents/security.md` CLI file format (ABAC conditions, `conditional_permission`, `resource_quota`, `risk_policy`) and should not be conflated with `PolicyDocument`.

`PolicyEvaluator` (`evaluator.py`): constructed with `policies` and/or `root_dir`; `load_policies(directory)` loads `*.yaml`/`*.yml`; `add_backend()` registers an `ExternalPolicyBackend`; `load_rego()`/`load_cedar()` construct `OPABackend`/`CedarBackend`. `evaluate(context, dynamic_context=None) -> PolicyDecision` dispatches to folder-scoped or flat evaluation depending on whether `root_dir` and `context["path"]` are set. In flat evaluation, rules sort by `priority` descending, first match wins; `DENY` and `BLOCK` both count as not-allowed. If no rule matches, registered backends (Rego/Cedar) are consulted in order, and a backend error causes an immediate fail-closed deny (issue #2992 fix: "skipping an errored backend and falling through to the default action discards the backend's intended fail-closed deny"). If nothing matches, the evaluator defaults to `PolicyAction.DENY` when no policies are loaded, matching the TS/.NET SDKs' default-deny posture; the entire evaluation is wrapped in `try/except`, returning a fail-closed deny on any unhandled exception.

Folder-scoped evaluation (`discovery.py`) walks from an action's path upward to `root_dir` collecting `governance.yaml`/`.yml` files, with an anti-path-traversal guard: it refuses to walk above `root` and returns `[]` if the resolved path is not relative to it. `merge.py` merges the root-first document chain, enforcing that a parent rule with `action == DENY` can never be overridden by a child, even with `override=True` (compared to "Azure Policy semantics" in the docstring). `dynamic_conditions.py` provides `DynamicConditionEvaluator.evaluate()` for time-of-day, day-of-week, and per-window token/cost budget gating. Other supporting modules include `backends.py` (`OPABackend`, `CedarBackend`), `rate_limiting.py` (`TokenBucket`), `decision.py` (`PolicyCheckResult`, `ViolationCategory`), `conflict_resolution.py`, the `context_*` family (accumulation, aggregation, audit, delegation, envelope), `obligations.py`, and `shared.py` (a cross-SDK shared policy schema bridge).

### StatelessKernel: the zero-dependency governance gate

`agent_os/stateless.py` (1048 lines) implements `StatelessKernel`, a pydantic-only execution kernel where every `execute()` call is funneled through policy checks before user logic runs. It ships three `DEFAULT_POLICIES`: `read_only` (blocks `file_write`, `database_write`, `send_email`), `no_pii` (blocks patterns like `ssn`, `credit_card`, `password`), and `strict` (requires approval for `send_email`, `file_write`, `code_execution`). At construction it computes `_globally_protected_actions`, the union of every `require_approval` action across all loaded policies, closing an "empty-policies-bypass" where omitting a policy name could otherwise skip the approval gate for high-risk actions.

`execute()` performs, in order: policy checks (`_check_policies`, denying on blocked actions, blocked-pattern substring matches, and PII pattern matches via `CredentialRedactor.find_pii_matches`); global approval enforcement (`_enforce_global_approval`, run after per-policy checks so an attacker cannot bypass it via an unknown policy list); stripping of caller-supplied approval-flag keys from params (defending against confusable-key attacks including case variants and homoglyphs); and, if configured, intent-based authorization via an `IntentManager`. Policy violations and denied approvals return `success=False, signal="SIGKILL"` rather than raising; other signals include `"SIGTERM"`, `"ESCALATE"`, `"DEFER"`, `"DEFER_TIMEOUT"`. Caller-supplied "approved" flags in params never satisfy an approval gate; only a trusted intent manager returning `was_planned=True` does. Note this engine is allowlist-agnostic by default: an action not mentioned in any policy's block/approval lists is allowed through, a different default posture from `PolicyEvaluator`'s default-deny.

### BaseAgent and ToolUsingAgent

`agent_os/base_agent.py` defines its own `PolicyDecision` enum (`ALLOW`, `DENY`, `AUDIT`, `ESCALATE`, `DEFER`), distinct from the policies-engine `PolicyDecision`. `BaseAgent(ABC)` wraps an internal `StatelessKernel`, an audit log, and an escalation queue; `_enforce_policy()` maps decisions to `ExecutionResult` signals (`ESCALATE` queues an `EscalationRequest`; `DEFER` awaits a registered async callback with a timeout, else fails; `DENY` returns `SIGKILL`). `ToolUsingAgent(BaseAgent)` adds tool-scoped execution. A separate `agent_os.integrations.base.BaseIntegration`/`GovernancePolicy` pair (per `ARCHITECTURE.md`'s Mermaid diagrams) governs the framework adapters, running `pre_execute` → policy `validate()` → `execute()` → `post_execute()`, with `GovernancePolicy` fields `max_tokens_per_request`, `max_tool_calls_per_request`, `blocked_patterns`, `allowed_tools`, `confidence_threshold`.

### CLI and security-hardening features

The `agentos` CLI (`src/agent_os/cli/__init__.py`) provides subcommands `init` (scaffold `.agents/`, `--template strict|permissive|audit`), `secure`, `audit` (`--format json`), `status`, `check` (`--staged`, `--ci`), `review` (`--cmvk`, `--models`), `install-hooks`, `validate` (`--strict`), `policy` (sub-subcommands `validate`/`test`/`diff`), `serve` (HTTP API: `GET /health`, `/status`, `/agents`, `POST /agents/{id}/execute`), `health`, and `metrics` (Prometheus text format).

README-documented hardening features, verified as named modules: tool content hashing via `agent_control_plane.tool_registry.ToolRegistry` (SHA-256 at registration, `verify_tool_integrity()` for tampering detection) and `agent_os.integrations.base.ContentHashInterceptor`; `PolicyEngine.freeze()` in the external `agent_control_plane` package, making the engine immutable post-init (`state_permissions` becomes a `MappingProxyType`); and `EscalationHandler` with `QuorumConfig` for M-of-N approval quorum plus approval-fatigue auto-deny. These target a specific threat model (sandbox escape via tool aliasing, runtime self-modification, approval fatigue) referenced against an external write-up on Claude Code denylist/sandbox escapes.

### Maturity and caveats

Framework adapters (LangChain, OpenAI Assistants, AutoGen, Semantic Kernel, CrewAI, OpenAI Agents SDK) live in `agent_os/integrations/` and use "lazy interception" so the target framework need not be installed until `.wrap()` is called. The README's self-reported "Status & Maturity" table lists `StatelessKernel`, the Policy Engine, Flight Recorder, CLI, framework adapters, and several `modules/` packages (`agent-primitives`, `cmvk`, `emk`, `amb-core`, `inter-agent-trust-protocol`, `agent-tool-registry`, `agent-control-plane`) as "Production-Ready / tested," while `agent-os-observability`, `mcp-kernel-server`, the GitHub CLI extension, and the Control Plane MCP/A2A adapters (placeholders returning canned responses or accepting all negotiation params) are "Experimental." The `nexus` trust-exchange module is a "Research prototype" with no `pyproject.toml`, no tests, and placeholder (non-secure XOR) cryptography. Marketing-style README claims ("2,573+ Tests Passing," "12 Framework Integrations," "<0.1ms p99 Governance Latency") are unverified.

---

## 5. Agent Mesh

### Package identity and status

Agent Mesh lives at `agent-governance-python/agent-mesh` and ships as the Python package `agentmesh_platform`, currently at **version 5.0.0**. `pyproject.toml` marks it explicitly as **"Deprecated. Previously published as agentmesh-platform. Install agent-governance-toolkit-core instead,"** with its runtime dependencies reduced to a single redirect: `agent-governance-toolkit-core>=4.1.0,<6.0`. `src/agentmesh/__init__.py` raises a `DeprecationWarning` on import pointing to `docs/package-consolidation/MIGRATION.md`. Despite the deprecation, the full `src/agentmesh` source tree remains present and actively tested (over 150 modules, roughly 9,300 lines across `identity/` and `trust/` alone). It requires Python `>=3.11` and notably depends on `agent_hypervisor>=3.7.0,<6.0` and `agentrust-trace>=0.2.0,<0.3.0` (TRACE Trust Record emission per ADR-0032).

The README still markets the package as **"AgentMesh, Public Preview," "SSL for AI Agents,"** part of an ecosystem alongside Agent OS, Agent Runtime, and Agent SRE (see sections 4, 6, 7), claiming 1,669+ passing tests and "<1ms p99" for its Full Governance Pipeline; these claims have not been reconciled with the package-level deprecation notice and should be treated as legacy messaging rather than verified figures.

`src/agentmesh/` is organized into `identity/`, `trust/`, `reward/`, `governance/` (about 40 files covering policy, compliance, EU AI Act, audit, Cedar/OPA adapters, federation), `registry/` (HTTP registry service), `relay/` (WebSocket relay service), `encryption/` (Signal-Protocol-style X3DH and Double Ratchet), `integrations/`, `transport/`, `server/`, `storage/`, `cli/`, `dashboard/`, `marketplace/`, and related support modules. The package `__init__.py` exposes a 4-layer public API: Layer 1 Identity (`AgentIdentity`, `AgentDID`, `Credential`, `CredentialManager`, `ScopeChain`, `DelegationLink`, `HumanSponsor`, `RiskScorer`, `SPIFFEIdentity`, `SVID`); Layer 2 Trust (`TrustBridge`, `ProtocolBridge`, `TrustHandshake`, `HandshakeResult`, `CapabilityScope`, `CapabilityGrant`, `CapabilityRegistry`); Layer 3 Governance (`PolicyEngine`, `ComplianceEngine`, `AuditLog`, `AuditChain`, `ShadowMode`); Layer 4 Reward (`RewardEngine`, `TrustScore`, `RewardDimension`, `RewardSignal`); plus a unified `AgentMeshClient`/`GovernanceResult`. An exception hierarchy rooted at `AgentMeshError` covers identity, attestation, trust (including handshake timeouts), delegation-depth, governance, storage, and marketplace failures.

### Identity (DID) and credentials

`identity/agent_id.py` defines `AgentDID` (pydantic), format **`did:mesh:<unique-id>`**, generated via `AgentDID.generate(name, org=None)` using `secrets.token_hex(16)` (128 bits) rather than a hash of attacker-knowable name/org data. `AgentIdentity` carries `did`, `public_key` (Ed25519, base64), `sponsor_email`/`sponsor_verified`, `capabilities: list[str]`, `status` (`active`/`suspended`/`revoked`), `parent_did`, `delegation_depth`, and `max_initial_trust_score` (a lineage-bound trust cap tied to "Invariant 6, Sybil resistance"). `AgentIdentity.create(...)` generates an Ed25519 keypair and derives `verification_key_id` as `f"key-{sha256(pubkey)[:16]}"`. `sign()`/`verify_signature()` perform Ed25519 sign/verify, with verification failures logged at `DEBUG` to avoid log-flooding from attacker-controlled bad signatures. `has_capability()` supports exact match, the wildcard `"*"`, and suffix-wildcard prefix matching (`"read:*"` matches `"read:data"`). `to_did_document()` exports a W3C DID Document with an `Ed25519VerificationKey2020` verification method. `IdentityRegistry` is an in-memory registry supporting `register()`, `revoke()` (cascades to children), and `list_active()`.

`identity/credentials.py` models short-lived bearer tokens via `Credential`: `token` (`secrets.token_urlsafe(32)`), `token_hash` (SHA-256, verified with `hmac.compare_digest`), default TTL **900 seconds**, status lifecycle `active`/`rotated`/`revoked`/`expired`, and `rotate()`, which chains a linked successor credential. `CredentialManager` adds `REVOCATION_PROPAGATION_TARGET = 5` seconds, a target rather than a benchmarked figure.

### Delegation chains

`identity/delegation.py` implements scoped delegation. `DelegationLink` represents one hop (`parent_did`, `child_did`, `parent_capabilities`, `delegated_capabilities`, `link_hash`, `previous_link_hash`) and enforces capability narrowing via `verify_capability_narrowing()`. `ScopeChain` caps depth at `DEFAULT_DELEGATION_MAX_DEPTH = 5`, validates chain linkage and narrowing on `add_link()`, and exposes `verify()` for full-chain re-validation (depth ordering, hash-chain integrity, escalation detection); signature verification against `known_identities` is documented as **best-effort/compatibility-mode, not a security guarantee**. On `AgentIdentity` itself, `MAX_DELEGATION_DEPTH = 10`; `delegate()` enforces the depth limit, rejects wildcard `"*"` propagation, and requires delegated capabilities to be a subset of the parent's. `verify_delegation_chain()` (static) walks `parent_did` links checking existence, capability narrowing, depth consistency, and cycle detection.

### Trust scoring and handshake protocol

Trust scores use a **0-1000** domain (`TRUST_SCORE_DEFAULT = 500`, `TRUST_REVOCATION_THRESHOLD = 300`). `trust/levels.py` provides the canonical `trust_level_for_score()` mapping used across the trust-engine HTTP API and CLI: `>=900` verified_partner, `>=700` trusted, `>=500` standard, `>=300` probationary, else untrusted.

`trust/handshake.py` implements an IATP-style Ed25519 challenge/response protocol. `TrustHandshake.initiate(peer_did, protocol="iatp", required_trust_score=700, ...)` issues a `HandshakeChallenge` (nonce, optional RFC 9334 freshness nonce, 30s expiry), and `_verify_response` performs an eight-step check: challenge ID match, expiry, DID-substitution binding, registry lookup and active-status check, `registry.is_trusted()`, Ed25519 signature verification, public-key match, and **registry-authoritative** trust score and capabilities (self-reported values in `HandshakeResponse` are never trusted for the final decision). `MAX_HANDSHAKE_MS = 200`; pending challenges are capped at 1,000 to mitigate DoS. Without an `IdentityRegistry`, all peers are rejected. `_do_initiate` is documented as a same-process simulation harness, signing on behalf of the peer using keys held in the registry, rather than a real network round trip; actual wire transport lives in `transport/` and the registry/relay services below.

`trust/bridge.py` provides `TrustBridge` (peer trust cache keyed by DID, `default_trust_threshold=700`, in-process HMAC-signed `PeerInfo` records explicitly noted as not a real security boundary) and `ProtocolBridge`, which translates messages between A2A, MCP, IATP, and ACP, verifying trust before translation and raising `PermissionError` otherwise. `add_verification_footer()` appends the markdown "Verified by AgentMesh (Trust Score: N/1000)" block behind the README's "Verification Footers" feature. Concrete adapters `A2AAdapter` and `MCPAdapter` exist; **no dedicated `ACPAdapter`** exists yet, so ACP messages route through the generic passthrough path.

`trust/cards.py` defines `TrustedAgentCard` (name, capabilities, `agent_did`, `public_key`, a trust score on a separate **0.0-1.0 scale**, `card_signature`), modeled on A2A's agent-card discovery pattern; `sign(identity)` binds the card to a live cryptographic identity. `trust/endorsement.py` implements an RFC 9334-flavored vouching model (`EndorsementType`: CAPABILITY, INTEGRITY, COMPLIANCE, IDENTITY, REFERENCE_VALUE) whose `EndorsementRegistry` explicitly does **not** perform cryptographic signature verification, leaving that to a caller.

### Reward / trust-scoring engine

`reward/` provides `RewardEngine`, `TrustScore`/`RewardDimension`/`RewardSignal` scoring, `NetworkTrustEngine`/`TrustEvent` (`trust_decay.py`), and a distribution subsystem with strategies `EqualSplitStrategy`, `TrustWeightedStrategy`, `ContributionWeightedStrategy`, and `HierarchicalStrategy` behind `RewardDistributor`. Dimension weights (summing to 1.0): policy compliance 0.25, resource efficiency 0.15, output quality 0.20, security posture 0.25, collaboration health 0.15. The Dify integration notes flag that trust-score time decay is **not yet implemented there**, even though `trust_decay.py` exists at the core engine level.

### Discovery, routing, and wire protocol

Discovery and routing are implemented as two FastAPI/WebSocket services built against a shared spec, `docs/specs/AGENTMESH-WIRE-1.0.md`, covering Agent Identity, Cryptographic Primitives, Key Management, X3DH Key Agreement, Double Ratchet, Message Envelope, KNOCK Intent Protocol, Registry API, Relay Service, Authentication (Ed25519-Timestamp default, SPIFFE/SVID enterprise), Governance Integration, Protocol Versioning, Security Considerations, and Test Vectors. The encryption layer (`encryption/x3dh.py`, `ratchet.py`, `channel.py`, `bridge.py`) is a from-scratch Signal-Protocol-style implementation (X3DH plus Double Ratchet), not a wrapper around an existing library.

The **registry service** (`registry/app.py`) exposes `POST /v1/agents` (registration with an Ed25519 proof over `public_key || proof_timestamp`, 5-minute replay window), `GET`/`DELETE /v1/agents/{did}`, pre-key bundle endpoints feeding X3DH, presence/heartbeat/reputation endpoints, and **`GET /v1/discover?capability=<str>&limit=<1-200, default 50>`** for capability-based discovery, returning `{results: [{did, capabilities, reputation_score, last_seen}], total}`. The **relay service** (`relay/app.py`) is a WebSocket store-and-forward message relay (`HEARTBEAT_INTERVAL = 30`s, `OFFLINE_THRESHOLD = 90`s); `_verify_connect_pop()` enforces DID proof-of-possession on connect (the DID must equal `did:mesh:` plus `sha256(public_key)`), disableable only via a dev-only environment variable with a loud warning.

Both services optionally require **Entra-tier verification**: when `AGENTMESH_ENTRA_AUDIENCE` and `AGENTMESH_ENTRA_TENANT_ID` are set, Entra-signed JWTs become mandatory, with a strict RS256/RS384/RS512 algorithm allowlist (rejecting `none` and HS*). The fail-closed contract returns HTTP 401 (registry) or WebSocket close code 4003 (relay) for invalid tokens; previously verified peers retain their tier on transient re-verify failures to prevent downgrade attacks. A separate in-process `AgentRegistry` (`services/registry/agent_registry.py`) provides an async, lock-guarded "Yellow Pages" registry distinct from the HTTP service.

### CLI and protocol adapters

`cli/main.py` provides a click-based CLI (`init`, `register`, `status`, `policy --validate`, `audit --agent --limit --format`, `init-integration --claude`), matching the README's documented commands, though its version string (`3.1.0`) is unsynced with the package's `__version__` (`5.0.0`). A separate `cli/proxy.py` implements the "SSL for AI Agents" MCP governance proxy. Adapters exist for A2A, AI Card, CrewAI, Django middleware, Flowise, Haystack, LangChain, LangFlow, LangGraph, MCP, and OpenAI Swarm; per the README's protocol support table, AI Card, A2A, MCP, and IATP are "Alpha" while ACP is "Planned."

### Documented known limitations

Per the README: no dedicated ACP adapter; `services/audit/` and `services/reward_engine/` are TODO wrapper stubs over complete core modules; `services/mesh-control-plane/` is an unimplemented placeholder; scope-chain verification in the separate `langchain-agentmesh` package is simulated; the Dify integration lacks `X-Agent-Signature` verification, trust-score decay, and persistent audit logs; Redis/PostgreSQL storage providers require real infrastructure for integration testing; the Kubernetes `GovernedAgent` CRD has no reconciling controller; SPIRE integration for the SPIFFE identity module is stubbed; and performance targets (under 5ms latency overhead, 10k registrations/sec) are design targets, not benchmarks.

---

## 6. Agent Runtime and Agent Sandbox

Source root: `agent-governance-python/agent-runtime` and `agent-governance-python/agent-sandbox`. Both packages are **Public Preview**, version `5.0.0`, and **deprecated thin wrappers** rather than the primary implementation surface. Both require Python `>=3.11` and build with `hatchling`.

`agent-runtime` (PyPI name `agentmesh_runtime`) declares itself deprecated: "Previously published as agentmesh-runtime. Install agent-governance-toolkit-core instead." Its `__init__.py` fires a `DeprecationWarning` on import and re-exports the entire public API from a sibling package, `hypervisor` (`agent-governance-python/agent-hypervisor/src/hypervisor`), the "canonical upstream implementation." It implements almost nothing beyond `deploy.py` and the re-export list; the README notes it "was renamed from `agent-runtime` to `agentmesh-runtime` to avoid a PyPI name collision with the AutoGen team's package."

`agent-sandbox` (PyPI name `agt_sandbox`) also fires a `DeprecationWarning` ("Use agent-governance-toolkit-cli instead") but, unlike `agent-runtime`, contains substantial first-party implementation: five sandbox providers, hardening primitives, and a code scanner. Its deprecation signals a planned consolidation, not code already elsewhere.

### Privilege rings

`agent-runtime`'s ring API (`RingEnforcer`, `ActionClassifier`, `RingElevationManager`, `RingElevation`, `ElevationDenialReason`, `RingBreachDetector`, `BreachSeverity`) re-exports `hypervisor.rings.*`. `ExecutionRing(int, Enum)` (`hypervisor/models.py`) uses a lower number for more privilege:

| Ring | Name | Semantics |
|---|---|---|
| 0 | `RING_0_ROOT` | Hypervisor config and penalty; requires SRE Witness |
| 1 | `RING_1_PRIVILEGED` | Non-reversible actions; requires eff_score > 0.95 + consensus |
| 2 | `RING_2_STANDARD` | Reversible actions; requires eff_score > 0.60 |
| 3 | `RING_3_SANDBOX` | Read-only/research; default for unknown agents |

`ExecutionRing.from_eff_score(eff_score, has_consensus)` derives a ring from trust score. `RingEnforcer` maps rings to `ResourceConstraints`: Rings 0/1 allow network, full filesystem write, and subprocess (32/16 max concurrent tools); Ring 2 scopes to allowlisted network, scoped write, 8 tools; Ring 3 denies all three (2 tools max). `RingEnforcer.check(agent_ring, action, eff_score, has_consensus, has_sre_witness)` always denies Ring 0 to agents; otherwise it requires `agent_ring.value <= required.value`, and `get_constraints` fails closed to `RING_3_SANDBOX` for unknown rings. `RingBreachDetector`/`BreachSeverity` score violation attempts and back a circuit breaker shared by sandbox providers and the kill switch.

### Command denylist

`RingEnforcer.check_command(command)` extracts the base token, strips trailing shell metacharacters (`;`, `&`, `|`), matches case-insensitively, and rejects empty/`None` commands. `DENIED_COMMANDS` (23 entries, `hypervisor/sandbox/__init__.py`, shared by the Dockerfile build, smoke tests, and `RingEnforcer`) covers network fetch tools (`curl`, `wget`, `ftp`, `telnet`), raw socket/proxy tools (`nc`, `ncat`, `netcat`, `socat`, `nmap`, `tcpdump`), alternative interpreters (`perl`, `ruby`, `python2`), the compiler toolchain (`gcc`, `g++`, `make`, `cc`), and shells (`bash`, `sh`, `dash`, `zsh`, `ksh`, `fish`), with `MINIMAL_SANDBOX_PATH`/`ALLOWED_BINARIES = ("python3", "python")` as fallback, plus a second, complementary denylist at the container-image level (see below).

A separate, static AST-based scanner (`src/agent_sandbox/code_scanner.py`) is distinct from the command denylist: `scan_code_for_subprocesses(code)` walks the Python AST for process-spawning APIs (`subprocess.*`, `os.exec*`/`spawn*`/`popen`/`system`, `pty.spawn`, `shutil.which`), and `enforce_no_subprocess_execution(code)` raises `SandboxCodeViolation` on violations. Its docstring calls this "an intentionally lightweight guardrail... it does not replace runtime process monitoring, seccomp, or eBPF tracing."

### Saga orchestration and execution-plan validation

Re-exported from `hypervisor.saga.*`: `SagaOrchestrator`, `SagaTimeoutError`, `SagaState`, `StepState`, `FanOutOrchestrator`, `FanOutPolicy`, `CheckpointManager`, `SemanticCheckpoint`, `SagaDSLParser`, `SagaDefinition`. `SagaOrchestrator` ("Public Preview, basic implementation") performs sequential step execution with reverse-order compensation on failure. `add_step(saga_id, action_id, agent_did, execute_api, undo_api=None, timeout_seconds, max_retries=0)` and `async execute_step(saga_id, step_id, executor)` run a step with timeout/retry; the docstring notes `asyncio.wait_for` can only cancel at `await` points, so a synchronous CPU-bound function wrapped in `async def` is not preemptible. On failure the orchestrator calls `Undo_API` in reverse order; a failed `Undo_API` ties into a Joint Liability penalty.

Execution-plan validation is implemented by the Saga DSL layer (`hypervisor/saga/dsl.py`, `hypervisor/saga/schema.py`), validating a declarative plan (steps with dependencies, agents, undo endpoints) before converting it to runnable `SagaStep` objects. `SagaDSLParser.parse()` raises `SagaDSLError` for missing `name`/`session_id`, empty steps, a step missing `id`/`action_id`/`agent`, or a duplicate step `id`, and is explicitly a stub for fan-out ("Fan-out groups in DSL are ignored, sequential execution only"): `_parse_fan_out` always returns `FanOutPolicy.ALL_MUST_SUCCEED` regardless of the DSL's stated policy.

`SagaSchemaValidator` runs JSON Schema validation first, then semantic checks: duplicate step IDs; `action_id` must start with a `VALID_ACTION_PREFIXES` entry (`model.`, `data.`, `deploy.`, `validate.`, `notify.`, `infra.`, `security.`, `monitor.`, `config.`, `test.`); every step must declare `undo_api` ("no step without compensation"); `depends_on` references must resolve; and circular dependencies are detected via DFS. `FanOutOrchestrator`/`FanOutPolicy` is likewise "basic implementation": only `ALL_MUST_SUCCEED` is enforced; `MAJORITY_MUST_SUCCEED` and `ANY_MUST_SUCCEED` are aspirational, type-surface only.

### Termination control (kill switch)

`KillSwitch`/`KillResult` (`hypervisor/security/kill_switch.py`); `KillReason`: `BEHAVIORAL_DRIFT`, `RATE_LIMIT`, `RING_BREACH`, `MANUAL`, `QUARANTINE_TIMEOUT`, `SESSION_TIMEOUT`; `HandoffStatus`: `PENDING`, `HANDED_OFF`, `FAILED`, `COMPENSATED`.

`KillSwitch(callback_timeout=5.0)` tracks per-agent termination callbacks and standby substitutes per session. `kill(agent_did, session_id, reason, in_flight_steps=None)` looks up a substitute; each in-flight step is marked `HANDED_OFF` if a substitute exists, else `COMPENSATED` (expecting saga compensation to rerun it), with the callback run outside the lock under the configured timeout. A documented invariant: the agent is unregistered unconditionally at the end of `kill()` regardless of success, since leaving a failed termination registered "would falsely advertise the agent as live and re-callable"; a caller seeing `terminated=False` must explicitly re-register before retrying. `kill()` returns a `KillResult` (`handoffs`, `handoff_success_count`, `compensation_triggered`, `terminated`); a `threading.RLock` lets a re-entrant callback avoid deadlock. Blast-radius containment combines this with the ring-breach circuit breaker: Docker and Hyperlight check `is_breaker_tripped` before subprocess execution.

### Agent Runtime's own code: deploy.py

The only non-re-exported implementation in `agent-runtime` is `src/agent_runtime/deploy.py`, providing `DockerDeployer` and `KubernetesDeployer` targets. `_validate_agent_id` enforces `^[a-zA-Z0-9][a-zA-Z0-9_-]{0,62}$` against unsafe interpolation into Docker/kubectl command lines; `GovernanceConfig` (`policy_path`, `trust_level`, `audit_enabled`, `max_tool_calls`, `rate_limit_rpm`, `kill_switch_enabled`, `retention_days`) is injected as `AGT_*` env vars. Both deployers apply equivalent hardening: `--cap-drop ALL`/`--security-opt no-new-privileges`/`--read-only` for Docker, `runAsNonRoot`/`seccompProfile RuntimeDefault`/dropped capabilities/read-only root filesystem for Kubernetes.

### SandboxProvider ABC and hardening primitives

`src/agent_sandbox/sandbox_provider.py` defines the backend-agnostic contract for all five providers. `SandboxConfig` carries `timeout_seconds=60.0`, `memory_mb=512`, `cpu_limit=1.0`, `network_enabled=False`, `read_only_fs=True`, `output_max_bytes=1_048_576`, and `ring: Any = None` (loosely typed to avoid a hard `hypervisor` dependency; when set, each provider applies `ResourceConstraints` and gates subprocess execution). `SandboxProvider(ABC)` requires `create_session`, `execute_code`, `destroy_session`, `is_available`; `run(...)` defaults to `NotImplementedError`. Two dependency-free leaf modules are reused by every backend: `_hardening.py` (`BLOCKED_ENV_VARS`, 16 entries including `LD_PRELOAD`, `BASH_ENV`, `PYTHONPATH`, `NODE_OPTIONS`; `PROTECTED_PATHS_UNIX`/Windows equivalents) and `isolation_runtime.py` (`IsolationRuntime`: `RUNC`, `GVISOR`/`runsc`, `KATA`, `AUTO`).

### Sandbox providers (five backends)

Ring enforcement is wired into three of five (Docker, Hyperlight, ACA).

- **`DockerSandboxProvider`** (`agt-sandbox[docker]`): prefers a hardened image tag, falling back to `python:3.11-slim` unless `require_hardened_image=True`. Applies `RingEnforcer`/`RingBreachDetector` when `cfg.ring` is set (soft-skipped if `hypervisor` missing); `execute_code` runs the policy gate, AST subprocess scanner, and ring subprocess gate first. Container hardening: swap disabled, `network_disabled`, `read_only`, `security_opt=["no-new-privileges", "seccomp=default", "apparmor=docker-default"]`, `cap_drop=["ALL"]`, `user="65534:65534"`, `pids_limit=128`.
- **`HyperLightSandboxProvider`** (`agt-sandbox[hyperlight]`): backed by the upstream `hyperlight-sandbox` CNCF project, one micro-VM per session on KVM/mshv/WHP; `cpu_limit` is dropped since Hyperlight pins one vCPU per micro-VM. Supports `snapshot_session`/`restore_snapshot`; ring wiring mirrors Docker.
- **`ACASandboxProvider`** (`agt-sandbox[azure]`, plus an early-access wheel outside PyPI): wraps Azure Container Apps' managed sandbox. `_apply_egress_policy` calls `set_egress_policy` when `network_default == "deny"` (fail-closed, even with empty hosts); `"allow"` leaves Azure's default-allow behavior standing, suitable only for trusted dev/research workloads.
- **`MxcSandboxProvider`** (no extra dependency group; native `mxc-exec` via bubblewrap/AppContainer/Seatbelt): `_reassert_security_keys` re-pins security-critical config keys after merging operator overrides, so a fragment can never silently widen egress or filesystem access. No tool-cap concept in the schema, so a non-empty `tool_allowlist` refuses to start; no ring wiring.
- **`NonoSandboxProvider`** (`agt-sandbox[nono]`, PyPI Alpha; Linux/macOS only): kernel-enforced isolation via Landlock (Linux 5.13+) or Seatbelt. `_check_egress` raises `ValueError` if `allow_outbound=True` with empty `allowed_hosts` and `allow_unrestricted_egress` not explicitly `True`, the strictest egress default of the five; non-empty `tool_allowlist` is likewise refused; no ring wiring.

### Hardened Docker image and denial-logging shim

`agent-sandbox/docker/Dockerfile.sandbox` builds from a digest-pinned `python:3.11-slim` with three layers: PATH pinning with no inherited `:$PATH`, symlinking only an allowlisted set (default `python3 cat echo ls sleep`); execute-bit stripping from a larger CLI list found on PATH; and a logging denial shim (`docker/agt-deny-shim.py`) symlinking a configurable subset of stripped binaries to a pure-stdlib Python script rather than merely removing execute permission. The shim logs a structured JSON record to stderr and optionally `$AGT_DENIED_LOG` (`O_NOFOLLOW`, mode `0o600`), exiting `126` rather than `127`, a documented, intentional behavior change.

Test coverage in `agent-sandbox` is led by `test_docker_sandbox.py` (190 methods), `test_azure_sandbox.py` (85), `test_hyperlight_sandbox.py` (56), and `test_nono_sandbox.py` (54), plus a 13-method `test_ring_enforcement_wiring.py` using fake ring stand-ins so it runs without `agent-hypervisor` installed. In `agent-runtime`, `test_deploy.py` has 35 tests and `test_runtime_imports.py` smoke-tests a 62-item `ALL_EXPORTS` list.

---

## 7. Agent SRE

### Package identity and status

`agent-sre` (Python, `agent-governance-python/agent-sre`) is deprecated at the packaging level while its source tree remains fully implemented and tested. Per `pyproject.toml`: `name = "agent_sre"`, `version = "5.0.0"`, description "Deprecated. Previously published as agent-sre. Install agent-governance-toolkit-cli instead," `dependencies = ["agent-governance-toolkit-cli>=4.1.0,<6.0"]`. The published wheel is a dependency-only redirect stub that does not ship `src/agent_sre` as an importable library from PyPI; `src/agent_sre/__init__.py` (the real source, still exercised in CI) issues a `DeprecationWarning` on import pointing to `docs/package-consolidation/MIGRATION.md`. No `[project.scripts]` entry point is declared, so no `agent-sre` shell command is installed by this package.

Despite the deprecation stub, `src/agent_sre` is a large reliability engine: 67 test files, 1,479 `def test_*` functions (the toolkit README's "1,257+ Tests Passing" figure is a toolkit-wide/older count, not reconciled with this total). Modules cover cost governance (`cost/`), SLOs (`slo/`), chaos engineering (`chaos/`), cascading-failure protection (`cascade/`), incident response (`incidents/`), progressive delivery (`delivery/`), anomaly detection (`anomaly/`), and integrations spanning Agent OS, Agent Mesh, Datadog, LangChain, Langfuse, LangSmith, Arize, Braintrust, Prometheus, Sentry, OTel, MCP, plus framework adapters (LangGraph, CrewAI, AutoGen, OpenAI Agents, Semantic Kernel, Dify).

### CLI entry points

The CLI (`src/agent_sre/cli/main.py`, an `argparse.ArgumentParser` named `"agent-sre"`, invoked via `python -m agent_sre`) is minimal and largely a status shim rather than an operational tool: `slo status`/`slo list` print static "no data" strings regardless of actual state (no persistence wired in); `cost summary` prints "No cost data available. Use the Python API to record costs."; `version` prints `"agent-sre 0.1.0"` (hardcoded, inconsistent with the `pyproject.toml` version `5.0.0`); `info` prints a JSON blob listing `engines`, `integrations`, `adapters`.

Real operational surfaces exist elsewhere: a FastAPI HTTP server (`src/agent_sre/api/server.py`, run via `uvicorn agent_sre.api.server:app`) exposing `GET /health`, `GET /api/v1/stats`, `GET /metrics` (Prometheus), and routes for SLOs, cost, chaos, incidents, and delivery under `/api/v1/`; and an MCP server (`AgentSREServer`) exposing tools `sre_check_slo`, `sre_report_cost`, `sre_request_budget`, `sre_check_rollout_status`, `sre_list_slos`.

### Kill switch (`cost/guard.py`)

The kill switch is a threshold-triggered state inside `CostGuard` (477 lines, marked "Public Preview, basic implementation"), not a standalone module. `CostGuard.__init__` accepts `per_task_limit=2.0`, `per_agent_daily_limit=100.0`, `org_monthly_budget=5000.0`, `auto_throttle=True`, `kill_switch_threshold=0.95`, `alert_thresholds=[0.50, 0.75, 0.90, 0.95]`; numeric args are validated for finiteness/range at construction.

Mechanics (in `_record_cost_locked`, guarded by `threading.Lock`): if `auto_throttle` and per-agent utilization reaches `kill_switch_threshold` (95% of daily limit), `budget.killed = True` fires a `CRITICAL`/`KILL` alert; a softer per-agent throttle trips at 0.85 (hardcoded) setting `budget.throttled = True`. An org-wide kill switch fires when org utilization crosses `kill_switch_threshold` against `org_monthly_budget`, setting `self._org_killed = True` and cascading `killed = True` to every existing `AgentBudget`. Once killed, `_check_locked()` short-circuits with `"Organization budget exhausted"` or `"Agent killed — budget exhausted"`. Recovery is via `reset_daily(agent_id=None)`, which clears per-agent state; there is no separate un-kill API, and it does not clear `self._org_killed`, so an org-level kill is sticky across daily resets.

Enforcement primitives: `check_task()` (advisory-only, does not reserve budget, so concurrent callers can both overshoot); `check_and_charge()` (recommended atomic check-and-charge under one lock); `record_cost()` (unconditional, unsafe for enforcement). `_check_anomaly_locked` applies Z-score detection over a `deque(maxlen=1000)` cost history (needs 10+ samples, flags `z_score > 2.0`). The README additionally claims "Z-score, IQR, and EWMA methods"; IQR/EWMA are not present in `guard.py`, and if implemented would live in the separate, unverified `cost/anomaly.py` / `cost/optimizer.py`. Test coverage: `test_cost.py` and `test_cost_hardened.py` (51 combined test functions), including `test_kill_switch` and `test_agent_killed_and_org_exceeded`.

### SLO monitoring and error budgets (`slo/`)

**Indicators** (`slo/indicators.py`): abstract `SLI` base with `collect()`, `record()`, `compliance()`. Seven concrete subclasses match the "7 indicator types" benchmark: `TaskSuccessRate`, `ToolCallAccuracy`, `ResponseLatency`, `CostPerTask`, `PolicyCompliance`, `DelegationChainDepth`, `HallucinationRate`. An eighth, `CalibrationDeltaSLI`, also exists and is exported but is not counted in the marketed "7 SLI Types" figure. Separately, `docs/slo-reference.md` documents `ToolCallSuccess` and `UserSatisfaction` SLI classes that do not exist in the code (closest real class is `ToolCallAccuracy`; no user-satisfaction indicator exists anywhere), a verified doc/code drift.

**Error budget engine** (`slo/objectives.py`): `ExhaustionAction` (`ALERT`, `FREEZE_DEPLOYMENTS`, `CIRCUIT_BREAK`, `THROTTLE`); `ErrorBudget` dataclass with `total` (1 minus SLO target), `consumed`, `window_seconds=2592000` (30 days), `burn_rate_alert=2.0`, `burn_rate_critical=10.0`, events in a bounded `deque(maxlen=max_events)` (default 100,000; oldest silently evicted). `burn_rate()` computes `actual_error_rate / allowed_error_rate` over a default 1-hour window; `alerts()` returns fixed warning/critical `BurnRateAlert`s over a 24-hour window. `SLO` auto-derives `error_budget` from indicator targets if not given, evaluates status (`HEALTHY`, `WARNING`, `CRITICAL`, `EXHAUSTED`, `UNKNOWN`), and fires alerts through an attached `AlertManager` only when severity worsens or recovers to healthy.

**SLO-as-code** (`slo/spec.py`, Pydantic v2): `SLOSpec(name, service, sli, target=99.0, window="30d", error_budget_policy, inherits_from=None)` with `.from_yaml()`/`.to_yaml()`; `load_slo_specs(directory)` globs YAML files; `resolve_inheritance(specs)` performs cycle-safe recursive parent-merge (e.g. `batch_agent.yaml` inherits `base.yaml`'s 99%/30d target, relaxed to 95%/7d). Persistence (`slo/persistence.py`) offers `InMemoryMeasurementStore` and `SQLiteMeasurementStore`; `slo/dashboard.py`'s `SLODashboard` adds `register_slo`, `take_snapshot`, `compliance_report`, `health_summary`.

### Chaos testing (`chaos/`)

**Core engine** (`chaos/engine.py`): `FaultType` has 12 members across three families: infra (`LATENCY_INJECTION`, `ERROR_INJECTION`, `TIMEOUT_INJECTION`), adversarial (`PROMPT_INJECTION`, `POLICY_BYPASS`, `PRIVILEGE_ESCALATION`, `DATA_EXFILTRATION`, `TOOL_ABUSE`, `IDENTITY_SPOOFING`), behavioral (`DEADLOCK_INJECTION`, `CONTRADICTORY_INSTRUCTION`, `TRUST_PERTURBATION`). `Fault` provides 21 named static factories, since several share a `FaultType` with different target semantics. `ChaosExperiment(name, target_agent, faults, duration_seconds=1800, abort_conditions, blast_radius=1.0)` has lifecycle `start()`, `inject_fault()`, `check_abort()`, `complete()`. `calculate_resilience()` is documented in-code as "simple pass/fail based on success rate": `passed = experiment_success_rate >= baseline_success_rate * 0.9`; its `recovery_time_ms` and `cost_increase_percent` parameters are accepted but unused, a simplified/preview scoring model.

`chaos/library.py`'s `ChaosLibrary` ships built-in templates (`timeout-injection`, `error-injection`, `latency-injection`, `adversarial-injection`, `adversarial-escalation`, `adversarial-exfiltration`, `deadlock-injection`, `contradictory-instruction`, `trust-perturbation`, `delegation-reject`, `credential-expiry`); the verified count is 10-11, while the README's "By The Numbers" table and quickstart both say "9 Chaos Fault Templates," a marketing/code count mismatch. Adversarial sub-engines add `AdversarialRunner` and `AdversarialEvaluator`, each with a builtin playbook/vector library, and scheduling (`chaos/scheduler.py`, `chaos/chaos_scheduler.py`) adds blackout windows and progressive/ramping severity. Chaos experiments are reachable via the API server and covered by `tests/unit/test_chaos.py` and `test_adversarial_chaos.py`.

### Circuit breakers: two independent implementations

Two separate, non-shared `CircuitBreaker` classes share a name but differ in module, semantics, and enum casing (a verified structural duplication, not a re-export). Both are covered by `tests/unit/test_circuit_breaker.py`.

**1. `agent_sre.cascade.circuit_breaker`** (266 lines): docstring states it preserves "the legacy `agent_os.circuit_breaker` API." `CircuitState` uses uppercase values (`CLOSED`, `OPEN`, `HALF_OPEN`); `CircuitBreakerConfig` fields include `failure_threshold=5`, `recovery_timeout_seconds=30.0` (also accepts legacy alias `reset_timeout_seconds`), `half_open_max_calls=1`. This implementation has real automatic HALF_OPEN recovery: `get_state()` transitions `OPEN → HALF_OPEN` once the recovery timeout elapses; `call()` supports both sync and async callables, returning a `fallback` or raising `CircuitOpenError`. `CascadeDetector(agents, cascade_threshold=3)` maintains one breaker per agent and flags a cascade when enough agents are simultaneously `OPEN` (OWASP ASI08).

**2. `agent_sre.incidents.circuit_breaker`** (205 lines, "Public Preview, basic implementation"): docstring states explicitly "Half-open recovery is not available in Public Preview, use `force_close()` or `reset()` to recover." `CircuitState` uses lowercase values; `HALF_OPEN` is wired through the enum and config knobs (`success_threshold`, `half_open_max_calls`, both reserved/unused) but no code path ever transitions to it, so recovery is manual-only via `force_open()`/`force_close()`/`reset()`. `record_failure()` trips to `OPEN` at `failure_threshold`, and every transition is appended to an audit trail (`CircuitEvent` list), absent from the `cascade` implementation; `CircuitBreakerRegistry` provides `get()`, `is_available()`, and a `summary()` rollup.

In short, `cascade.CircuitBreaker` self-heals via timed half-open probing and supports sync/async wrapping plus cascade detection, while `incidents.CircuitBreaker` is a simpler closed/open breaker with an audit log that requires manual intervention to recover, despite config fields suggesting timer-based healing.

### Related incident and runbook machinery

`incidents/detector.py`'s `IncidentDetector` handles signal ingestion, correlation, deduplication, and pruning, with `Incident` lifecycle methods `acknowledge()`, `investigate()`, `mitigate()`, `resolve()`. `incidents/postmortem.py`'s `PostmortemGenerator.generate(incident)` synthesizes an automated postmortem draft from incident data (`Postmortem.to_markdown()`). `incidents/runbook.py`, `runbook_executor.py`, and `runbook_registry.py` provide `RunbookExecutor.execute()` (rollback and timeout handling) and `RunbookRegistry.match(incident)`, with runbooks loadable from YAML via `load_runbooks_from_yaml()`.

---

## 8. Agent Compliance

Agent Compliance is a Python package at `agent-governance-python/agent-compliance/` in the AGT monorepo, providing installer, verification, linting, integrity, and prompt-defense tooling that Agent OS and Agent Mesh (sections 4, 5) plug into for policy enforcement.

### Package identity

PyPI distribution `agent-governance-toolkit-compliance`. `pyproject.toml` declares version `5.0.0`, while `agent_compliance/__init__.py` reports `__version__ = "3.2.2"`, a packaging inconsistency rather than a behavioral bug; `pyproject.toml` requires Python `>=3.11`, though the README badge claims `python-3.9+`, another stated inconsistency. License MIT, marked "Public Preview." Hard dependencies: `pydantic>=2.4.0,<3.0`, `pyyaml>=6.0,<7.0`, `click>=8.0,<9.0`. Optional extras cover `core`, `integrations`, `cli`, `protocols`, per-framework adapters (`langchain`, `crewai`, `openai-agents`, `langgraph`, `llamaindex`, `haystack`, `pydantic-ai`, `adk`), legacy aliases, `opa` (empty placeholder), `cedar`, and `full`. The package has no required dependency on other AGT components; `agent_os`/`agentmesh` are imported opt-in. Console scripts include `agent-governance-toolkit`, `agent-compliance`, and `agt`.

Key modules under `src/agent_compliance/` (roughly 8,472 lines total): `verify.py` (OWASP ASI verification, runtime evidence), `integrity.py` (tamper detection), `lint_policy.py` (policy linter), `prompt_defense.py` (`PromptDefenseEvaluator`), `policy_test.py`, `promotion.py`, `supply_chain.py`, `cli/red_team.py`, `governance/attestation_validator.py`, `security/scanner.py`.

### OWASP verification

`verify.py`'s `GovernanceVerifier` checks for importable components mapped to an internal "OWASP ASI 2026" control set, `OWASP_ASI_CONTROLS` (`ASI-01`..`ASI-10`), e.g. ASI-01 "Prompt Injection" -> `agent_os.integrations.base.PolicyInterceptor` through ASI-10 "Behavioral Anomaly" -> `agentmesh.governance.compliance.ComplianceEngine`. This numbering does not match the taxonomy in `docs/OWASP-COMPLIANCE.md` (there, ASI-01 = "Agent Goal Hijack"): two independently maintained control taxonomies, both labeled "OWASP ASI 2026," coexist in the repo. Dynamic imports go through `_validate_module_name`, enforcing an `ALLOWED_MODULE_PREFIXES` frozenset (`agent_os.`, `agentmesh.`, `agent_compliance.`, `agent_sre.`, `agent_hypervisor.`, `hypervisor.`, `agent_runtime.`, `agent_lightning_gov.`, `agent_marketplace.`), citing MSRC Case 112362 as grounds it must not be widened without security review.

`verify()` returns a `GovernanceAttestation` (`passed`, `controls`, `toolkit_version`, `attestation_hash`, `controls_passed`/`controls_total`, plus evidence-mode fields), with `compliance_grade()` (A >=90, B >=80, C >=70, D >=60, else F) and `badge_markdown()` (shields.io, green at 100%, yellow >=80%, else red). `recalculate_hash()` was hardened to cover `passed`, pass/fail counts, and failure fields, since a narrower prior field set let an attacker flip `passed` without invalidating the hash.

**Runtime evidence verification** (`verify_evidence(evidence_path, *, strict=True, allow_failures=False)`) loads a JSON/YAML document requiring schema `EVIDENCE_SCHEMA = "agt-runtime-evidence/v1"` and a `deployment` object, then checks: policy files loaded and resolvable (blocking path traversal); deny-by-default or explicit deny semantics (narrowed from an earlier "any rule mentions deny" heuristic that misclassified allow-default policies); at least one registered tool; an enabled audit sink with a target; identity enabled; and a non-empty package/version manifest. Failures force `attestation.passed = False` unless `allow_failures=True`, a documented development-only escape hatch that always raises a `UserWarning`; `strict` (default `True`) is metadata only. CLI: `agt verify` supports `--badge`, `--evidence PATH`, and a deprecated no-op `--strict`; exits 1 if `attestation.passed` is false.

### Policy linting

`lint_policy.py`'s `lint_file`/`lint_path` validate YAML policy files: required top-level fields (`version`, `name`, `rules`, error if missing); deprecated field renames (`type`->`action`, `op`->`operator`, `policy_name`->`name`, `policy_version`->`version`, warning); empty rules list (warning); unknown `action` outside `{allow, deny, audit, block, escalate, rate_limit}` (error); unknown `operator` outside `{eq, ne, gt, lt, gte, lte, in, not_in, matches, contains}` (error); non-integer `priority` (error); and cross-rule `allow`/`deny` conflicts on matching condition keys (warning), using canonicalized JSON values so reordered dicts and type differences are handled correctly. Diagnostics report exact source lines via `_LineMap`, which walks the `yaml.compose()` AST rather than doing substring search. CLI: `agt lint-policy PATH [--strict]`.

### Integrity checks

`integrity.py`'s `IntegrityVerifier` detects tampering of the governance modules themselves: `GOVERNANCE_MODULES` (15 default modules) via SHA-256 source hash, and `CRITICAL_FUNCTIONS` (`PolicyEngine.evaluate`, `PolicyConflictResolver.resolve`, `AuditChain.add_entry`, `CardRegistry.is_verified`) via bytecode hash (`hashlib.sha256(marshal.dumps(func.__code__))`, replacing an earlier scheme that could miss a substituted function with identical opcodes). Both phases are fail-closed: with no manifest every check auto-passes; with a manifest, a module missing an entry is a failure rather than a pass, fixing a prior gap that let tampering evade detection. A corrupted manifest raises `ValueError` at construction rather than falling back to pass-everything mode. CLI: `agt integrity [--manifest PATH] [--generate OUTPUT_PATH]` (mutually exclusive) writes `integrity.json` when generating.

Separately, `security/schemas/security-exemptions.schema.json` defines exemption records for the plugin-marketplace security scanner, keyed by `tool` (`detect-secrets`, `bandit` with a `rule`, `pip-audit` with `cve`/`expires`) or `category`, carrying `file`, `reason`, `approved_by`, and optionally `line`, `fingerprint`, `ticket`, `temporary`. `SecurityScanner` (`security/scanner.py`) consumes these via `_load_exemptions()`/`_is_exempted()`, runs secret/dependency/code-pattern/markdown scans, and marks `critical`/`high` findings merge-blocking; it targets the marketplace trust surface rather than agent prompts.

### PromptDefenseEvaluator

`prompt_defense.py` implements a pure-regex, deterministic, zero-LLM-cost static analyzer that audits an agent's system-prompt text for missing defensive language before deployment. It does not test runtime behavior and complements, rather than replaces, runtime prompt-injection detection elsewhere in AGT. Documented performance is under 5ms for typical (<=2KB) prompts; `MAX_PROMPT_LENGTH = 100,000` characters guards against ReDoS, with `evaluate()` raising `ValueError` if exceeded.

The evaluator covers 17 total vectors, not 12: 12 mapped to the OWASP LLM Top 10 (conversational safety) plus 5 mapped to the OWASP Agentic Top 10 / ASI (agentic safety). Each vector is a `_DefenseRule(vector_id, name, owasp, patterns, min_matches)`. The 12 conversational vectors (`vector_id`: tag/severity): `role-escape` LLM01/high, `instruction-override` LLM01/high, `data-leakage` LLM07/critical, `output-manipulation` LLM02/medium, `multilang-bypass` LLM01/medium, `unicode-attack` LLM01/low, `context-overflow` LLM01/low, `indirect-injection` LLM01/critical, `social-engineering` LLM01/medium, `output-weaponization` LLM02/high, `abuse-prevention` LLM06/medium, `input-validation` LLM01/high. Most need `min_matches=2` (two independent pattern classes, e.g. `instruction-override` needs both a refusal verb and attack-vocabulary target concepts) so the bare attack string cannot self-score as "defended"; only `role-escape`, `output-manipulation`, and `context-overflow` need a single match. Regexes use bounded quantifiers instead of unbounded `.*` as a documented ReDoS mitigation.

The 5 agentic vectors, covering risks specific to autonomous agents (all `min_matches=2`): `cross-agent-auth` ASI-07/high, `transaction-guardrails` ASI-02/critical, `skill-provenance` ASI-04/high, `least-agency` ASI-01/high, `encoding-injection` ASI-01/high. Each requires a capability/attack-surface mention plus an explicit constraint (re-verification, a limit, a treat-as-data rule); this vocabulary was ported from the open-source `ultraprobe` npm package (MIT license), a disclosed attribution rather than an original invention.

Grading uses an ordered tuple `GRADE_THRESHOLD_LIST = (("A",90), ("B",70), ("C",50), ("D",30), ("F",0))` scanned top-down; score = `round(defended_count / total * 100)`. Confidence per finding: defended gives `min(0.9, 0.5 + matched*0.2)`; partial match gives flat `0.5`; zero matches gives flat `0.3`, correcting a prior inverted scheme that assigned high confidence to zero matches. Reports store a SHA-256 `prompt_hash` rather than raw prompt text, an explicit privacy/audit-trail design choice.

Core API: `evaluate(prompt)`, `evaluate_file(path)`, `evaluate_batch(prompts: dict)`, `to_audit_entry(...)` (an `AuditEntry`-shaped dict for `MerkleAuditChain` integration), and `to_compliance_violation(report)` (converts each undefended finding into a `{control_id: "OWASP:{owasp}::{vector_id}", severity, evidence, remediated: False}` record).

CLI: `agt red-team scan PATH [--min-grade {A,B,C,D,F}] [--json] [--strict]` accepts a prompt file or directory (globbing `*.txt`, `*.md`, `*.prompt`, `*.system`), reporting grade/score/coverage/missing vectors, failing under `--strict` below `--min-grade`. The group also exposes `attack` (adversarial playbooks via an optional `agent_sre.chaos.adversarial` dependency), `list-playbooks`, and `report`. Test coverage is 81 methods across 15 classes, including fixture prompts (weak, strong, partial) confirming the two-pattern design prevents bare attack vocabulary from self-scoring as defended.

Not covered in depth here: `policy_test.py`, `promotion.py`, `supply_chain.py`, `governance/attestation_validator.py`, `cli/contributor_check.py`, `cli/credential_audit.py`, `cli/cred.py`, and compliance-mapping documents under `docs/compliance/`, `docs/enterprise/`, `docs/analyst/`.

---

## 9. Agent Hypervisor and Agent Discovery

### Package status

`agent-hypervisor` (`agent-governance-python/agent-hypervisor`) is packaged as `agent_hypervisor` version `5.0.0` and marked a **deprecated standalone package**: `pyproject.toml` reads "Deprecated. Previously published as agent-hypervisor. Install agent-governance-toolkit-core instead," its sole runtime dependency being `agent-governance-toolkit-core>=4.1.0,<6.0`, and `src/hypervisor/__init__.py` raises a `DeprecationWarning` at import. Despite this, the full source tree remains present and tested, and `README.md` markets it as "Agent Hypervisor, Public Preview" (v2.1, roadmap through Q4 2026 "v3.0") claiming 644+ passing tests, a discrepancy between packaging (deprecated stub) and documentation (actively evolving product). The description below reflects the source code, with the deprecation noted.

`agent-discovery` is packaged as `agentmesh_discovery` version `5.0.0`, despite an internal `__version__` and CLI `--version` both hardcoded to `"0.1.0"`. It is `Development Status :: 3 - Alpha` and NOT deprecated, with optional extras `github` (`httpx`) and `agentmesh` (`agentmesh_platform`).

### Execution audit (`hypervisor.audit`)

Three modules under `hypervisor/audit/`: `delta.py`, `commitment.py`, `gc.py`. The **delta engine** implements a tamper-evident, append-only, SHA-256 hash-chained audit log, the module the README calls functional (versus stub). `SemanticDelta` records `delta_id`, `turn_id`, `session_id`, `agent_did`, `changes: list[VFSChange]`, `parent_hash`, `delta_hash`; `compute_hash()` runs SHA-256 over a canonical, sorted-key JSON serialization including `parent_hash` for chain linkage. `DeltaEngine.capture()` links each new delta to the prior delta's hash; `compute_hash_chain_root()` returns the last delta's hash (the "audit log root" surfaced by `Hypervisor.terminate_session`); `verify_chain()` walks the chain checking hash and parent linkage.

The **commitment engine** is explicitly a "stub implementation... Public Preview: stores commitments in-memory only. No blockchain anchoring." `CommitmentRecord.blockchain_tx_id` is always `None` (never set); `commit()` stores one record per session in memory (overwrites prior, no history); `queue_for_batch()`/`flush_batch()` form an in-memory FIFO documented "(Public Preview: no-op)," meaning no batching/anchoring effect occurs, not that the append itself is skipped.

The **ephemeral GC** is also explicitly a stub: "GC is a no-op. Data is retained in-memory for session lifetime only." Its `RetentionPolicy` (`delta_retention_days=180`) is declared but unenforced; `collect()` always reports zero purged files/caches and unchanged storage size, and `should_expire_deltas()` unconditionally returns `False`, confirmed by tests even for a 31-day-old delta against a 30-day policy.

**Wiring** (`hypervisor/core.py`): each `ManagedSession` owns its own `DeltaEngine`; the `Hypervisor` singleton owns one shared `CommitmentEngine` and `EphemeralGC`. On `terminate_session()`, audit commit is skipped if `enable_audit` is `False`; cleanup calls the no-op GC. `SessionConfig.enable_blockchain_commitment` is documented "Reserved... has no effect," confirmed by no reference to it in the audit code path.

### Execution rings and command denylist (`hypervisor.rings`)

`ExecutionRing(int, Enum)` has four levels, `RING_0_ROOT` through `RING_3_SANDBOX` (lower = more privileged). `from_eff_score(eff_score, has_consensus)` assigns `RING_1_PRIVILEGED` above 0.95 trust with consensus, `RING_2_STANDARD` above 0.60, else `RING_3_SANDBOX` (default for unknown/untrusted agents). Ring 0 is never auto-assigned; it requires SRE Witness attestation, enforced in `RingEnforcer.check()`.

`ActionClassifier` maps actions to a `ClassificationResult` (ring, risk weight, reversibility, confidence), deliberately uncached since `action_id` is not guaranteed unique to one behavior. `RingEnforcer` is the central gatekeeper: `RING_CONSTRAINTS` hardcodes per-ring limits on network, filesystem, subprocess access, and max concurrent tools (32 down to 2 from Ring 0 to Ring 3; Ring 3 disallows network and subprocess and is filesystem read-only). `check()` always denies actions requiring `RING_0_ROOT` regardless of caller flags, and otherwise denies if the agent's ring is numerically less privileged than required.

**Command denylist**: `RingEnforcer.check_command(command)`, added per the CHANGELOG as a v2.1.0 (2026-06-15) security feature. `None`/empty/whitespace-only input is denied; the base command is extracted via `stripped.split()[0]`, trailing shell metacharacters (`;&|`) stripped, then compared case-insensitively against `DENIED_COMMANDS`. Only trailing characters are stripped, so tests validate whitespace-delimited injection variants (`"curl; rm -rf /"`, `"curl && rm -rf /"`, `"curl | sh"`) rather than unspaced embedded ones. `DENIED_COMMANDS` (`hypervisor/sandbox/__init__.py`, 23 entries) groups network fetch tools (`curl`, `wget`, `ftp`, `telnet`), raw socket/proxy tools (`nc`, `ncat`, `netcat`, `socat`, `nmap`, `tcpdump`), alternative interpreters (`perl`, `ruby`, `python2`), the compiler toolchain (`gcc`, `g++`, `make`, `cc`), and shells (`bash`, `sh`, `dash`, `zsh`, `ksh`, `fish`). The module also defines `MINIMAL_SANDBOX_PATH` and `ALLOWED_BINARIES = ("python3", "python")`, shared with `docker/Dockerfile.sandbox`; a test confirms every denied command resolves to `None` via `shutil.which` on that path, giving defense-in-depth at both the API and sandbox-container PATH layers. `check_command` does not check the agent's ring itself; that is a separate check via `check_resource()`. `tests/unit/test_command_denylist.py` has 21 tests, matching the CHANGELOG's claim.

`RingBreachDetector` implements sliding-window anomaly scoring feeding automatic demotion/kill-switch triggers per the README. `RingElevationManager` implements TTL-bound "sudo"-style elevation (`elevate(agent_did, session_id, target_ring, ttl_seconds=300, max 3600)`, `revoke(elevation_id)`); the README states elevation "is available in the Enterprise Edition. Public Preview includes the API surface but returns a denial response," unlike ring computation/enforcement/breach-detection, which it calls fully functional.

### Agent Discovery: shadow-AI discovery architecture

Located at `agent-discovery/src/agent_discovery/`: `models.py`, `inventory.py`, `reconciler.py`, `risk.py`, `scanners/{base,process,config,github}.py`, `cli/main.py`. A Pydantic v2 `DetectionBasis` enum includes `PROCESS`, `GITHUB_REPO`, `CONFIG_FILE`, `KUBERNETES`, `AZURE`, `NETWORK`, `MANUAL`, though `KUBERNETES`/`AZURE`/`NETWORK` have no corresponding scanner implementation, appearing to be forward-declared/roadmap bases. `AgentStatus` spans `REGISTERED` through `SHADOW`/`DECOMMISSIONED`/`UNKNOWN`; `RiskLevel` spans `CRITICAL` down to `INFO`. `DiscoveredAgent` carries a `fingerprint` dedup key, identity fields (`did`, `spiffe_id`, `owner`), `status`, `evidence: list[Evidence]`, aggregate `confidence` (updated via `max()`), and timestamps. `compute_fingerprint()` canonicalizes merge keys sorted by key, then truncates a SHA-256 digest to 16 hex chars as the stable cross-scanner dedup key. A `ScannerRegistry` singleton auto-registers `ProcessScanner`, `ConfigScanner`, `GitHubScanner`.

**Process discovery** enumerates OS processes via subprocess shell-outs, not `psutil` ("to minimize dependencies"): `wmic` on Windows, `ps aux` on Unix, both with 30s timeouts, failing closed (`[]`) on error. `AGENT_SIGNATURES` defines 11 framework regex signatures with confidence scores, spanning `langchain`, `crewai`, `autogen`, `openai-agents`, `semantic-kernel`, the toolkit's own `agentmesh|agent.os|agent.governance` (0.95, highest), `mcp-server`, `llamaindex`, `haystack`, `pydantic-ai`, and `google-adk`; the first matching signature wins per process, and PID-based fingerprinting means a restarted agent produces a different fingerprint. **Secret redaction** applies 8 regex patterns substituting `[REDACTED]` for credential key/value forms, OpenAI-style `sk-` keys, GitHub and Slack tokens, AWS/Google API keys, PEM private key blocks, and JWTs, applied before storage (truncated to 500 chars).

**Config/artifact discovery** (`ConfigScanner`) walks the filesystem via `os.walk` (default depth 10), pruning common build/VCS directories. `CONFIG_PATTERNS` maps 12 known filenames to types/confidences (e.g. `agentmesh.yaml`/`.agentmesh/config.yaml` to `agt` at 0.95, `mcp.json`/`.mcp/config.json` to `mcp-server` at 0.85), and also scans `Dockerfile`/`docker-compose.yml` (up to 64KB) against 6 regex patterns at a flat 0.70 confidence. File contents are never persisted, only metadata and paths.

**GitHub discovery** (`GitHubScanner`, requires optional `httpx`) authenticates via `token` or `GITHUB_TOKEN`, warning unauthenticated requests cap at 60/hour versus 5,000/hour authenticated. It uses the Git Tree API to fetch the full file listing in one call, then issues follow-up content calls only for confirmed-present dependency manifests, cutting API calls "from 13+ per repo down to 1-4." `AGENT_DEPENDENCIES` checks 11 dependency patterns with word-boundary regex, from `agent-os-kernel`/`agentmesh-platform` (0.85) down to generic `mcp` (0.60, most false-positive-prone), and rate limiting backs off exponentially on HTTP 403/429.

**Inventory** (`AgentInventory`) is an in-memory dict keyed by fingerprint, optionally JSON-file-backed; `ingest()` merges evidence into existing agents sharing a fingerprint (the cross-scanner correlation mechanism) and persists after each ingest. **Reconciliation**: `RegistryProvider(ABC)` decouples reconciliation from any specific governance backend; `StaticRegistryProvider` matches by exact DID, fuzzy substring name match (a loose match risking false-negative shadow detection), or exact fingerprint. `Reconciler.reconcile()` sets status to `REGISTERED` or `SHADOW`, builds `ShadowAgent` recommendations, but does not itself persist the status change.

**Risk scoring** is additive, clamped to `[0, 100]`: no DID/SPIFFE identity (+30), no owner (+20), shadow/unregistered status (+20), high-risk type (`autogen`/`crewai`/`langchain`/`openai-agents`, +15) or medium-risk type (`mcp-server`/`semantic-kernel`/`pydantic-ai`, +10, mutually exclusive), age over 30 days (+10) or 7 days (+5), confidence under 0.5 (-10, the only mitigating factor); thresholds run `>=75` CRITICAL down to `>=10` LOW, else INFO.

**CLI** (`agent-discovery`, `click`/`rich`): `scan` (`--scanner process|config|github`, `--paths`, `--github-org`, `--output table|json`, `--storage`, default `~/.agent-discovery/inventory.json`, differing from the README's documented `~/.agent-governance-python/agent-discovery/inventory.json`), `inventory` (`--output table|json|summary`), and `reconcile` (`--registry-file`, printing a risk-sorted, color-coded table).

### Test coverage and gaps

Directly counted: hypervisor `test_audit.py` (13), `test_command_denylist.py` (21), `test_rings.py` (10), `test_ring_enforcement.py` (54), `test_ring_improvements.py` (33, summing to 118 for the rings module across four files); discovery `test_cli.py` (8), `test_inventory.py` (11), `test_models.py` (13), `test_process_redaction.py` (3), `test_reconciler.py` (8), `test_risk.py` (9), `test_scanners.py` (5). The hypervisor README's "644+ Tests Passing" headline and per-module table (audit: 10, rings: 34) is a rollup that does not exactly match these direct per-file counts, an unresolved discrepancy not confirmed via `pytest --collect-only`.

Not covered here: `hypervisor/liability/*` (Joint Liability subsystem), `hypervisor/saga/*`, `hypervisor/session/*`, `hypervisor/observability/*`, `hypervisor/security/kill_switch.py`/`rate_limiter.py`, `hypervisor/integrations/*`, `hypervisor/api/*`, and the `agentmesh` extra in `agent-discovery`.

---

## 10. Agent Marketplace and Agent Lightning

Agent Marketplace and Agent Lightning are two of the toolkit's Python packages, at `agent-governance-python/agent-marketplace` and `agent-governance-python/agent-lightning` (section 3 covers the full package ecosystem), both versioned `5.0.0` in lockstep. `agent-marketplace` (`agentmesh_marketplace`) is status Beta, extracted from `agentmesh.marketplace` with a backward-compat import shim. `agent-lightning` (`agentmesh_lightning`) is labeled "Public Preview" ("APIs may change before GA"), extracted from `agent_os.integrations.agent_lightning`, and declares zero required third-party dependencies (Agent OS support is an optional extra). Neither package hard-depends on siblings; cross-package calls are duck-typed or wrapped in `try/except ImportError`.

### Agent Marketplace: plugin governance

The canonical schema is `PluginManifest` (pydantic model, `manifest.py`), backing `agent-plugin.yaml`. Fields include `name`, `version` (rejecting Unicode digit-like characters that could crash registry sorting), `description`, `author`, `plugin_type` (`POLICY_TEMPLATE`, `INTEGRATION`, `AGENT`, `VALIDATOR`), `capabilities`, `dependencies`, `signature`, `organization` (None means global/shared), and `artifact_url`/`artifact_sha256`. `signable_bytes()` produces canonical JSON (sorted keys, ASCII-only) excluding `signature`, replacing an earlier YAML canonicalization that was not byte-stable across environments. `PluginSigner` (`signing.py`) wraps an Ed25519 private key to sign manifests; `verify_signature()` raises `MarketplaceError` on failure. Because `artifact_sha256` is part of `signable_bytes()`, a signature binds manifest metadata to the artifact hash.

`PluginInstaller` (`installer.py`) resolves manifests, optionally verifies signatures, recursively resolves dependencies with cycle detection, and, when `artifact_url` is set, downloads and SHA-256-verifies the artifact before unpacking; without one, only the signed manifest is written (registration-only mode). Install-time static scanning blocks source imports of `subprocess`, `os`, `shutil`, `ctypes`, `importlib`, though dynamic imports go undetected, with runtime enforcement deferred to an external sandbox. `PluginRegistry` (`registry.py`) is a thread-safe name/version map with optional JSON persistence; `register()` checks policy compliance and duplicates before mutating state.

`MarketplacePolicy` (`marketplace_policy.py`) covers `mcp_servers` (allowlist/blocklist mode), `allowed_plugin_types`, `require_signature`, and per-organization overrides; organizations can only add to enterprise-allowed plugin types, never remove from them. In blocklist mode, blocked sets merge; in allowlist mode, the effective policy is the strict intersection of org-requested and enterprise-allowed servers, fixing a prior bug where an out-of-allowlist request silently granted the entire enterprise allowlist. `evaluate_plugin_compliance()` returns `ComplianceResult(compliant, violations)` after checking signature, type, and MCP allow/block rules.

#### Trust tiers

`trust_tiers.py` is the primary plugin trust-scoring system, modeled on but standalone from AgentMesh's identity trust scoring (section 5): a 0-1000 score across five tiers.

| Tier | Score range | max_token_budget | max_tool_calls | allowed_tool_access |
|---|---|---|---|---|
| revoked | 0-299 | 0 | 0 | read-only |
| probationary | 300-499 | 1,000 | 5 | read-only |
| standard | 500-699 | 5,000 | 25 | read-write |
| trusted | 700-899 | 20,000 | 100 | read-write |
| verified | 900-1000 | 100,000 | 500 | full |

Capabilities are gated by minimum tier: `network`/`filesystem` need standard+, `execute` needs trusted+, `admin` needs verified only. New plugins start at score 500 (+100 signature, +50 non-empty capabilities, -50 for a short description), clamped to [0, 1000]. `PluginTrustStore` persists scores to JSON and raises `RuntimeError`, rather than silently zeroing, on a corrupt store, since "silently zeroing all scores would re-promote previously-untrusted plugins."

`usage_trust.py` layers a separate, additive telemetry adjustment (capped at plus-or-minus 200) on the base score: adoption bonuses by daily active users, reliability bonuses/penalties by error rate (at 1,000+ invocations), an incident penalty (`max(incident_count * -50, -200)`), and staleness/adoption-trend adjustments. Trust and quality remain three distinct systems: trust tiers ("is this safe?"), usage-trust adjustment, and quality scoring ("is this good?") via `quality_scoring.py` (badges UNRATED/BRONZE/SILVER/GOLD/PLATINUM) and `quality_assessment.py` (letter grades A-F across dimensions like security posture, test coverage, API design); quality scores do not feed back into trust tier or capability limits.

Supporting modules: `schema_adapters.py` converts third-party manifest formats (Copilot-style, Claude-style) into the canonical `PluginManifest`; `batch.py` evaluates a directory of manifests against a YAML policy, producing severity-ranked violations; `workflow_bundle.py` groups agent/skill/tool/knowledge components into named bundles with an associated governance policy reference; `hooks.py` provides pre-commit/CI validation entry points; `exceptions.py` defines the single `MarketplaceError` used throughout. The CLI (`agentmesh-marketplace`) exposes a `plugin` group: `install`, `uninstall`, `list`, `search`, `verify`, `publish`, `evaluate`, `trust`, plus a conditionally-registered `evaluate-batch`.

### Agent Lightning: RL training governance

Agent Lightning wraps reinforcement-learning training loops, compatible with the external `agentlightning` package's `Trainer`, `GRPO` algorithm, and `LightningStore`, with Agent OS kernel policy enforcement (section 4). The README's benchmark table claims policy violations drop from 12.3 percent to 0.0 percent and task accuracy rises from 76.4 percent to 79.2 percent with Agent OS enabled; these are illustrative README figures, not independently verified in the test suite. The governing spec, `docs/specs/AGENT-LIGHTNING-FAST-PATH-1.0.md` (spec ID **AGENT-LIGHTNING-GOVERNANCE-1.0**, status Draft), cross-references the Agent OS Policy Engine, AgentMesh Identity and Trust, and Agent Hypervisor Execution Control specs. Despite the filename, "fast-path" appears nowhere else in the codebase; the spec covers the full RL governance surface with no low-latency bypass mechanism described, so the name is a codename, not an implemented feature.

`GovernedRunner` (`runner.py`) exposes an Agent-Lightning-compatible interface (`init`, `init_worker`, `teardown`, `async step()`, `async iter()`). Per-step violation and signal state is isolated via `contextvars.ContextVar`, reset in a `finally` block, preventing cross-contamination between concurrent `step()` calls on one instance. `step()` prefers `kernel.execute_async`, falls back to `kernel.execute`, then to an ungoverned call with a warning; exceptions are logged with full traceback rather than dropped. `PolicyViolationType` has exactly four members: `BLOCKED`, `MODIFIED`, `WARNED`, `SIGNAL_SENT`. Severity-to-penalty defaults map critical/high/medium/low to 100.0/50.0/10.0/1.0, applied only when the caller supplied no explicit penalty; `GovernedRollout.total_penalty` is always recomputed as the sum of violation penalties.

`RewardConfig` (`reward.py`) uses the same penalty magnitudes plus a clean bonus of +5.0, a reward clamp of [-100.0, 100.0], and an optional multiplicative mode; `policy_penalty()` falls back to the medium penalty for unrecognized severity strings. `PolicyReward.__call__()` computes base reward plus penalty (or multiplicative), adds the clean bonus when no violations occurred, and clamps to bounds; `CompositeReward` combines weighted reward functions.

`GovernedEnvironment` (`environment.py`) is a Gymnasium-compatible environment (`max_steps=100`, `violation_penalty=-10.0`, `terminate_on_critical=True`, `success_bonus=10.0`). Penalty scaling multiplies the base penalty by 10 for critical and by 5 for high, unmultiplied for medium/low; termination occurs on any critical violation when configured, truncation at `max_steps`. If the kernel exposes no push hook, `step()` falls back to polling recent violations, so a pull-only kernel does not silently record zero violations.

`FlightRecorderEmitter` (`emitter.py`) adapts Agent OS Flight Recorder audit entries into `LightningStore` span format, filtering by entry type (`policy_check`, `signal`, `tool_call`) and truncating tool args/results to 1000 characters; it maintains a cursor to avoid the prior O(n squared) cost of repeated full walks.

Security considerations documented in the spec (section 17) but not fully code-enforced: `violation_callback` is not validated as callable at construction, despite the spec's SHOULD; the runner trusts the kernel to faithfully report violations, with the Flight Recorder as a secondary reconciliation trail; and the file-export path uses a plain `open()` with no symlink-following guard despite the spec's MUST NOT.

---

## 11. RAG governance and MCP governance

### Package landscape

Per `docs/package-consolidation/MIGRATION.md`: `agent-mcp-governance` is a deprecated stub package (replacement: `agent-governance-toolkit-protocols`), while `agent-rag-governance` is explicitly declared standalone and excluded from consolidation (alongside `agent-discovery`, `agentmesh-lightning`, `agentmesh-drift`, `agentmesh-observability`, `agentmesh-marketplace`). The two are architecturally different: one is a live governance product, the other an empty forwarding shim whose real functionality lives elsewhere.

### `agent-mcp-governance`: deprecated re-export shim

Location: `agent-governance-python/agent-mcp-governance/`. Metadata: `name = "agent_mcp_governance"`, `version = "5.0.0"`, description "Deprecated ... Install agent-governance-toolkit-protocols instead," dependency `agent-governance-toolkit-protocols>=4.1.0,<6.0`.

The entire runtime module (`src/agent_mcp_governance/__init__.py`) emits a `DeprecationWarning` on import and defines nothing else. Its own `README.md` describes a richer, non-existent surface, claiming a "thin, typed re-export surface" exposing `GovernanceMiddleware`, `AuditMiddleware`, `TrustGate`, and `BehaviorMonitor` from `agent_os.*` submodules, and depending on `agent-os-kernel >=3.0.0,<4.0.0` (conflicting with the actual `pyproject.toml` dependency). None of these four names are actually re-exported: **the described MCP governance middleware API is aspirational/stale relative to the actual importable module**, which only warns.

### Where MCP security actually lives

The real interception and threat-detection logic sits in `agent-governance-python/agent-os/src/agent_os/mcp_security.py` (1,049 lines), under the `agent_os` package (itself also labeled deprecated, pointing to `agent-governance-toolkit-core`). The normative spec is `docs/specs/MCP-SECURITY-GATEWAY-1.0.md` (Draft, 2025-07-28, 1,909 lines).

#### Gateway architecture

`MCPGateway` sits between agent runtime and MCP tool servers as a two-stage pipeline:

1. **Tool call interception**: `intercept_tool_call(agent_id, tool_name, params) -> (allowed, reason)`, checked in order: deny-list, allow-list, sensitive-tool approval callback, rate limit, allow. Deny-list always wins over allow-list. Constructor accepts `allowed_tools`, `denied_tools`, `sensitive_tools`, `approval_callback`, `enable_builtin_sanitization` (default true), `metrics`, `rate_limit_store`, `audit_sink`, `response_scanner`, `response_policy` (default BLOCK).
2. **Response scanning**: `intercept_tool_response(...) -> MCPResponseDecision` via `MCPResponseScanner`, checking instruction-tag injection (`<SYSTEM>`, `[INST]`, `<|im_start|>`, `<<SYS>>`), imperative injection ("ignore previous instructions"), credential leaks, PII leaks, and exfiltration URLs. Enforcement is `BLOCK`/`SANITIZE`/`LOG`.

#### Security scanner: tool poisoning, rug pulls, typosquatting, hidden instructions

`MCPSecurityScanner` (`mcp_security.py:328`) statically analyzes tool definitions. `MCPThreatType` has six values matching the spec: `TOOL_POISONING`, `RUG_PULL` (description/schema changed since registration), `CROSS_SERVER_ATTACK` (including typosquats), `CONFUSED_DEPUTY`, `HIDDEN_INSTRUCTION`, `DESCRIPTION_INJECTION`, with severity `INFO`/`WARNING`/`CRITICAL`.

`scan_tool(tool_name, description, schema=None, server_name="unknown")` runs four ordered checks:

1. **Hidden instructions**: invisible Unicode (zero-width joiners, RTL overrides) → CRITICAL; hidden markdown/HTML comments → CRITICAL; encoded base64/hex payloads decoded and matched against suspicious keywords (long base64 flagged by default) → WARNING; excessive whitespace → WARNING; instruction-like regexes → CRITICAL.
2. **Description injection**: reuses the shared `prompt_injection.py` detector, plus role-override (WARNING) and exfiltration (CRITICAL) regexes.
3. **Schema abuse**: suspicious field names, excessive parameter counts, embedded instructions in field descriptions.
4. **Cross-server attack**: references to other MCP servers, instructions to invoke external tools, typosquatted names via `_is_typosquat`.

**Typosquatting.** `_is_typosquat(name_a, name_b)` lower-cases both names and computes Levenshtein distance: `1 <= dist <= 2 and min(len(la), len(lb)) >= 4` fires `CROSS_SERVER_ATTACK`, matching spec section 6.9 verbatim (names on different servers, 1-2 edits apart, shorter name at least 4 characters).

**Rug pulls.** `check_rug_pull` compares current description/schema SHA-256 hashes against a stored `ToolFingerprint`; any mismatch produces a CRITICAL `RUG_PULL` threat and bumps the fingerprint's version counter.

`ScanResult` aggregates a server scan (`safe`, `threats`, `tools_scanned`, `tools_flagged`). `MCPSecurityConfig` allows overriding all built-in regex pattern lists; absent a config, built-in defaults apply with a "sample rules are in use" warning.

#### Other gateway spec components

- **Message signing** (`MCPMessageSigner`): HMAC-SHA256 over payload+nonce+timestamp+sender_id, 5-minute replay window, 10,000-entry nonce cache.
- **Session auth** (`MCPSessionAuthenticator`): 1-hour token TTL, max 10 concurrent sessions per agent with oldest-eviction.
- **Sliding rate limiter** (`MCPSlidingRateLimiter`): 100 calls / 300 seconds default.
- **Auth enforcement** (`McpAuthPolicy`): methods `{oauth2, mtls, api_key, bearer, none}`, `deny_none=true` default, `require_tls=true`, `min_tls_version="1.2"`.
- **CVE feed** (`McpCveFeed`): OSV API integration, 1-hour cache, fails closed if unreachable.
- **Trust-gated MCP** (`TrustGatedMCPServer`/`Client`): AgentMesh DID-based identity (see section 5); checks in order are 1MB max argument size, tool existence, trust-score threshold (default 300), wildcard capability match, circuit breaker (5 failures, 1-minute reset). Client blocks loopback/link-local addresses and restricts to http/https/ws/wss.
- **Agent SRE MCP Server**: exposes `sre_check_slo`, `sre_report_cost`, `sre_request_budget`, `sre_check_rollout_status`, `sre_list_slos` (see section 7).
- **Schema drift detection**: `DriftDetector` with 8-value `DriftType` (`TOOL_ADDED`, `SCHEMA_CHANGED`, `PARAMETER_REMOVED`, `TYPE_CHANGED`, etc.) and 3-value `DriftSeverity`, using deterministic schema fingerprinting.
- **Metrics/audit**: OTel-backed `MCPMetrics` (`mcp_decisions`, `mcp_threats_detected`, `mcp_rate_limit_hits`, `mcp_scans`); every decision also produces an `AuditEntry` with an external `audit_sink` hook for SIEM forwarding.

**Test coverage**: `test_mcp_security.py` (63 tests), `test_spec_mcp_gateway_conformance.py` (127 conformance tests for the spec).

**Summary**: `agent-mcp-governance` implements none of this directly; it is a deprecation-warning shim. The substantive functionality lives in `agent-os`, with intended long-term homes `agent-governance-toolkit-protocols`/`-core`.

### `agent-rag-governance`: active, standalone RAG governance package

Location: `agent-governance-python/agent-rag-governance/`. `pyproject.toml` declares `version = "5.0.0"` versus the module's own `__version__ = "0.1.0"` (a discrepancy), Alpha status, sole hard dependency `pydantic>=2.4.0,<3.0` (though `RAGPolicy` is a plain dataclass). Optional extras: `langchain`, `llamaindex`.

Per its README, this fills a gap left by `agent-os`'s write-time (`MemoryGuard`) and output-time (`ContentGovernance`) controls: no retrieval-time collection access control, audit trail, retrieval-loop rate limiting, or pre-LLM chunk scanning.

Public API: `RAGGovernor`, `GovernedRetriever`, `RAGPolicy`, `RAGAuditEntry`, `AuditLogger`, `ContentScanner`, `ScanResult`, `RateLimiter`, plus exceptions `RAGGovernanceError`, `CollectionDeniedError`, `RateLimitExceededError`, `ContentScanError`, across `governor.py` (373 lines), `llamaindex.py` (270), `policy.py` (204), `content_scanner.py` (187), `audit.py` (185), `rate_limiter.py` (99), `exceptions.py` (82).

`RAGGovernor(policy, agent_id).wrap(retriever, collection)` returns a `GovernedRetriever` whose `.invoke()` runs, in order: `_check_collection` (raises `CollectionDeniedError`), `_check_rate` (raises `RateLimitExceededError`), `_retrieve` (falls back to legacy `.get_relevant_documents`, raising `TypeError` rather than silently dropping kwargs), `_scan_chunks` (filters blocked chunks with a warning log, not an exception), and `_audit` in a `finally` block so denials or exceptions are never hidden from compliance review. Async/batch/streaming methods (`ainvoke`, `astream`, etc.) explicitly raise `NotImplementedError` instead of being transparently forwarded, preventing a silent bypass.

`RAGPolicy` fields include `allowed_collections`, `denied_collections`, `max_retrievals_per_minute`, `content_policies` (`"block_pii"`, `"block_injections"`), `cedar_policy`/`cedar_policy_path`. Cedar, when set, takes precedence over allow/deny lists; if the Cedar backend (`agent_os.policies.backends.CedarBackend`, optional dependency) falls back to its generic evaluator (which ignores resource constraints), `RAGPolicy` re-evaluates the raw statements itself with forbid-overrides-permit and resource matching.

`ContentScanner` normalizes text via `unicodedata.normalize("NFKC", ...)`, strips Unicode format-category characters (zero-width joiners, bidi marks), and casefolds before matching, blocking obfuscation attacks; residual confusable-glyph attacks (e.g. Cyrillic for Latin letters) remain an acknowledged limitation. It runs 8 injection regexes (instruction-override, role-override, code execution) and 4 PII regexes (email, phone, SSN, BIN-aware credit card numbers), described as "pure regex, deterministic, zero LLM cost, under 1ms per chunk," sharing its OWASP ASI06 injection taxonomy with `agent_os.memory_guard` without importing it, keeping the base package dependency-free of `agent-os`.

`RateLimiter` is a pure-Python, thread-safe sliding-window limiter with periodic compaction. `RAGAuditEntry` never logs raw query text, only a salted SHA-256 hash (reading `AGENT_RAG_AUDIT_SALT`, else unsalted with a one-time warning, since unsalted hashes of common queries are reversible via rainbow tables). `AuditLogger` writes JSON-lines to stdout or an append-mode file, thread-safe per-process but explicitly not cross-process safe.

`GovernedQueryEngine` extends the same `RAGGovernor` pipeline to LlamaIndex `BaseQueryEngine`/`BaseRetriever` objects, confirming the same four controls apply uniformly across LangChain- and LlamaIndex-style integrations.

Nine test files (`test_audit.py`, `test_cedar_policy.py`, `test_content_scanner.py`, `test_exceptions.py`, `test_governor.py`, `test_llamaindex.py`, `test_policy.py`, `test_rate_limit_window.py`, `test_rate_limiter.py`, over 80 test functions) cover Unicode-obfuscation resistance, Cedar precedence/fallback, bypass-method blocking, rate-limiter windows, and audit correctness.

---

## 12. Agent Control Specification (ACS) policy engine

### What ACS is

The Agent Control Specification (ACS), rooted at `policy-engine/`, is the policy decision layer folded into AGT as the "AGT 5.0 policy layer": a pure Rust core plus multi-language SDK bindings. Per `policy-engine/README.md` and the normative `policy-engine/spec/SPECIFICATION.md` (version `0.3.1-beta`, status Draft), ACS is stateless (no mutable state persists across evaluations, §1.1), deterministic (same manifest, snapshot, mode, and dispatcher outputs always produce the same verdict), fail closed (any evaluation error yields `deny` with a reserved `runtime_error:` reason and no transform, §1.1/§21), and I/O-free during evaluation (no network, model, tool, or classifier/judge calls inside the runtime).

The host (an AGT adapter in Agent OS, or any embedding application) is the Policy Enforcement Point: it assembles a JSON snapshot at a defined intervention point and calls ACS, the Policy Decision Point, which evaluates the bound policy and returns a normalized verdict the host must enforce. "ACS decides, the host enforces" (`QUICKSTART.md`). The integration spans AGT host adapters, an `agt-policies` Python bridge, and the ACS native runtime itself. AGT folder discovery, scope, and merge pre-resolve manifests before the engine sees them (`spec/agt/AGT-RESOLUTION-1.0.md`), so the runtime always receives one fully composed, `extends`-free manifest.

### Core model: manifest, intervention points, policy input

A manifest (single YAML or JSON document, unknown top-level keys rejected) declares a required `agent_control_specification_version`, optional `metadata`, optional `extends` (ordered parent paths or HTTPS URLs, §2.2), required `policies`, required `intervention_points` (keyed only by the 8 closed names), optional `tools` catalog, optional `annotators` (`classifier`, `llm`, `endpoint`), and an AGT-added optional `approval` block (§24). The eight intervention points (a closed set; an unknown name fails closed with `intervention_point_unknown`) are `agent_startup`, `input`, `pre_model_call`, `post_model_call`, `pre_tool_call`, `post_tool_call`, `output`, `agent_shutdown`; only the two tool points project a tool from the manifest `tools` catalog.

Manifest paths (§3) root at `$snap` (raw snapshot), `$pi` (canonical policy input), `$policy_target`, or `$tool` (`null` for non-tool points), with allowed roots restricted per field (`transform.path` may only use `$policy_target`); violations fail closed with `manifest_invalid` or `transform_target_forbidden`. The canonical policy input (§7) has exactly five members (`intervention_point`, `policy_target`, `snapshot`, `annotations`, `tool`); canonical serialization (§8) sorts object members by name at every level, the basis for hashing and action-identity digests. Evaluation order (§6, 9 steps) resolves the intervention point config and `policy_target`, projects the tool, collects annotations, calls the dispatcher, normalizes into a `Verdict`, and (in `enforce` mode) applies the transform. Two modes exist (§5): `enforce` applies `transform`; `evaluate_only` validates but does not apply it; the computed verdict is identical in both.

### Verdicts and transform semantics

A dispatcher returns JSON normalized into a `Verdict` (`core/src/verdict.rs`, `Decision` enum: `Allow | Deny | Warn | Escalate | Transform`) with members `decision`, `reason` (must not start with `runtime_error:`), `message`, `transform` (required iff `decision=transform`), `evidence` (offline-verification pointer, §13.3), and `result_labels` (stateless IFC channel, §13.2). `allow`/`warn` permit with no target change; `transform` permits and replaces the policy target (§14); `deny` refuses; `escalate` defers to the host approval path (§17.1).

This is the largest documented divergence from upstream ACS: upstream verdicts could carry a mutating "effects" array on `allow`/`warn`. AGT removes verdict-attached effects entirely and replaces them with a dedicated `transform` decision. The `effects` module (`core/src/effects.rs`, 776 lines) is still compiled for internal/test continuity during "the M2 sunset" but is not re-exported publicly.

`Transform` (`{path, value}`) must be rooted at `$policy_target`; other roots fail closed with `transform_target_forbidden`, and a malformed/unresolvable path or missing value fails with `transform_invalid`. It cannot touch the snapshot, annotations, projected tool, or host state; multi-step rewriting chains intervention points. Every evaluation derives `input_identity` and `enforced_identity` (SHA-256 digests, `sha256:<hex>`, §13.1); escalation approval binds to `enforced_identity` to close a time-of-check/time-of-use gap. IFC (§13.2, §11) is a stateless label-flow model: the runtime never stores label state, a policy returns `result_labels`, and the host re-supplies them as `input.snapshot.ifc.source_labels` on later calls. The Rego library `agent_control_specification.lib.ifc` (`policy/lib/ifc.rego`, mirrored in `policy/cedar-lib/ifc.cedar`) defines the default lattice `public < internal < confidential < secret`.

### Policy types, dispatchers, resource limits

Four `policies.type` values (§12.1): `rego` targets OPA via `bundle` (path) or `bundle_url` (HTTPS, pinned hash); `cedar` targets Cedar (`cedar-policy` crate v4) with inline `policy_set` or `policy_path`, mapping `Allow`/`Deny` to `allow`/`deny` and expressing `warn`/`escalate`/`transform` via an `advice` annotation (§12.4); `test` is a fixed-verdict test double; `custom` invokes a host dispatcher identified by a required `adapter` string. The dispatcher boundary (§12.3) is synchronous and opaque: the runtime hands a `PreparedPolicyInvocation` to a `PolicyDispatcher` trait implementation and treats its output as JSON; a dispatcher error fails closed with `policy_invocation_failed`. Annotators (§10, types `classifier`, `llm`, `endpoint`) are always host-provided via `AnnotatorDispatcher`; ACS ships no built-in classifier/judge engine, only reference implementations under `core/src/dispatchers/` gated behind sub-features `aacs`, `openai_moderation`, `perspective`, `llama_guard`, `lakera_guard`, `auto`.

`core/src/limits.rs` bounds snapshot size (1 MiB default), policy-input nesting depth (64), annotators per point (16), annotator/policy output size (256 KiB each), `extends` chain depth (16), and merged manifest size (1 MiB); a breach fails closed with `resource_limit_exceeded`. Reserved reasons (§16, `core/src/error.rs` `RuntimeError` enum) include `manifest_invalid`, `intervention_point_unknown`, `path_missing`, `tool_unknown`, `annotation_failed`, `policy_invocation_failed`, `transform_invalid`, `transform_target_forbidden`, `approval_action_mismatch`, plus AGT host-side `resolution_path_traversal`, `resolution_cycle`, `resolution_invalid_governance`, `resolution_merge_conflict`. `spec/reserved-reasons.json` is stale: it still lists legacy `effect_invalid`/`effect_target_forbidden` instead of the `transform_*` names and omits the `resolution_*` reasons, even though `RuntimeError` keeps both legacy and current variants side by side.

Section 17 obligates a conformant host to never carry out a `deny`, always route `escalate` to an approval path and block until resolved, always substitute the transformed target when present, and never present `evaluate_only` results as enforcement. The approval resolver callback (§17.1), consulted only for `escalate` in `enforce` mode, must return an outcome carrying `enforced_identity`, re-derived before proceeding; outcomes are Allow/Deny/Suspend, and any unconfigured or failing resolver is treated as deny. The optional manifest `approval` block (§24) declares `default_resolver`, `timeout_seconds`, `on_timeout` (`deny`/`allow`/`suspend`), and a `resolvers` map. `core/src/telemetry.rs` defines a `TelemetrySink` trait and redacted-by-design event kinds, with built-in sinks `NoopTelemetrySink`, `InMemoryTelemetrySink`, `StdoutJsonTelemetrySink`, `MultiSink`; every SDK shares one OpenTelemetry contract, and a sink that raises is swallowed, so telemetry is never load-bearing.

### Rust workspace, FFI, and SDK bindings

`policy-engine/Cargo.toml` defines a workspace (`core`, `sdk/python`, `sdk/rust`, `integrations/rig`, `integrations/openai`, `integrations/mcp`, `integrations/annotators`, `integrations/otel`) embedded in the top-level AGT Cargo workspace. Key crates, all version `0.3.1-beta.0`: `agent_control_specification_core` (`core/`, the stateless runtime, `crate-type = ["lib", "cdylib"]`), `agent_control_specification` (`sdk/rust/`), `agent_control_specification_openai`/`_mcp`/`_rig` (guards for `async-openai`, `rmcp`, `rig-core`), `agent_control_specification_annotators`, and `agent_control_specification_otel` (the only integration crate marked `publish = true`). The README claims the runtime was "renamed from `agent_control_specification_core` to `agt_core_engine` in M2," but the crate is still named `agent_control_specification_core` everywhere: aspirational roadmap text, not implemented state. Core `src/` totals roughly 11,462 lines, led by `manifest.rs` (3,108 LOC) and `runtime.rs` (1,387 LOC).

`core/src/ffi.rs` (869 lines) is a minimal C ABI (NUL-terminated UTF-8 strings, `acs_free_string`, panics caught via `catch_unwind`) with 16 exported `#[no_mangle]` functions forming the surface the Python (PyO3), Node (napi-rs), and .NET (P/Invoke) SDKs bind against. Wire JSON schemas live under `spec/schema/wire/`: `snapshot`, `policy-input`, `verdict`, a vestigial `effect.schema.json`, `request`, `result`; the manifest schema is `spec/schema/manifest.schema.json`.

Four host-language SDKs wrap the shared Rust core, all describing ACS as "a stateless, deterministic, fail-closed policy decision runtime." Python (`sdk/python/`, PyPI `agent-control-specification`, `0.3.1b0`, PyO3 via maturin, Python >=3.11): `AgentControl.from_path()`/`.evaluate_intervention_point()`/`.run()`/`.protect_tool()`, with adapter helpers spanning LangChain, OpenAI/Anthropic clients, OpenAI Agents SDK, AutoGen, CrewAI, Semantic Kernel, LiteLLM proxy, Azure AI Foundry, and MCP. Node (`sdk/node/`, npm `agent-control-specification`, `0.3.1-beta.0`, napi-rs, Node >=18): `AgentControl.fromPath()`. .NET (`sdk/dotnet/`, NuGet `AgentControlSpecification`, P/Invoke): `AgentControl.FromPath()`, with companion packages `.AI`, `.SemanticKernel`, `.AutoGen`, `.AgentFramework`. Rust (`sdk/rust/`, crate `agent_control_specification`) shares the core's feature set. No Go SDK exists despite the README claiming "Go added in M4"; no `go.mod` or `sdk/go` exists under `policy-engine/`. Per-language orchestration helpers bundle evaluation, enforcement, and approval-resolver consultation into single calls: `run` (`input`+`output`), `run_model` (`pre_model_call`+`post_model_call`), `run_tool`/`protect_tool` (`pre_tool_call`+`post_tool_call`).

### The `acs-generate` CLI

`policy-engine/generator/` is a separate Python package, `acs-generator` (PyPI), `0.3.1b0`, Python >=3.11, optional `openai` extra, console scripts `acs-generate`/`acs` mapping to `acs_generator.cli:main`. Key modules: `cli.py`, `engine.py` (`GenerationEngine`, LLM-prompt-driven generation), `manifest_builder.py`, `rego_builder.py` (renders Rego from a decision-severity rule plan: `deny(4) > escalate(3) > transform(2) > warn(1) > allow(0)`), `llm.py` (`OpenAICompatibleLanguageModel`).

Two generation modes: guided init (deterministic, no LLM), for example `acs-generate init --non-interactive --name "Payments Agent" --points input,pre_tool_call,output --tool wire_transfer:banking,payments --deny-keyword password --out build/acs-payments`; and free-text generation (`acs-generate --prompt "..." --tool ... --out ...`) calling an OpenAI-compatible LLM via `--api-base`/`--api-key`/`--model` or `ACS_GENERATOR_*` env vars. Output artifacts: `manifest.yaml`, `policy/<slug>.rego`, `report.md`, optional `snapshots/<intervention_point>.json` and `test_policy.py`. Validation schema-checks the manifest, applies the Python SDK's semantic checks, rejects deprecated policy-input keys, and checks generated Rego when `opa` is on `PATH`. `generator/README.md` retains stale "effects" terminology predating the transform-only migration, though `rego_builder.py`'s actual behavior is current.

### Stock policy libraries, Cedar mirror, and the spec set

`policy-engine/policy/lib/` (Rego) and `policy-engine/policy/cedar-lib/` (Cedar) are two parallel, matched reusable policy libraries, each with unit tests and a `run_tests.sh`, covering ten shared concerns: default composition (`agt_default`), IFC, approval gating, budgets, confidence thresholds, content-hash dedup, drift detection, egress allowlisting, PII pattern matching, and redaction. The Cedar dispatcher (`core/src/cedar.rs`) maps a matched `forbid` to `deny`, plain `Allow` to `allow`, and `Allow` with an `@advice` annotation to `warn`/`escalate`/`transform`. Cedar has documented limits relative to Rego, a tradeoff rather than a bug: no regex (only `like` wildcards), no URL parser, no multi-span transform, integer-only scoring, and no runtime lattice-closure iteration for IFC.

Distinct from the repository-wide formal specs (see section 16), `policy-engine/spec/` holds ACS's own versioned normative spec: `SPECIFICATION.md` (531 lines, 24 numbered sections plus an appendix covering the model, manifests, paths, intervention points, modes, evaluation order, policy input, canonical serialization, tools, annotators, IFC, policies, verdicts, transforms, resource limits, reserved reasons, host obligations, streaming, telemetry, conformance, security, and versioning); the stale `reserved-reasons.json`; and `schema/` (`manifest.schema.json`, `approval.schema.json`, `cedar_advice.schema.json`, `wire/*.schema.json`). An `agt/` subdirectory holds four AGT-layer draft specs (status Draft, version `1.0.0-alpha`): `AGT-EVIDENCE-1.0.md` (proof artefacts for high-assurance dispatchers such as SMT-verified gates and TEE-attested PDPs), `AGT-MANIFEST-1.0.md` ("a strict superset of the ACS manifest"), `AGT-RESOLUTION-1.0.md` (a pure function `resolve_manifest(root, action_path) -> Manifest` running folder discovery/scope/merge host-side), and `AGT-SNAPSHOT-1.0.md` (the JSON snapshot shape AGT hosts build per intervention point, anchored by a mandatory `envelope` block with `agent.id`, `session.id`, `intervention_point`, `timestamp`, `budgets`).

### Deployment modes and Kubernetes/Istio reference

ACS documents itself explicitly as not a sidecar and not a standalone service: `docs/PRODUCTION_DEPLOYMENT.md` calls it "an in-process security runtime for mediated agent paths," with the host owning the agent loop, model calls, tool execution, approval path, networking, credentials, and release process. Only two operational modes exist, driven by `EnforcementMode`: `evaluate_only` (shadow evaluation) and `enforce` (production blocking/transformation). The one concrete infra reference, `deploy/kubernetes/acs-sidecar-reference/`, is explicitly labeled "not a tested cluster recipe" and "not CI verified": a Kustomize bundle showing an app embedding an ACS SDK inside an Istio mesh pod (strict mTLS) alongside an OPA sidecar (loopback `http://127.0.0.1:8181`) and an Envoy sidecar. Its README stresses "ACS is not a sidecar in this topology": enforcement happens inside the app process, while Istio provides only encrypted transport.

### Benchmarks, conformance, and documentation drift

Micro-benchmarks (`core/benches/evaluation.rs`, Criterion) cover policy-input building, manifest `extends` merge, and verdict normalization. An opt-in AgentDojo security benchmark (`benchmarks/agentdojo/`) measures ACS's effect on Attack Success Rate over a banking-agent prompt-injection subset: a deterministic scripted harness shows baseline ASR 1.000 falling to ACS ASR 0.000 with unchanged benign utility (0.800); a live Azure run holds ASR at 0.000 in both conditions. Conformance testing (`tests/conformance/cases/`) uses 25 fixture JSON files mapped to spec sections; `tests/parity/` holds cross-SDK fixtures asserting Rust/Python/Node/.NET agree byte-for-byte; `tests/formal/acs_mediation.qnt` is a Quint model of mediation semantics (see section 16).

Several documentation/implementation divergences are flagged in the repository rather than silently reconciled: the README's claimed rename to `agt_core_engine` and its claimed Go SDK are both unimplemented roadmap text; the README's examples table references directories that do not exist on disk; `spec/reserved-reasons.json` is out of sync with the actual `RuntimeError` enum; and the wire schema `effect.schema.json`, the generator README, and conformance/benchmark naming are vestiges of the pre-transform "effects" model even though the code has migrated to `transform`. The dual `Effect*`/`Transform*` variants in `RuntimeError` are a deliberate backward-compatibility decision, not accidental drift.

---

## 13. Language SDKs and VS Code extension

Beyond the reference Python implementation, the toolkit ships parallel SDKs for TypeScript, .NET, Rust, and Go, plus a VS Code extension. All four SDKs implement a common core (identity, trust, policy, audit, lifecycle, execution rings, kill switch, prompt defense, credential vault, sandbox, MCP security scanning, shadow discovery), but coverage, documentation completeness, and versioning diverge across languages.

### TypeScript SDK (`agent-governance-typescript/`)

Package `@microsoft/agent-governance-sdk` (npm), version `5.0.0`, marked "Public Preview." MIT licensed, pure TS/JS with no native bindings (`@noble/ciphers`, `@noble/curves`, `@noble/ed25519`, `@noble/hashes`, `js-yaml`; `engines.node >= 18.0.0`; built with `tsc`, tested with Jest). Single export map from `src/index.ts`.

Core modules mirror the common surface: `AgentIdentity`/`IdentityRegistry` (Ed25519 DIDs), `TrustManager`, `PolicyEngine`/`PolicyConflictResolver` with YAML loading and pluggable `CedarBackend`/`OPABackend` (fail-closed), `FacetRegistry` for SQL/Kubernetes facet extraction, `AuditLogger` (SHA-256 hash-chained), `LifecycleManager` (8-state: `provisioning -> active <-> suspended/rotating/degraded -> quarantined -> decommissioning -> decommissioned`), `RingEnforcer`/`RingBreachError`, `KillSwitch`, `ShadowDiscovery`, `PromptDefenseEvaluator`, `CredentialVault`/`CredentialInjector` with `DenyReceipt`, `McpSecurityScanner` (`McpThreatType`: `ToolPoisoning`, `Typosquatting`, `HiddenInstruction`, `RugPull`), `DockerSandboxProvider`, and SRE primitives (`CircuitBreaker`, `ErrorBudgetTracker`, `SLOTracker`).

The unified `AgentMeshClient` (`src/client.ts`) ties these together; `executeWithGovernance(action, params)` runs ring enforcement, policy evaluation, audit logging, and trust update, returning a `GovernanceResult`. `GenericFrameworkAdapter` is identity-bound to its client; a mismatched `agentId` on invocation is denied fail-closed (documented breaking-change note). `GovernanceVerifier` produces attestations and can require a `RuntimeEvidence` document (`schema: 'agt-runtime-evidence/v1'`) for supply-chain-style verification. `CascadeContainmentManager` (blast-radius analysis) and `ContextPoisoningDetector` cover multi-agent safety; `OciManifestAdapter` packages agent artifacts as OCI images with an "AI Card" metadata convention.

Distinctive to TypeScript: an E2E encryption layer (AgentMesh Wire Protocol v1.0, `src/encryption/`) implementing a Signal-protocol-style stack (`X3DHKeyManager`, `DoubleRatchet`, `SecureChannel`, `MeshClient`, `RegistryClient`) for agent-to-agent encrypted messaging over WebSocket. No equivalent is documented in the other three SDKs.

Examples (`examples/`): `quickstart.ts`, `policy-eval.ts`, `credential-vault-example.ts`. About 30 test files cover audit, client, discovery, encryption, framework adapter, identity, lifecycle, MCP, policy, prompt defense, rings, and sandbox.

### .NET SDK (`agent-governance-dotnet/`)

Solution `AgentGovernance.sln` with three shippable NuGet packages, versioned `5.0.0` centrally via `Directory.Build.props`, strong-named, SourceLink-enabled, MIT licensed.

| Package | Target | Key dependency |
|---|---|---|
| `Microsoft.AgentGovernance` | net8.0 | `YamlDotNet 18.0.0` |
| `Microsoft.AgentGovernance.Extensions.ModelContextProtocol` | net8.0 | `ModelContextProtocol 1.4.0` |
| `Microsoft.AgentGovernance.Extensions.Microsoft.Agents` | net8.0 | `Microsoft.Agents.AI 1.10.0` |

`GovernanceKernel` is the primary facade; `GovernanceOptions` configures policy paths, `ConflictStrategy` (`DenyOverrides`, `AllowOverrides`, `PriorityFirstMatch`, `MostSpecificWins`), audit, metrics, rings, prompt-injection detection, and circuit breaker. Key method: `EvaluateToolCall(agentId, toolName, args)`. Namespaces include `Policy` (`OpaPolicyBackend`, `CedarPolicyBackend`), `RateLimiting` (sliding-window, YAML `"100/minute"` syntax), `Hypervisor` (`ExecutionRings.ComputeRing(trustScore)`: Ring0 >= 0.95 full access, Ring1 >= 0.80 write+network 1000 calls/min, Ring2 >= 0.60 limited write 100 calls/min, Ring3 < 0.60 read-only 10 calls/min; `KillSwitch`; `SagaOrchestrator` with automatic reverse-order compensation), `Lifecycle` (same 8-state machine), `Sre` (`CircuitBreaker`, `SloEngine`), `Security` (`PromptInjectionDetector`, 7 attack types; `PromptDefenseEvaluator`, 12 defense vectors matching "the Python prompt-defense reference"; `CredentialVault`), `Discovery`, `Audit` (pub-sub via `kernel.OnEvent`), `Telemetry` (documented p99 evaluation latency < 0.1ms), `Mcp`, and `Sandbox` (`ISandboxProvider`, `DockerSandboxProvider`).

**Documented divergence**: `AgentIdentity.CreateAsymmetric()` uses native ECDSA P-256 rather than the Ed25519 used by TS/Rust/Go; the README states cross-language key material is not interchangeable.

The MCP extension adds `IMcpServerBuilder.WithGovernance(options)`, requiring an authenticated agent identity by default (`RequireAuthenticatedAgentId = true`); `context.Items["agent_id"]` is no longer trusted for identity resolution (breaking-change note). The Microsoft Agents extension adds `AIAgentBuilder.WithGovernance(...)`, wrapping (not replacing) an existing agent; function middleware maps tool calls to `EvaluateToolCall` and sets `FunctionInvocationContext.Terminate = true` on deny, and only works if the underlying pipeline supports function invocation middleware.

Examples: `examples/Quickstart/`, `examples/AspNetMiddleware/`. The README enumerates OWASP Agentic AI Top 10 coverage with named mitigations (a documentation artifact, not a certification), and cross-links to the Python `maf_adapter.py` and Azure Foundry deployment docs, implying the Python MAF adapter is more complete for that scenario.

### Rust SDK (`agent-governance-rust/`)

Cargo workspace, members `agentmesh` and `agentmesh-mcp`, `workspace.package.version = "5.0.0"`, edition 2021, `rust-version = "1.89"`. Dependencies are exact-pinned (`cedar-policy = "=4.11.2"`, `regorus = "=0.10.1"`, `ed25519-dalek = "=2.2.0"`, `opentelemetry = "=0.32.0"`, `aes-gcm = "=0.10.3"`). **Version drift**: the workspace declares `5.0.0`, but `agentmesh/README.md` code samples still reference crate versions `3.5.0`/`3.7.0`, and a `#[deprecated(since = "3.5.0", ...)]` attribute exists on the `mcp` re-export module.

`agentmesh` (crates.io, "Public Preview") gates its CLI and telemetry behind feature flags `cli` (pulls `clap`, builds the `agt` operator binary) and `telemetry` (pulls `opentelemetry`); the default build has zero CLI/OTel dependencies. `agentmesh-mcp` is a standalone MCP crate, "the canonical MCP implementation"; `agentmesh::mcp` is now only a `#[deprecated]` compatibility re-export (issue #2013), scheduled for removal.

The feature-gated `agt` CLI is unique to Rust among the four SDKs (TS, .NET, and Go have no bundled CLI binary in this area). Subcommands: `agt check --policy <path> --input '<json>'`, `agt policy validate|explain`, `agt audit tail|export`, `agt trust show|set` (score range 0..=1000).

Documented modules mirror the other SDKs: `policy.rs` (allow/deny/requires-approval/rate-limit decision types), `trust.rs` (integer scoring 0-1000 across 5 tiers: `VerifiedPartner` 900-1000, `Trusted` 700-899, `Standard` 500-699, `Probationary` 300-499, `Untrusted` 0-299), `audit.rs`, `identity.rs` (Ed25519), `rings.rs` (`Admin`=0, `Standard`=1, `Restricted`=2, `Sandboxed`=3), `lifecycle.rs`, `prompt_injection.rs` (`Sensitivity`: `Strict`/`Balanced`/`Permissive`; fails closed on malformed overrides; audit log is bounded, hash-only, never persisting raw prompt/canary/blocklist content), `credential_vault.rs`, `sandbox.rs`, `protocol_facets.rs`, and `telemetry.rs` (span `agentmesh.policy.evaluate`, attributes sanitized to decision/allowed/elapsed/hashes; README notes "Prometheus metrics and broader audit/trust/prompt/ring telemetry remain follow-up scope").

**Largest documentation-vs-surface gap found among the four SDKs**: the `agentmesh` README's API table omits roughly ten `pub`, re-exported modules:

- `control_support.rs`: `CircuitBreaker`, `ErrorBudget`, `KillSwitch`, `SloEngine` (bundled together, unlike the separate top-level modules in other SDKs).
- `governance_support.rs` (largest module): `ComplianceEngine`, `EUAIActRiskClassifier`, `AnnexIVDocument` (EU AI Act Annex IV documentation generation), `CedarEvaluator`, `OPAEvaluator`, `FederationEngine`, `OrgPolicy`, `TrustPolicy`, `SignedAuditEntry`/`HashChainVerifier`.
- `identity_support.rs`: `AgentDID`, `SPIFFEIdentity`/`SVID`, `MTLSIdentityVerifier`, `PKCS11KeyStore`, `KeyRotationManager`, `RevocationList`: a zero-trust identity surface undocumented in the crate README.
- `integration_support.rs`: shadow discovery types, `FrameworkAdapter`, `GovernanceHook`/`Middleware`, `PromptDefenseEvaluator`.
- `reward_support.rs`: `RewardEngine`, `RewardDistributor`, and strategies (`EqualSplitStrategy`, `ContributionWeightedStrategy`, `HierarchicalStrategy`, `TrustWeightedStrategy`): a reward-distribution layer present in no other SDK's public README.
- `trust_support.rs`: `CapabilityRegistry`, `TrustedAgentCard`, `TrustHandshake`: an agent-to-agent trust handshake and capability-card protocol.

`agentmesh-mcp` (`src/mcp/`) provides `McpGateway`, `McpSlidingRateLimiter`, `CredentialRedactor`, `McpSessionAuthenticator`, `McpMessageSigner`. `McpGateway` requires session-backed authentication (`process_authenticated_request`); the legacy `process_request` path is removed and fails closed (breaking migration documented in both crate READMEs). Examples (`agentmesh/examples/`): `quickstart.rs`, `audit-logging.rs`, `normalize_b64.rs`.

### Go SDK (`agent-governance-golang/`)

Module `github.com/microsoft/agent-governance-toolkit/agent-governance-golang`, `go 1.25`, single external dependency `gopkg.in/yaml.v3 v3.0.1`. The `packages/agentmesh/doc.go` comment states `// Version: v3.1.0`, matching the actual git tag `agent-governance-golang/v3.1.0`. **This confirms a real, verifiable version-skew gap**: the Go module's published version genuinely lags the monorepo-wide `5.0.0` bump (commit `e5693cb1`) that TS, .NET, and Rust picked up.

Single package `agentmesh`; no CLI binary or `cmd/` directory exists. Documented modules: `client.go` (`AgentMeshClient`, `ExecuteWithGovernance`), `identity.go`, `trust.go` (`VerifyPeer` fails closed unless independent verification evidence is available), `policy.go`/`policy_backends.go` (`LoadRego`, `LoadCedar`, both fail-closed), `audit.go` (`GetEntries` returns clones, not references), `mcp.go` (same 4 threat categories as TS), `rings.go`, `kill_switch.go` (scope constructors `GlobalKillSwitchScope`, `AgentKillSwitchScope`, `CapabilityKillSwitchScope`), `lifecycle.go`, `slo.go`, `middleware.go` (`NewHTTPGovernanceMiddleware` fails closed unless `AgentIDResolver` returns a verified identity; `LegacyTrustedHeaderAgentIDResolver` is an explicit, discouraged migration bridge surfacing caller-asserted headers only as `caller_asserted_agent_id`), `discovery.go` (the only SDK whose shadow-discovery surface includes a GitHub-repository-contents-API scan mode via `ScanGitHubRepositories`), and `promptdefense.go`.

Not documented in the README but present and exported: `credential_vault.go`, `sandbox.go`, and `protocol_facets.go` (SQL comment stripping, statement splitting, CTE-inner-verb detection): a real but smaller documentation gap than Rust's.

Examples: 14 directories under `examples/`, including `audit-chain`, `execution-rings`, `full-stack`, `http-middleware-fail-closed`, `kill-switch-scopes`, `mcp-scan` (a homoglyph/Cyrillic typosquatting test case), `policy-opa-cedar`, `shadow-discovery`, `slo-tracking`, and `trust-scoring`, each with its own `main.go` and README.

### VS Code extension: `agent-os-vscode`

Located inside `agent-governance-typescript/agent-os-vscode/`. Package `@microsoft/agent-os-vscode` (Marketplace publisher `agent-os`), version `5.0.0`, MIT licensed, `engines.vscode ^1.85.0`, runtime dependencies pinned exact (`axios 1.16.0`, `ws 8.21.0`).

Despite living in the TypeScript SDK's directory, the extension is not a thin wrapper over `@microsoft/agent-governance-sdk`. Its richest live-data features (SLO dashboard, agent topology graph, policy compliance snapshot) depend on a separate Python package, **`agent-failsafe`** (`pip install agent-failsafe[server]`), auto-detected and auto-installable as a local REST server on `127.0.0.1:9377`. If unavailable, the extension falls back to a `disconnected`/`not-installed` state with empty snapshots rather than failing: a fail-soft UX choice distinct from the SDKs' fail-closed governance semantics.

Local, SDK-independent features: real-time regex/pattern-based policy checks (destructive SQL, file deletes, secret exposure, privilege escalation, optional unsafe network calls) in `src/policyEngine.ts`, running client-side in "basic" mode without calling into the TypeScript SDK's `PolicyEngine`.

CMVK (Cross-Model Verification Kernel, `src/cmvkClient.ts`) is an opt-in multi-model code review feature calling `https://api.agent-os.dev/cmvk` (default models `["gpt-4", "claude-sonnet-4", "gemini-pro"]`, consensus threshold 0.8), falling back to a local mock reviewer when unreachable; this is a cloud dependency, and the endpoint/model list should be read as example configuration rather than a verified working public service.

The extension exposes roughly 30 commands (review, audit log, policy editor, workflow designer, SLO/topology webviews, SSO sign-in, CI/CD setup, governance hub, kernel debugger, memory browser) and about 25 configuration settings, including `agentOS.mode` (`basic`|`enhanced`|`enterprise`), `agentOS.enterprise.sso.*` (azure/okta/google/github), and `agentOS.enterprise.compliance.framework` (soc2/gdpr/hipaa/pci-dss, a template selection, not a certification). The UI includes a 3-slot configurable sidebar, detail panels (SLO, Topology, Hub, Kernel Debugger, Memory Browser, Audit, Policy), a Policy Editor, a drag-and-drop Workflow Designer, and an Onboarding walkthrough. A separate `GovernanceServer` can serve dashboards in an external browser with documented hardening: session-token-authenticated WebSocket, HTTP rate limiting, locally vendored D3.js/Chart.js, nonce-only CSP, loopback-only binding, HTML-escaping, and Python-path validation against shell metacharacters before subprocess spawn.

Enterprise features (SSO, RBAC, CI/CD config generation, compliance templates) are client-side scaffolding; nothing in the reviewed code constitutes a verified working third-party SSO handshake. Language services provide real-time governance diagnostics and quick-fix code actions for Python/TypeScript/YAML, separate from the core SDK. Requirements: VS Code >= 1.85.0, Node.js 18+, Python 3.10+ for the `agent-failsafe` backend.

### Cross-SDK observations

- **Version governance**: TS, .NET, Rust, and the VS Code extension are all synchronized at `5.0.0`. The Go module is the outlier, still tagged `v3.1.0`, trailing by multiple releases.
- **Identity algorithm parity gap**: TS, Rust, and Go use Ed25519 for agent identity signing; .NET uses native ECDSA P-256, and cross-language key material is documented as not interchangeable.
- **CLI parity gap**: only Rust ships an operator CLI (`agt`, feature-gated).
- **README-vs-surface documentation gaps**, ranked by severity: Rust's `agentmesh` README omits roughly ten `pub` modules covering EU AI Act compliance tooling, SPIFFE/mTLS/PKCS11 identity, and a reward-distribution engine. Go's README omits `credential_vault.go`, `sandbox.go`, and `protocol_facets.go` (smaller but real). TypeScript's and .NET's READMEs are comparatively complete.
- **MCP governance model convergence**: TS, Rust, and .NET recently moved in step from a caller-asserted/anonymous-friendly MCP agent-identity model to a fail-closed, session- or claims-authenticated model, each with an explicit migration note (see section 11 for MCP governance details).
- **VS Code extension's Python dependency**: the extension's richest live data is sourced from the Python `agent-failsafe` package via a local REST server, not from the TypeScript SDK in the same repository directory; it should not be treated as simply "the TypeScript SDK's UI."

---

## 14. Coding-agent surfaces

The Agent Governance Toolkit ships four sibling "governance surface" packages that adapt the same policy model to four agentic coding hosts: Claude Code, GitHub Copilot CLI, OpenCode, and Google's Antigravity CLI. All are versioned `5.0.0`, licensed MIT, authored by Microsoft Corporation, and marked "Public Preview" (APIs and policy schema may change). Each depends on `@microsoft/agent-governance-sdk`, pins transitive `js-yaml` to `4.2.0` via npm `overrides`, and builds its policy engine on four SDK primitives: `PolicyEngine`, `ContextPoisoningDetector`, `McpSecurityScanner`, `PromptDefenseEvaluator`.

### Claude Code plugin: `@microsoft/agent-governance-claude-code`

Version `5.0.0`, single dependency `@microsoft/agent-governance-sdk@4.0.0`, `node >= 22.0.0`. In the repo the plugin lives at `agent-governance-claude-code/` and is registered in `.claude-plugin/marketplace.json` as plugin `agt-governance` (also `5.0.0`, category `security`). Local install: `cd agent-governance-claude-code && npm install`, then `claude --plugin-dir ./agent-governance-claude-code`.

**Bootstrap and hooks.** `bin/agt-node` (a POSIX shell shim) and `bin/agt-node.cmd` wrap every Node invocation so `.mcp.json` and `hooks/hooks.json` can call it consistently across platforms. `hooks/hooks.json` wires three events, each a 30-second-timeout command hook:

| Event | Script | Enforces |
|---|---|---|
| `SessionStart` | `hooks/session-start.mjs` | injects governance context (mode, policy source, prompt-defense grade); does not block |
| `UserPromptSubmit` | `hooks/user-prompt-submit.mjs` | evaluates prompt against poisoning backend; `deny`/`review` both map to `decision: "block"` (Claude's hook contract has no "ask") |
| `PreToolUse` | `hooks/pre-tool-use.mjs` | evaluates tool calls; `deny` → `permissionDecision: "deny"`, `review` → `permissionDecision: "ask"` (native confirmation UX) |

No `PostToolUse` hook is registered; the README calls this a deliberate parity gap ("`PostToolUse` in Claude cannot reliably redact tool output after the tool has already executed"). Every hook reads JSON from stdin, writes JSON to stdout, and on any thrown error exits 2, the fail-closed pattern used throughout.

**Policy engine (`lib/policy.mjs`, 1,158 lines).** Policy resolves in order: `AGT_CLAUDE_POLICY_PATH` env var, `~/.claude/agt/policy.json`, bundled `config/default-policy.json`, then an in-code `createMinimalFallbackPolicy()` (enforce mode, `defaultEffect: "review"`) if even the bundled file fails to parse. `compilePolicy()` normalizes `mode` (default `"enforce"`), `schemaVersion` (capped at `1`), `denyOnPolicyError` (default `true`), `minimumPromptDefenseGrade` (default `"B"`), and prepends 10 fixed "production guard" context lines (anti-injection, anti-exfiltration, role-stability) ahead of any custom `additionalContext`.

Four evaluation backends register against the SDK's `PolicyEngine`: `agt-command-patterns` (regex-matches `blockedToolCalls` against extracted command text), `agt-direct-resources` (walks tool arguments recursively, classifying string leaves as URLs or paths against `pathRules`/`urlRules`), `agt-prompt-poisoning` (feeds prompt text through a per-call `ContextPoisoningDetector` seeded with `poisoningPatterns`, only on `prompt.submit`), and `agt-mcp-scan` (runs `McpSecurityScanner.scan()` against command text and serialized args for every `tool.*` action).

In `advisory` mode, `decisionFromSeverity` always returns `allow` (findings surface only as `AGT advisory: ...` context text); in `enforce` mode, `critical`/`high` severity denies, `medium` requests review. Two narrow bypass exemptions exist: `recursive-delete` rules permit `rm -rf` against a fixed safe-cleanup allowlist (`node_modules`, `dist`, `build`, `.next`, `target`, `__pycache__`, `.pytest_cache`, `.venv`, `venv`, `coverage`, `.turbo`, `out`), and `secret-read` rules permit reading `.env.example`/`.sample`/`.template` files.

**Default bundled policy** (`config/default-policy.json`): `mode: "enforce"`, `defaultEffect: "review"`; `allowedTools` limited to `Read`, `Glob`, `Grep`, and the two MCP status/check tools; `reviewTools` covers `Bash`, `WebFetch`, `WebSearch`, `Write`, `Edit`, `MultiEdit`. Three `blockedToolCalls` rules (all `tool: "Bash"`, `effect: "deny"`): `recursive-delete`, `dangerous-bootstrap` (curl/wget piped to shell, cloud metadata IP/hostname access), and `secret-read` (`.env*`, SSH keys, cloud credential directories, `.npmrc`, `.docker/config.json`, `.kube/config`, environment dumps). `directResourcePolicies` add path rules for credential reads (deny) and persistence writes to shell profiles/git hooks (review), plus a URL rule denying cloud metadata endpoints. Two `poisoningPatterns` (both `severity: "critical"`) match "ignore previous instructions" style language and system/developer prompt exfiltration requests.

**Audit log** (`lib/audit.mjs`): a hash-chained JSON ledger at `~/.claude/agt/audit-log.json`, each entry `{timestamp, agentId, action, decision, previousHash, hash}` (SHA-256 of the prior fields), genesis hash 64 zeros, capped at 10,000 entries, written atomically. A broken hash chain causes new governance decisions to fail closed. The same hash-chained pattern recurs across all four surfaces.

**MCP server** (`server/agt-mcp.mjs`, 233 lines): a hand-rolled JSON-RPC 2.0 stdio server (no MCP SDK dependency), registered as `agt_governance`, exposing two tools: `agt_policy_status` (no params) and `agt_policy_check_text` (requires string `text`), both returning JSON via `getPolicyStatus()`/`checkArbitraryText()`.

**Slash commands**: `/agt-governance:agt-status` and `/agt-governance:agt-check`, thin markdown wrappers that call the corresponding MCP tool once and print the JSON result verbatim.

**Tests**: four suites under `node --test`, 17 cases, covering end-to-end hook spawning, the JSON-RPC server, policy unit behavior (including corrupt-audit-log fail-closed handling), and a repo-root CI script (`scripts/sync-claude-marketplace-version.mjs`) that keeps the plugin's version in sync across `package.json`, `.claude-plugin/plugin.json`, and the root marketplace manifest.

### Copilot CLI: `@microsoft/agent-governance-copilot-cli`

Package `@microsoft/agent-governance-copilot-cli`, version `5.0.0`, Node `>=22.0.0`, dependency `@microsoft/agent-governance-sdk@4.0.0`. Bin entry `agt-copilot`. This is the production install surface, distinct from a tutorial reference implementation at `examples/copilot-cli-agt` (see section 19).

Install:
```
npx @microsoft/agent-governance-copilot-cli install
npx @microsoft/agent-governance-copilot-cli update [--force-policy]
```
`installPackage()` (`lib/cli.mjs`, 877 lines) resolves Copilot home (`--copilot-home` flag, `COPILOT_HOME` env, else `~/.copilot`), copies `assets/extensions/agt-global-policy/` into it via stage-then-atomic-rename, seeds `agt/policy.json` from the bundled default only if absent (or `--force-policy`), and refuses to overwrite a pre-existing non-AGT extension unless `--replace-unmanaged` is passed. Ownership is tracked via `.agt-install-manifest.json`. The installer vendors the entire SDK dependency tree into the extension's `vendor/` directory so it is self-contained. It does not edit Copilot's `settings.json`; `doctor` only reports whether `experimental`/`experimental_flags: ["EXTENSIONS"]` are set.

CLI commands: `install`, `update`, `policy apply --file <path> | --profile <strict|balanced|advisory>`, `policy validate`, `policy path`, `policy show`, `uninstall [--remove-policy]`, `doctor [--json]`.

Applying a custom (non-bundled) policy is validated against a hardened baseline (`validatePolicyBaseline()`): `mode` must be `enforce`, `denyOnPolicyError` must be `true`, `toolPolicies.defaultEffect` must be `review`, `allowedTools` cannot contain `"*"`, `minimumPromptDefenseGrade` must be at least `B`, deny rules must exist for cloud metadata endpoints and credential/secret reads, and `scanOutputTools` must include `powershell`, `bash`, `read_powershell`, `list_powershell`. Bundled profiles (`strict.json`, `balanced.json`, `advisory.json`) are exempt via canonical-JSON comparison against the shipped files.

The installed extension is a native Copilot CLI extension loaded via `extension.mjs`, importing `joinSession` from the host-provided `@github/copilot-sdk/extension`. It registers lifecycle hooks `onSessionStart`, `onUserPromptSubmitted`, `onPreToolUse`, `onPostToolUse` (via `inspectToolResult`), `onSessionEnd`, plus a `/agt` slash command (`status`, `reload`, `check "<text>"`, `help`) and the same two MCP-style tools exposed in-process, not via a JSON-RPC MCP server as in the other three surfaces.

The default policy (`config/default-policy.json`, 315 lines) allows `view`, `glob`, `rg`; reviews `powershell`, `bash`, `curl`, `web_fetch`, `fetch`, `browser`, `web_search`; suppresses output for web/fetch tools and treats `bash`/`powershell` output advisory-only. It carries `blockedToolCalls` for `recursive-delete`, `dangerous-bootstrap` (`iex`/`Invoke-Expression`, `-EncodedCommand`, `certutil`/`bitsadmin`), `secret-read` (also covering `gh auth token`, `az account get-access-token`, `kubectl config view --raw`, OS credential-store lookups), and `persistence-write`, plus 14 named `poisoningPatterns` covering injection phrasing, exfiltration, guardrail-disable requests, and role-confusion markers.

Tests: `test/policy-engine.test.mjs` (12 tests) and `test/install.test.mjs` (6 tests), 18 total.

### OpenCode: `@microsoft/agent-governance-opencode`

Package `@microsoft/agent-governance-opencode`, version `5.0.0`, Node `>=22.0.0`, `main: src/index.mjs`, dependency `@microsoft/agent-governance-sdk@3.7.0` (one major behind the Copilot CLI and Antigravity CLI packages). No `bin` field and no dedicated installer CLI. The package's `AGENTS.md` (66 lines) states it is "pinned to 3.6.0 to track the Claude Code package," a stale version string relative to the actual `5.0.0` package version; the MCP server's own `VERSION` constant is likewise hardcoded to `"3.6.0"`.

It is consumed as an OpenCode plugin three ways: an npm-specifier `plugin` entry in `opencode.json`, a workspace-local plugin file re-exporting `src/index.mjs`, or a bundled stdio MCP server wired under `opencode.json`'s `mcp.agt-governance`. Policy resolves via `AGT_OPENCODE_POLICY_PATH` env var, then `./.agt/policy.json`, then `~/.config/opencode/agt/policy.json`, then the bundled default.

`src/index.mjs` (180 lines) exports `AgtGovernance`, an async plugin factory implementing: `session.created` (best-effort status log), `event` (inspects `message.part.updated` text parts and **throws** on `deny`, a known limitation the code comments flag as "not the way to go" since throwing silently breaks the session), `tool.execute.before` (throws on `deny`; on `review`, mutates `output.args.__agt_review_reason` as a hint, since OpenCode exposes no server-side "ask" decision from a plugin hook), and `tool.execute.after` (if `result.redact` is true, overwrites output with redacted text, a parity win over Claude Code's inability to rewrite tool output).

Secret redaction uses a `SECRET_PATTERNS` table of 7 detectors (`aws-access-key`, `github-token`, `github-fine-grained`, `openai-key`, `azure-account-key`, `private-key-block`, `jwt-token`); matches are replaced with `[AGT_REDACTED:<pattern-id>]` in enforce mode, reported only in advisory mode. The default policy uses OpenCode's lowercase tool names: `allowedTools` are `read`, `glob`, `grep`, `list`; `reviewTools` are `bash`, `webfetch`, `websearch`, `write`, `edit`, `patch`, `multiedit`.

The MCP server (`server/agt-mcp.mjs`, 233 lines, also exported as `./mcp-server`) is a dependency-free stdio JSON-RPC server exposing `agt_policy_status` and `agt_policy_check_text`. Tests span three files, 25 cases: `policy.test.mjs` (10), `plugin.test.mjs` (10), `mcp-server.test.mjs` (5).

### Antigravity CLI: `@microsoft/agent-governance-antigravity-cli`

Package `@microsoft/agent-governance-antigravity-cli`, version `5.0.0`, Node `>=20.19.0` (lower floor than the other two CLI packages), dependency `@microsoft/agent-governance-sdk@4.0.0`. Bin entry `agt-antigravity`.

Install:
```
npm install -g @microsoft/agent-governance-antigravity-cli
agt-antigravity install
```
Installer architecture mirrors Copilot CLI: staged copy plus atomic rename, `.agt-install-manifest.json` ownership tracking, vendored SDK integrity checks (`VENDORED_RUNTIME_CHECKS`), and `--replace-unmanaged` semantics. `ANTIGRAVITY_CLI_HOME` overrides the install root (default `~/.antigravity`). CLI commands: `install`, `update`, `policy <apply|validate|path|show>`, `uninstall [--remove-policy]`, `doctor [--json]`.

The extension is declared via `antigravity-extension.json` (internal `version` field reads `3.3.0`, stale relative to the `5.0.0` package). It registers an MCP server (`agt_global_policy`), a plan directory, and two settings with env overrides: `AGT_ANTIGRAVITY_POLICY_PATH` and `AGT_ANTIGRAVITY_AUDIT_PATH`.

`hooks/hooks.json` wires four subprocess hooks (30-second timeout each): `SessionStart`, `BeforeAgent`, `BeforeTool` (matcher `.*`), `AfterTool` (matcher `.*`), built on a shared `lib/hook-runtime.mjs` (74 lines). Because Antigravity's subprocess hook model cannot pause for approval, enforce-mode policy treats `review` as `deny` unconditionally, confirmed by a dedicated test: "`evaluatePreToolUse` denies review-only tools because Antigravity hooks cannot pause for approval." `AfterTool` can only suppress output wholesale (`suppressOutput: true`), not redact in place, unlike OpenCode's rewrite capability.

Slash commands are TOML prompt macros: `commands/agt/status.toml` (`/agt:status`) and `commands/agt/check.toml` (`/agt:check {{args}}`), both instructing the model not to invent values or findings. `ANTIGRAVITY.md` (10 lines) is the always-loaded context banner warning against untrusted tool output and hidden-instruction compliance.

The MCP server (`mcp/server.mjs`, 225 lines) is `Content-Length`-framed only (unlike OpenCode's server) and returns a richer `agt_policy_status` payload combining `formatPolicySummary()` and `getPolicyStatus()`. Tests span four files, 22 cases, including install-time integrity checks absent from the Copilot CLI suite ("install failing when installed runtime dependencies drift from package-lock metadata").

### Cross-surface patterns

All four packages share near-identical `lib/policy.mjs`, `lib/poisoning.mjs`, and (where present) `lib/sdk-loader.mjs` structures and function names (`evaluatePreToolUse`, `evaluatePromptSubmission`, `inspectToolResult`, `SUPPORTED_POLICY_SCHEMA_VERSION`), but each package maintains its own copy rather than importing a shared library. Enforcement UX differs by host contract: Claude Code and Copilot CLI map `review` to a native "ask" confirmation; OpenCode has no such hook and only annotates a review hint unless the operator hardens `defaultEffect` to `deny`; Antigravity CLI collapses `review` to `deny` unconditionally. Output handling also differs: OpenCode can rewrite/redact tool output in place, while Antigravity CLI and Claude Code can only suppress or block output wholesale. A recurring version-drift pattern appears across surfaces, where inner extension/server metadata versions (`3.3.0`, `3.6.0`) lag the outer `5.0.0` npm package version.

---

## 15. Framework integrations

Framework and platform adapters for AgentMesh (AGT's identity and trust subsystem, see section 5) live in `agent-governance-python/agentmesh-integrations/`. The README scopes this tree to "platform plugins & trust providers," as distinct from "framework adapters" that wrap governance kernels (LangChainKernel, CrewAIKernel), said to belong under `agent-os/src/agent_os/integrations`. In practice, most of the LangChain/CrewAI/LangGraph/etc. code implementing AgentMesh's trust layer (identity, `TrustGatedTool`, `TrustPolicy`) physically resides here.

Nearly every Python sub-package declares `version = "5.0.0"` with a description of the form "Deprecated. Previously published as X. Install agent-governance-toolkit-integrations[extra] instead," a `dependencies` entry pointing at `agent-governance-toolkit-integrations[<extra>]>=4.1.0,<6.0"`, and an `__init__.py` emitting a `DeprecationWarning` toward `docs/package-consolidation/MIGRATION.md`. These directories are legacy shims; their current source is force-included into the sibling `agent-governance-toolkit-integrations` wheel via extras (`langchain`, `crewai`, `openai-agents`, `langgraph`, `llamaindex`, `haystack`, `pydantic-ai`, `flowise`, `langflow`, `adk`, `avp`, `cedarling`, `nostr-wot`, `structural-authz`, `openshell`, `audit-export`, `dev`). Packages NOT treated as deprecated shims: `audit-accountability-export`, `mcp-receipt-governed`, `mcp-trust-proxy`, `a2a-protocol`, `template-agentmesh` (starter template), and TypeScript packages `copilot-governance` and `mastra-agentmesh`.

The README advertises `dify-plugin/`, `moltbook/`, and `openclaw-skill/` as "Stable/Published," but none of these three directories exist in the checkout (aspirational or removed). Only `dify/` (archived Flask middleware) is present.

### Adapter enumeration

| # | Directory | Module | Integration style | Status / notes |
|---|---|---|---|---|
| 1 | `langchain-agentmesh/` | `langchain_agentmesh` | `identity.py`, `trust.py` (7 classes incl. `TrustedAgentCard`, `TrustPolicy`, `DelegationChain`), `tools.py` (`TrustGatedTool`, `TrustedToolExecutor`), `callbacks.py` (`TrustCallbackHandler`, subclasses `BaseCallbackHandler`) | Stable; most-used adapter, backs `notebooks/04_langchain_agentmesh_chatbot.ipynb`; 51 tests |
| 2 | `langgraph-trust/` | `langgraph_trust` | `identity.py`, `gate.py` (`TrustScoreTracker`, `TrustGate` checkpoint node), `state.py`, `policy.py`, `edges.py` (`trust_edge()`, `trust_router()`) | Published on PyPI as `langgraph-trust`; largest suite: 57 tests / 4 files |
| 3 | `crewai-agentmesh/` | `crewai_agentmesh` | Single `trust.py`: `AgentProfile`, `CapabilityGate`, `TrustTracker`, `TaskAssignment`, `TrustedCrew` (wraps a CrewAI crew) | 34 tests |
| 4 | `openai-agents-trust/` | `openai_agents_trust` | `identity.py`, `trust.py`, `policy.py`, `audit.py`, `guardrails.py` (`trust_input_guardrail()`, `policy_input_guardrail()`), `handoffs.py` (`trust_gated_handoff()`), `hooks.py` (`GovernanceHooks(RunHooksBase)`) | Published on PyPI; 51 tests / 2 files |
| 5 | `openai-agents-agentmesh/` | `openai_agents_agentmesh` | Single `trust.py`: `AgentTrustContext`, `HandoffResult`, `HandoffVerifier`, `FunctionCallResult`, `TrustedFunctionGuard` | Older, simpler sibling of #4, same target SDK |
| 6 | `llamaindex-agentmesh/` | `llama_index/agent/agentmesh/` (namespace package) | `identity.py`, `trust.py`, `worker.py` (`TrustedAgentWorker(BaseAgentWorker)`), `query_engine.py` (`TrustGatedQueryEngine(BaseQueryEngine)`, `DataAccessPolicy`) | Merged upstream as `llama-index-agent-agentmesh`; own `__version__ = "3.2.2"`; no `tests/` in this checkout |
| 7 | `haystack-agentmesh/` | `haystack_agentmesh` | `governance.py` (`GovernancePolicyChecker`), `trust_gate.py` (`AgentTrustRecord`, `TrustGate`), `audit.py` (hash-chained `AuditEntry`/`AuditLogger`) | 31 tests / 3 files |
| 8 | `adk-agentmesh/` | `adk_agentmesh` | Google Agent Development Kit adapter; `audit.py`, `evaluator.py` (`Verdict`, `ADKPolicyEvaluator`), `governance.py` (`GovernanceCallbacks`) | Pins `google-adk>=0.1.0,<1.0`; 1 test file |
| 9 | `agentmesh-avp/` | `agentmesh_avp` | Amazon Verified Permissions trust provider; `provider.py` (`AVPProvider`, `_is_valid_did()`) | No extra third-party dependency in consolidated extras |
| 10 | `cedarling-agentmesh/` | `cedarling_agentmesh` | Bridges Cedar policy engine; `backend.py` (`CedarlingBackend`, `_tool_to_cedar_action()`, `_validate_tokens()`) | Breaking change: `tokens` moved to per-request `evaluate()`, `timeout_seconds` removed (in-process `cedarling-python`); pins `cedarpy>=4.0.0,<5.0` |
| 11 | `structural-authz-agentmesh/` | `structural_authz_agentmesh` | Single ~660-line `trust.py`: `TrustGrade`, `TrustArtifact` (Ed25519-signed), `DelegationLink`/`DelegationChain`, `AuthzGate`, `generate_keypair()` | Richest single-file trust model; `did:`-prefixed identifiers; 2 test files |
| 12 | `nostr-wot/` | `agentmesh_nostr_wot` | `provider.py` (`NostrWoTProvider`); bridges Nostr NIP-85 Web-of-Trust scores into `TrustEngine(external_providers=[...])` | Marked "scaffold," not production-complete; 1 test file |
| 13 | `pydantic-ai-governance/` | `pydantic_ai_governance` | `audit.py`, `intent.py` (`classify_intent()`), `decorator.py` (`govern()` decorator), `policy.py`, `trust.py`, `toolset.py` (`GovernanceToolset`) | Pins `pydantic-ai>=0.0.10,<1.0`; 1 test file |
| 14 | `openshell-skill/` | `openshell_agentmesh` | Shell command interception: `cli.py` (console script `openshell-agentmesh`), `skill.py` monkeypatches `subprocess.run/Popen`, `os.system/popen` behind `governed_shell()` | Architecturally distinct from framework adapters; 1 test file |
| 15 | `template-agentmesh/` | `template_agentmesh` | Starter scaffold: `trust.py` exports `ActionGuard`, `AgentProfile`, `TrustTracker`; documented in Tutorial 28 (`docs/tutorials/28-build-custom-integration.md`) | Not deprecated; requires implementing `agentmesh.trust.TrustProvider` |
| 16 | `flowise-agentmesh/` | `flowise_agentmesh` | Visual flow nodes: `audit_node.py`, `governance_node.py`, `policy.py`, `rate_limiter_node.py`, `trust_gate_node.py` | No extra deps; 1 test file |
| 17 | `langflow-agentmesh/` | `langflow_agentmesh` | Visual components: `audit_logger.py`, `compliance_checker.py`, `governance_component.py`, `trust_router.py` | No extra deps; 1 test file |
| 18 | `dify/` | Flask middleware | `identity.py`, `middleware.py`, `trust.py`; `TrustMiddleware`/`VerificationIdentity` Ed25519 DIDs, trust scoring, audit logging | Archived: Dify upstream PR #32079, closed 2026-02-07, resubmit guidance to `langgenius/dify-plugins`; 0 test files |
| 19 | `mcp-trust-proxy/` | `mcp_trust_proxy` | `proxy.py`: `ToolPolicy`, `AuthResult`, `TrustProxy`, wraps any MCP tool with AgentMesh trust verification | No runtime dependencies; 1 test file |
| 20 | `mcp-receipt-governed/` | `mcp_receipt_governed` | `adapter.py` (`CedarPolicyEvaluator`, `McpReceiptAdapter`), `receipt.py` (`GovernanceReceipt`, `sign_receipt()`, `verify_receipt_chain()`, `ReceiptStore`) | Optional `crypto` extra; `scripts/verify_receipts.py`; 2 test files |
| 21 | `audit-accountability-export/` | `audit_accountability_export` | `export.py`: `canonical_sha256()`, `audit_entry_to_accountability_export()`, `accountability_export_to_eeoap_statement()` | Example mapping `AuditEntry` to an external accountability export shape; 1 test file |
| 22 | `a2a-protocol/` | `a2a_agentmesh` | Google Agent2Agent (A2A) bridge: `agent_card.py` (`AgentCard`), `task.py` (`TaskState`, `TaskEnvelope`), `trust_gate.py` | 1 test file |
| 23 | `copilot-governance/` | TypeScript, `@microsoft/agentmesh-copilot-governance` | GitHub Copilot Extension for code review: `reviewer.ts` (`reviewCode`), `policy-validator.ts`, `agent.ts`, `owasp.ts` (`OWASP_AGENTIC_RISKS`) | Public Preview; `tsup`/`vitest`; 0 test files present despite configured test script |
| 24 | `mastra-agentmesh/` | TypeScript, `@microsoft/agentmesh-mastra` | Middleware for Mastra agents: `governance.ts` (`governanceMiddleware`), `trust.ts` (`trustGate`), `audit.ts` (`auditMiddleware`), `governed-tool.ts` (`createGovernedTool`) | Public Preview; 0 test files present |

### Cross-cutting pattern

Regardless of target framework, adapters independently reimplement the same conceptual trio: identity (Ed25519 keypairs, DIDs, `VerificationIdentity`/`AgentProfile`/`AgentIdentity`), trust (a 0.0-1.0 score tracked via success/failure deltas, e.g. `TrustScoreTracker`, `TrustTracker`, `TrustScorer`), and audit (append-only, often hash-chained `AuditEntry`/`AuditLog`/`AuditTrail`/`GovernanceReceipt` records). Core `agentmesh` types (`agentmesh.trust.TrustProvider`, `TrustEngine`) live in the sibling `agent-mesh` package (see section 5); the README states the rule "Core NEVER imports from here."

### Python benchmarks, fuzzing, and notebooks

`benchmarks/governance_overhead.py` measures p50/p95/p99 latency and ops/sec (via `numpy`) across 21 named benchmarks in 9 categories (baseline, policy, trust, audit, delta_audit, credential, rings, hypervisor, e2e), importing directly from `agent_os.policies.evaluator` and `agentmesh.governance.trust_policy` rather than from the adapters above. A committed sample result (`benchmarks/results/governance_overhead.json`) reports figures such as bare-action p50 0.0003ms and 1-rule policy eval at roughly 190K ops/sec.

Seven `atheris`-based coverage-guided fuzz targets under `fuzz/` exercise policy condition evaluation, input validation (path traversal, SQL/code injection), the MCP security scanner, YAML policy parsing, the prompt injection detector, the sandbox AST validator, and trust scoring. `fuzz_trust_scoring.py` asserts trust scores fall in `0 <= score.score <= 1000`, an integer 0-1000 internal scale that differs from the 0.0-1.0 float scale used across most adapters above.

Five Jupyter notebooks under `notebooks/` demonstrate usage end to end: policy enforcement basics, an MCP security proxy walkthrough, multi-agent governance with circuit breakers and chaos testing, the LangChain AgentMesh chatbot (direct usage example for adapter #1, including `TrustGatedTool` and rate-limit exhaustion demos), and a Citadel/Azure AI Foundry governance deployment walkthrough.

---

## 16. Formal specifications

AGT defines its behavioral contracts through eleven formal specification documents under `docs/specs/`, plus one non-numbered reference guide, `docs/WIRE-PROTOCOL-FACETS.md`. Ten of the eleven use RFC 2119 / RFC 8174 key-word conformance language and state that all SDK implementations (Python, TypeScript, Rust, .NET, Go) MUST conform. All are marked **Status: Draft** (`AUDIT-COMPLIANCE-1.0.md` is `1.0-DRAFT`; `DYNAMIC-POLICY-CONDITIONS-1.0.md` is `Draft (v1)` and lacks RFC-2119 boilerplate); none are Final or Stable. The ACS policy-engine spec family is covered separately in section 12.

Most specs share a structure: Introduction, Terminology, domain sections, Failure Semantics, Security Considerations, Conformance Requirements (a numbered MUST checklist plus a "Test Coverage" bullet list), Worked Examples, References. None state a numeric test count in their own prose; counts below come from counting `def test_` occurrences in the corresponding `test_spec_*_conformance.py` files under `agent-governance-python/`, per `docs/conformance.md`. `docs/ROADMAP.md` claims 10 specifications with 992 conformance tests in aggregate; the ten numbered specs' dedicated test files sum to 990.

### Summary table

| # | Spec (file) | Scope | Key MUSTs | Conformance tests |
|---|---|---|---|---|
| 1 | Agent OS Policy Engine 1.0 (`AGENT-OS-POLICY-ENGINE-1.0.md`) | Policy evaluation engine (declarative + integration layers) | 12 baseline + 4 integration | 67 |
| 2 | AgentMesh Identity and Trust 1.0 (`AGENTMESH-IDENTITY-TRUST-1.0.md`) | Agent identity, credentials, trust scoring/decay | 14 | 126 |
| 3 | Agent Hypervisor Execution Control 1.0 (`AGENT-HYPERVISOR-EXECUTION-CONTROL-1.0.md`) | Ring-based execution isolation | 14 | 80 |
| 4 | AgentMesh Trust and Coordination 1.0 (`AGENTMESH-TRUST-COORDINATION-1.0.md`) | Multi-agent trust/coordination, handshake, capability scoping | 18 | 62 |
| 5 | Agent SRE Governance 1.0 (`AGENT-SRE-GOVERNANCE-1.0.md`) | SLOs, error budgets, circuit breakers, chaos, replay | 28 (144 total) | 111 |
| 6 | MCP Security Gateway 1.0 (`MCP-SECURITY-GATEWAY-1.0.md`) | MCP tool call interception, response scanning, signing | 22 (175 total) | 127 |
| 7 | Agent Lightning Fast-Path 1.0 (`AGENT-LIGHTNING-FAST-PATH-1.0.md`) | RL training governance wrapper | 20 (92 total) | 100 |
| 8 | Framework Adapter Contract 1.0 (`FRAMEWORK-ADAPTER-CONTRACT-1.0.md`) | Base integration contract, 10 framework adapters | 14 MUST + 6 SHOULD (71 total) | 152 |
| 9 | Audit and Compliance 1.0 (`AUDIT-COMPLIANCE-1.0.md`) | Audit, compliance, observability across 5 AGT components | 225 total, 3-tier levels | 157 |
| 10 | AgentMesh Wire 1.0 (`AGENTMESH-WIRE-1.0.md`) | E2E encrypted pair-wise agent messaging | 7 total | none dedicated |
| 11 | Dynamic Policy Conditions 1.0 (`DYNAMIC-POLICY-CONDITIONS-1.0.md`) | Additive temporal/budget conditions for policy engine | 2 total | 8 |
| - | WIRE-PROTOCOL-FACETS (`docs/WIRE-PROTOCOL-FACETS.md`) | SQL/K8s context-enrichment for policy rules (not a spec) | n/a | n/a |

### 1. Agent OS Policy Engine 1.0

Defines the single enforcement point for governed agent actions: a declarative layer (YAML/JSON `PolicyDocument` files evaluated by a `PolicyEvaluator`) and an integration layer (`GovernancePolicy` objects applied by framework adapters to intercept tool calls, enforce token limits, check blocked patterns). Covers nine condition operators (`eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `in`, `contains`, `matches`), folder-level policy discovery/merge, four conflict resolution strategies (`DENY_OVERRIDES`, `ALLOW_OVERRIDES`, `PRIORITY_FIRST_MATCH`, `MOST_SPECIFIC_WINS`), external backends (OPA/Rego, Cedar), and fail-closed semantics. Identity/trust scoring, ring enforcement, coordination, and SLO governance are deferred to sibling specs. Key MUSTs: a missing context field evaluates to `false`, never raises; four policy actions (`allow`, `deny`, `audit`, `block`); a "Deny Immutability Invariant" (child policies cannot override a parent deny); fail-closed on evaluation errors. Maps to Baseline requirements B-1 through B-6.

### 2. AgentMesh Identity and Trust 1.0

The longest and most granular of the four core specs (27 sections): DID generation (`did:mesh:<unique-id>`, >= 128 bits randomness), Ed25519 keypair binding, mandatory human sponsor accountability, short-lived scoped bearer credentials, a 0-1000 trust score across five reward dimensions with temporal decay and network contagion, an IATP challenge-response handshake, delegation via monotonically narrowing scope chains, and key rotation with cryptographic proofs. Key MUSTs: Ed25519 as the mandatory signature algorithm; private keys never in serialized output; trust scores clamped to [0, 1000]; wildcard `"*"` never delegated; constant-time token comparison; revoked identities never reactivated; JWK export excludes private keys by default. The toolkit's largest conformance suite (126 tests). Underlies Standard S-1 (cryptographic identity), S-2 (sub-200ms single-use handshake), Advanced A-2 (liveness attestation), and A-3 (TEE-sealed signing).

### 3. Agent Hypervisor Execution Control 1.0

Hardware-inspired execution isolation modeled on OS kernel privilege rings, deriving ring assignment from trust scores: four execution rings (Ring 0-3), action classification (reversibility, read-only status, admin flags), time-bounded trust-gated privilege elevation, per-agent token-bucket rate limiting, session isolation, a kill switch, and an append-only SHA-256 hash-chained audit trail. Unknown agents start at Ring 3 (Sandbox); Ring 0 is unreachable regardless of trust score without SRE Witness attestation. Example: `eff_score=0.97, has_consensus=true` maps to Ring 1; a Ring 2 agent requesting Ring 1 with `trust_score=0.60` is denied (requires >= 0.85). Sole basis for Advanced A-1 (execution ring enforcement).

### 4. AgentMesh Trust and Coordination 1.0

The longest of the four core specs (1844 lines), analogized to mTLS at the identity/capability level: a 0-1000 trust score with five tiers, an Ed25519 challenge/response handshake, a `TrustBridge` for peer-trust coordination with HMAC integrity checks, an RFC 9334-aligned endorsement registry, `action:resource[:qualifier]` capability scoping with deny lists, signed Agent Cards, a protocol bridge across A2A/MCP/IATP/ACP, rate limiting, and behavior monitoring with quarantine. Carries 18 top-level MUSTs, the most of the four core specs: registry-backed trust scores are authoritative; HMAC verification failures cause peer-record deletion and fail-closed denial; deny lists checked before grants; trust does not propagate transitively. Overlaps with AgentMesh Identity and Trust (both define trust scores and handshakes), but is the mesh-operational layer while Identity and Trust is the identity/credential/decay layer. Maps to Standard S-3 (delegation ceiling propagation), though delegation mechanics are specified more deeply there.

### 5. Agent SRE Governance 1.0

Defines SLOs, error budgets, circuit breakers, chaos engineering, alerting, incident detection, deterministic trace replay, Ed25519 artifact signing, and OpenTelemetry integration (144 total MUSTs, 28 in the checklist). Notable: an `SLOStatus` enum with 5 totally ordered values (`HEALTHY` < `UNKNOWN` < `WARNING` < `CRITICAL` < `EXHAUSTED`); an auto-budget formula (`error_budget.total = 1.0 - min(sli.target for sli in indicators)`); a `CircuitState` enum (`CLOSED`, `OPEN`, `HALF_OPEN`, not auto-entered in Public Preview); a 12-value `FaultType` enum spanning infrastructure, adversarial, and behavioral categories; mandatory SHA-256 trace hashing with pre-persistence PII redaction. Circuit breaker state feeds the Hypervisor kill switch; SRE Witness is required for Ring 0.

### 6. MCP Security Gateway 1.0

Defines the `MCPGateway` dual-stage pipeline: tool call interception (allow/deny/sensitive lists, approval workflows, rate limiting), response scanning (prompt injection, credential leaks, PII redaction, exfiltration URL blocking), and a security scanner covering six threat types (`TOOL_POISONING`, `RUG_PULL`, `CROSS_SERVER_ATTACK`, `CONFUSED_DEPUTY`, `HIDDEN_INSTRUCTION`, `DESCRIPTION_INJECTION`). Also covers HMAC-SHA256 message signing (minimum 256-bit keys), cryptographic session authentication, a sliding-window rate limiter (100 calls / 300s default), a CVE feed via the OSV API, trust-gated MCP server access, and schema drift detection across 8 drift types. Highest MUST density in the batch (175 total, 22 in the checklist) and the second-largest conformance suite (127 tests).

### 7. Agent Lightning Fast-Path 1.0

Defines an RL training governance layer wrapping RL kernels: a Governed Runner wraps Agent OS kernels; typed Policy Violations carry severity-based penalties; a Governed Rollout bundles task I/O with governance metadata; Reward Shaping converts violations into negative RL reward signals (additive or multiplicative, clamped to `[min_reward, max_reward]`); a Gymnasium-style Governed Environment enforces policy per step; a Flight Recorder Emitter exports audit logs as Lightning spans. Violations are learning signals rather than hard failures, though `fail_on_violation=True` MUST raise `PolicyViolationError`. 92 total MUSTs, 20 in the conformance checklist.

### 8. Framework Adapter Contract 1.0

Defines the base integration contract (`BaseIntegration` abstract class), the `GovernancePolicy` dataclass, `ExecutionContext` lifecycle, an interceptor chain (`ToolCallRequest`, `ToolCallResult`, `PolicyInterceptor`, `ContentHashInterceptor`, `CompositeInterceptor`), a native hook pattern, and ten framework adapters: LangChain, CrewAI, AutoGen, OpenAI Assistants, Anthropic, Google ADK, Semantic Kernel, OpenAI Agents SDK, PydanticAI, smolagents. Principles: framework-native integration over monkey-patching; policy pinning (contexts deep-copy the active policy at creation); graceful degradation (adapters importable without the target SDK, raising `ImportError`); deprecation over removal (legacy `wrap()`/`unwrap()` remain functional for at least two minor releases). `ContentHashInterceptor` uses SHA-256 content hashing to defeat tool-aliasing attacks. 71 total MUSTs (14 MUST + 6 SHOULD in the checklist), and the toolkit's largest conformance suite (152 tests).

### 9. Audit and Compliance 1.0

The longest spec in the toolkit (2292 lines) and an outlier in versioning (`1.0-DRAFT`) and structure. Specifies audit, compliance, and observability architecture across five components: Agent OS (core audit logging, OTel integration), Agent Mesh (Merkle-chained audit log, compliance engine, Decision Bill of Materials reconstruction, Audit Collector REST API), Agent Hypervisor (event bus, semantic delta engine, commitment engine), Agent SRE (observability events/OTel conventions), Agent Lightning (flight recorder emission, RL violation tracking). Uses a dual-tagging convention distinguishing **[Pure Specification]** from **[Default Implementation]** sections, unique in this batch, and requires exactly 4 named compliance frameworks: `EU_AI_ACT`, `SOC2`, `HIPAA`, `GDPR`. Its conformance model is a 3-tier system rather than a flat MUST-list: Level 1 (Basic Audit: backend protocol, canonical schema, structured logging); Level 2 (Governance Events: adds event sink SPI, batching/circuit breaker processor, OTel integration, cross-component correlation); Level 3 (Full Compliance: adds Merkle audit chain, compliance framework engine, Decision BOM, semantic delta engine, commitment engine, REST API, all 4 frameworks). 225 total MUSTs, the highest in the toolkit, and the single largest conformance test file (157 tests).

### 10. AgentMesh Wire 1.0

Defines the wire protocol for end-to-end encrypted pair-wise (1:1) agent-to-agent messaging: confidentiality, forward secrecy, post-compromise security, replay protection, offline store-and-forward delivery. Group messaging (1:N) is out of scope for v1.0, reserved for a future MLS (RFC 9420) version, an unimplemented roadmap item. Asserts clean-room design from published standards only (X3DH, Double Ratchet, X25519/RFC 7748, HKDF-SHA256/RFC 5869, ChaCha20-Poly1305/RFC 8439, Ed25519/RFC 8032, W3C DID Core). The least finished spec by internal evidence: only 7 total MUSTs (far fewer than any other spec), no numbered Conformance Requirements section, and a Test Vectors section that is explicitly incomplete, with placeholder values and future-tense language ("will be provided," "to be filled with actual test vector"). Its Appendix C lists 8 unimplemented documentation/tutorial deliverables, framed as "once this spec is implemented." Its one concrete test-count figure, 61 Python tests, refers to a pre-existing Signal-protocol implementation cited as design justification, not to this spec's own conformance suite; no dedicated conformance test file exists.

### 11. Dynamic Policy Conditions 1.0

A small, additive extension (172 lines) to the Python Agent OS policy engine, defining runtime-aware dynamic conditions evaluated alongside static `field/operator/value` rule conditions. Scope is narrow: two temporal condition types (`time_window`, `day_of_week`) and two budget condition types (`token_count_per_window`, `cost_per_window`). Static conditions are evaluated first; a rule matches only if both are true. `PolicyEvaluator.evaluate(context, dynamic_context=None)` adds an optional, additive parameter, preserving backward compatibility. Timezone handling requires IANA zone-data-aware conversion for DST. A V1 Implementation Note flags that budget window counters are in-memory, process-local, non-persistent, and reset on process restart ("durable quota accounting is out of scope for v1"). Five features are explicitly deferred: quota conditions, system/behavior signal conditions, composite conditions, geo-based conditions, external signal sources. Only 2 total MUSTs, no dedicated conformance section, and the toolkit's smallest test suite (8 tests).

### WIRE-PROTOCOL-FACETS (reference guide, not a numbered spec)

`docs/WIRE-PROTOCOL-FACETS.md` documents "Wire-Protocol-Aware Policy Evaluation" as a how-to guide, not a version-numbered spec: no version header, no RFC 2119 boilerplate, no Conformance Requirements section. Not to be confused with AgentMesh Wire 1.0 despite the shared "wire" terminology: one is a Signal-protocol E2E messaging spec, the other is context-enrichment for policy rule evaluation. `PolicyEngine.evaluate()` runs `extract_protocol_facets(context)` before rule evaluation, populating dot-notation fields when the context contains a `sql` or `k8s` sub-dict. SQL facets (`sql.verb`, `sql.target`, `sql.tables`, `sql.functions`) require the `sqlglot` package in Python; without it, `sql.verb` fails closed to `UNKNOWN`. Kubernetes facets (`k8s.verb`, `k8s.resource`, `k8s.namespace`, `k8s.name`, `k8s.subresource`) derive from HTTP method and path, auto-populated by the MCP proxy (`agentmesh proxy`) from tool call arguments. Custom parsers register via `default_registry.register(name, extractor_fn)`. Language parity: Python (`agentmesh.governance.protocol_facets`) and Rust (`agentmesh::protocol_facets`) are Shipped; TypeScript, .NET, and Go are Tracked but not shipped. Both shipped implementations use regex/tokenizer-based (not full-grammar) SQL parsing.

### Cross-cutting notes

`docs/conformance.md` groups the non-Wire conformance test files into three levels: Baseline (Policy Engine + Audit and Compliance, B-1 through B-6), Standard (adds Identity and Trust, MCP Gateway, AgentMesh Wire, Framework Adapter, S-1 through S-6), Advanced (adds Hypervisor Execution Control, Agent Lightning, Agent SRE, A-1 through A-5). The reference Python implementation claims Advanced level across all 9 spec files it covers; TypeScript and .NET claim Standard; Rust and Go claim Baseline only. The ACS policy engine's own spec family and conformance corpus is documented separately in section 12.

---

## 17. Standards compliance

All mapping documents live under `docs/compliance/`, indexed by `docs/compliance/index.md`. Every mapping carries an explicit disclaimer that it is an internal self-assessment, "NOT a validated certification or third-party audit": Microsoft's own gap analysis against published frameworks, not a third-party attestation.

### OWASP Agentic Security Initiative (ASI 2026)

`docs/compliance/owasp-agentic-top10-architecture.md` (v1.1, reviewed 2026-06-10), the "canonical ASI coverage page," maps all 10 official OWASP Top 10 for Agentic Applications 2026 risks plus one AGT-specific extension explicitly stated as not an eleventh official entry:

| ASI ID | Risk | Coverage | Primary AGT component |
|---|---|---|---|
| ASI01 | Agent Goal Hijack | Full | `governanceMiddleware`, `blockedPatterns` regex |
| ASI02 | Tool Misuse and Exploitation | Full | `createGovernedTool` allow/deny-lists, rate limits |
| ASI03 | Identity and Privilege Abuse | Full | PII redaction, RBAC in policy YAML |
| ASI04 | Agentic Supply Chain | Partial | tool pinning; no built-in SBOM |
| ASI05 | Unexpected Code Execution | Full | static reviewer blocks `pickle.loads()`, `eval()`/`exec()` |
| ASI06 | Memory and Context Poisoning | Partial | audit hash-chain; no memory sandbox |
| ASI07 | Insecure Inter-Agent Communication | Full | trust-gate, DID verification |
| ASI08 | Cascading Agent Failures | Full | circuit breaker, rate limiter |
| ASI09 | Human-Agent Trust Exploitation | Partial | audit trail; no human-in-the-loop approval |
| ASI10 | Rogue Agents | Full | `AgentBehaviorMonitor`, quarantines |
| AGT extension | Agent Traceability | Full | hash-chain audit middleware (SHA-256) |

Stated result: 7/10 Full, 3/10 Partial, 0 Gaps, with evidence in `agent_os/governance/middleware.py`, `tool_wrapper.py`, `agentmesh/services/behavior_monitor.py`, and `agent_os/trust/gate.py`. The document also includes a "Physical Agent Systems Profile" extending ASI01-ASI10 to robots/drones/AVs/IoT actuators, explicitly a threat-model extension rather than an OWASP publication or robotics safety certification, since AGT is a policy enforcement point, not a safety-rated controller.

A separate `owasp-llm-top10-mapping.md` (2023 numbering) scores 0 fully mitigated, 9 partial, 1 gap (LLM10 Model Theft: "the toolkit wraps LLM APIs, does not host models"), with a "Detection Without Enforcement" finding that six of ten risks have detection utilities never wired into enforcement. `mcp-owasp-top10-mapping.md` (2025 Beta) scores 7 of 10 fully covered, 3 partial (MCP01 secrets, MCP06 intent-flow subversion, MCP09 shadow MCP servers).

### NIST AI Risk Management Framework (AI RMF 1.0)

`docs/compliance/nist-ai-rmf-alignment.md` (v1.0, 2026-07-14) assesses all 19 subcategories across GOVERN, MAP, MEASURE, MANAGE: 12 Fully Addressed (63%), 7 Partially Addressed (37%), 0 Gaps. Strongest areas are GOVERN 1 (Policy), MANAGE 1 (Risk Response), and MANAGE 4 (Monitoring); weakest is MAP 5, the bias/fairness subcategory, a flagged Priority-1 gap ("regex-only... no bias detection algorithms or fairness metrics... no DSAR workflow"). A cross-reference matrix maps all 19 subcategories to ATF element IDs, OWASP risk IDs, EU AI Act articles, and SOC 2 criteria. A companion document, `nist-rfi-2026-00206.md`, is Microsoft's response to NIST's RFI "Security Considerations for AI Agents" (Docket 2026-00206).

### EU AI Act (Regulation 2024/1689)

`docs/compliance/eu-ai-act-checklist.md` (prepared 2026-04-03, reviewed 2026-06-10) covers 11 articles: Art. 4, 6, 9, 10, 11, 12, 13, 14, 15, 26, 50. Result: 2 of 11 fully out of scope (Art. 4, Art. 10), 0 fully covered, 9 Partial, with Conformity Risk rated High for Art. 6, 9, and 26, which the document calls "functionally non-compliant in their current state." Key findings: Art. 6 classification logic lives only in `examples/compliance_checker.py`, not importable or tested in CI; Art. 9's `assess_risk_category()` does keyword substring matching against 15 hardcoded terms and misclassifies an adversarial social-scoring example as MINIMAL_RISK; Art. 12 logging is the strongest area but `FlightRecorder` hashes INSERT-time state, not the final verdict; Art. 26 retention defaults (`retention_days` default 90, minimum 1) violate the roughly 180-day floor implied by Art. 26(6), a flagged must-fix. Articles not covered at all include Art. 17, 27, 43, 49, 62, and 72.

### SOC 2 Type II (AICPA Trust Services Criteria)

`docs/compliance/soc2-mapping.md` (updated April 2026, Toolkit v2.3.0) maps Security (CC1-CC9), Availability (A1), Processing Integrity (PI1), Confidentiality (C1), Privacy (P1-P8): 0 of 5 fully covered, 4 Partial, 1 Gap (Privacy, the largest gap area: no consent management, no DSAR workflow, unenforced `retention_days`, only 2 built-in PII regex patterns). A "Resolved" subsection records fixes since a prior snapshot, including a real `DeltaEngine.verify_chain()` implementation and a working `KillSwitch` saga handoff, indicating this document postdates the EU AI Act checklist, which still lists both as unresolved.

### Cloud Security Alliance Agentic Trust Framework (ATF)

`docs/compliance/atf-conformance-assessment.md` (ATF v0.9.0, assessed April 2026, Toolkit v3.1.0, target maturity "Senior") reports 25/25 requirements addressed across the framework's five elements, 18 fully met and 7 partially met, 0 not met:

- **1. Identity** ("Who are you?"): 5/5, partial on I-4 Purpose Declaration.
- **2. Behavior** ("What are you doing?"): 5/5, partial on B-2 Action Attribution and B-3 Behavioral Baseline.
- **3. Data Governance** ("What are you eating? What are you serving?"): 5/5, partial on D-3 PII/PHI Protection and D-5 Data Lineage.
- **4. Segmentation** ("Where can you go?"): 5/5, all fully met.
- **5. Incident Response** ("What if you go rogue?"): 5/5, partial on R-5 Graceful Degradation.

The document states AGT "meets Senior requirements and partially addresses Principal-level requirements" (citing D-5, R-4, S-4/S-5 as Principal-tier items met).

### AARM and README badge claims

The README's Standards Compliance table (`README.md:359-369`, repeated at `README.md:398`) carries two external claims not substantiated by any file inside `docs/compliance/`: an "AARM-Extended (R1-R9)" badge linking to `https://aarm.dev/builders/agent-governance-toolkit-microsoft`, stating "All R1-R9 requirements satisfied; verified Jun 14, 2026" with no corresponding internal document, and an ATF entry stating "All five elements mapped: Agent Mesh (identity), Agent OS (policy), Agent Compliance (governance), Agent Runtime (sandboxing), Agent SRE (incident response)," linking externally rather than to `atf-conformance-assessment.md`. The README's ATF claim is a coarser restatement that omits that document's 18-full/7-partial granularity.

### Additional mappings and cross-cutting patterns

Other mappings in the directory: `iso-42001-mapping.md` (ISO/IEC 42001:2023, Clauses 4-10, Clause 8 Operation as the strongest area), `cis-controls-v81-mapping.md` (CIS Controls v8.1: 28 of 42 safeguards fully addressed, 10 partial, 4 gaps), and `nsa-mcp-alignment.md` (NSA/CISA MCP guidance: 8 themes covered, 3 partial, mirroring the OWASP MCP mapping). Operational templates (skimmed for structure only) include `fria-template.md`, `impact-assessment-template.md`, `incident-response-workflow.md`, `post-market-monitoring.md`, `record-retention-policy.md`, and `data-provenance-model.md`.

Across frameworks, the mapping documents consistently distinguish "detection exists" from "enforcement is wired," repeatedly naming the same root-cause defects: the `DeltaEngine.verify_chain()` stub, `FlightRecorder` hashing INSERT-time state instead of final verdict, `KillSwitch` handoff logic, only 2 built-in PII regex patterns (SSN, credit card), and the `retention_days` default/minimum violating the EU AI Act's roughly 180-day retention floor. Differing review dates (SOC 2 and ATF April 2026, EU AI Act checklist April/June 2026, NIST AI RMF July 2026) explain some resolved-versus-open discrepancies for the same defect. Every document reiterates the same boundary: AGT is a runtime and application-middleware governance layer, not a training-data pipeline tool, model host, or substitute for safety-rated hardware controllers.

---

## 18. Security posture and red-teaming

Security artifacts span `SECURITY.md`, `docs/security/`, `tests/redteam/`, `benchmarks/prompt-injection/`, `.clusterfuzzlite/`, `.gitleaks.toml`, `.safety-policy.yml`, and `docs/dependency-audits/`.

### Policy and threat model

`SECURITY.md` follows the standard Microsoft OSS template and adds a trust-boundary summary (`AI Agent (untrusted) -> AGT Policy Engine (trust anchor) -> Protected Resources`, with a tamper-proof audit-log branch), a 7-row threat table (policy bypass, identity spoofing, audit tampering, budget evasion, tool-call injection, supply chain compromise, privilege escalation via delegation), and operator guidance to run the policy engine "as a separate process or sidecar, not embedded in the agent's own process," preventing a compromised agent from altering policy evaluation in-process. In-scope components: `agent_os`, `agentmesh`, `agent_sandbox`, CI/CD supply chain, `agent_compliance`. Out of scope: DoS against the local `agt` CLI, Dependabot-tracked issues, social engineering. Contact `secure@microsoft.com` (24-hour acknowledgement, 90-day coordinated disclosure); supported versions 3.4.x/3.3.x/3.2.x.

Two retrospective advisories, both fixed in v2.1.0: **CostGuard Organization Kill Switch Bypass** (High, `<2.1.0`, PR #272), crafted NaN/Infinity/negative budget inputs bypassing the org-level kill switch, fixed by rejecting IEEE-754 special values and making `_org_killed` permanent; and **Thread Safety Fixes** (Medium), four races (CostGuard breach history #253, VectorClock #243, `ErrorBudget._events` deque #172, a .NET sweep #252).

The canonical `docs/security/threat-model.md` (STRIDE-oriented, cross-referencing the OWASP ASI mapping in section 17) scopes Agent OS, AgentMesh, Agent Runtime, and Agent SRE, and enumerates four trust boundaries: Human to Agent, Agent to Agent, Agent to Tool (the "highest-risk execution boundary"), and Agent to Platform Control Plane. Its Residual Risks section is candid rather than complete, linking to `docs/LIMITATIONS.md`: misconfigured-but-valid policies, unsafe approvals under time pressure, a knowledge-flow risk (AGT governs tool calls, not the documents/embeddings agents consume, §7), credential persistence across sessions (§8), a physical-AI scope gap (§10), per-action rather than continuous policy evaluation (§11), and a DID method inconsistency: Python/.NET use `did:mesh:*` while TS/Rust/Go use `did:agentmesh:*` (§12). A "Configuration Bypass Vectors" table, attributed to Periculo's external red-team analysis, lists default-allow when no policies are loaded, permissive mode left on in production, tool-name aliasing, and "import-only governance" (a false "governed" status; mitigation: `agt doctor` / `agt audit`).

### Boundary honesty

AGT documents itself as application-layer middleware, not an OS or hardware isolation boundary. `docs/LIMITATIONS.md` §6 ("What AGT Is Not") contrasts application-layer middleware against OS kernel/hardware isolation and deterministic policy enforcement against probabilistic guardrails, concluding "AGT is one layer in a defense-in-depth strategy, not the entire strategy." §9 states that if governance middleware is imported but not configured, "agents may run without effective policy enforcement," including a dashboard showing "governed" status while no rules are enforced. §1 notes AGT cannot detect indirect prompt injection corrupting agent reasoning or correlate sequences of individually-allowed actions into a malicious workflow, citing external research (Dai et al., arXiv:2605.06158) reporting 80-95% attack success rates when persistent memory carries attack state across individually-permitted sessions. `docs/security/tenant-isolation.md` and `docs/security/tenant-isolation-checklist.md` are byte-for-byte identical (192 lines each), an apparent unintentional duplication.

### Red-team test suite and prompt-injection benchmark

`tests/redteam/test_asi.py` (414 lines) is a "Red Team Simulation Suite for OWASP ASI Starter Packs," payloads "synthesized from Arcanum-Sec research," measuring kill rate, false-positive rate, and latency. A `SCENARIOS` list of 29 entries (28 adversarial, 1 benign baseline) is evaluated against three starter policy packs (`healthcare`, `financial-services`, `general-saas`) via `agent_os.policies.evaluator.PolicyEvaluator`, parametrized into 29 x 3 = 87 pytest cases. Categories by `asi_risk` tag include ASI-01 (CBRN framing, nested delegation), ASI-03 (identity poisoning, MFA bypass, CEO password reset; 6 scenarios), ASI-04 (MCP registry poisoning, plugin hijack, dependency poisoning; 6 scenarios), ASI-09 (payment redirection, VIP impersonation, phishing; 4 scenarios), ASI-10 (charter roleplay, purpose override, autonomous-loop bypass; 3 scenarios), and others (ASI-02, -05, -06, -07), plus the benign baseline as the sole false-positive check. This is a policy-YAML-level black-box conformance suite, not a live LLM red-team, targeting 100% block rate for adversarial scenarios.

`benchmarks/prompt-injection/` is explicitly an evaluation-only fixture ("no runtime behavior changes, no embedding detector, no default blocking policy, no production performance claim"). A 280-row deterministic smoke corpus (110 attack / 170 benign rows) is scored by a Rust harness using AGT's production `agentmesh` `PromptInjectionDetector` (not a new model), with a build-time check that fails if raw prompt text leaks into committed evidence. Reported baseline results, caveated as "smoke-fixture numbers only": 7/110 attack rows caught (6.36% recall) and 16/170 benign rows flagged (9.41% false-positive rate). The low recall is documented candidly: rules-based detection is expected to be high-precision on known patterns rather than a complete detector, and the fixture's main value is surfacing false-positive pressure from benign text that discusses or quotes injection phrases.

### ClusterFuzzLite fuzzing

`.clusterfuzzlite/build.sh` (fails loudly by design) installs the toolkit packages, pins `atheris==2.3.0`, and compiles every `fuzz_*.py` under `agent-governance-python/fuzz/`:

| Target | Surface |
|---|---|
| `fuzz_condition_eval.py` | Policy condition-expression evaluation, mirrors `SharedPolicyEvaluator` |
| `fuzz_input_validation.py` | `validate_path` (traversal, `PROTECTED_DIRS` denylist), `validate_code` (denies `exec/eval/subprocess`), `validate_sql` (denies destructive DDL/DML) |
| `fuzz_mcp_security.py` | `agent_os.mcp_security.MCPSecurityScanner.scan_tool` |
| `fuzz_policy_yaml.py` | `yaml.safe_load` plus bounded walk of parsed policy structure |
| `fuzz_prompt_injection.py` | `agent_os.prompt_injection.PromptInjectionDetector.scan` |
| `fuzz_sandbox.py` | `agent_os.sandbox.SandboxValidator.validate_code` (AST-based) |
| `fuzz_trust_scoring.py` | `agent_os.trust_root.TrustManager`, asserts `0 <= score.score <= 1000` |

All seven decode bytes with `errors="replace"` and fall back to `except Exception: pass`, tuned to surface crashes and hangs rather than any exception. Only `fuzz_trust_scoring.py` encodes a functional correctness assertion; the rest are pure crash/hang fuzzers.

### Secret scanning and dependency policy

`.gitleaks.toml` extends the upstream Gitleaks default ruleset and prefers path-based allowlisting over `.gitleaksignore` fingerprints, which break on squash merges. Allowlisted paths cover the vendored Agent Control Specification (ACS) example/test fixtures (placeholders `ghp_secret123`, `finalSecret42`), credential-redaction unit tests across Python/.NET, the redactor's own Rust source, and AgentMesh/MCP integration test suites with fake tokens.

`.safety-policy.yml` (Safety CLI legacy `safety check`) sets no severity floor, does not ignore unscored CVEs, and does not hard-fail on its own, deferring severity gating to CI workflow logic.

`docs/security/scanning.md` documents a narrower pipeline scoped to plugin contributor PRs: `detect-secrets`, `pip-audit`, `npm audit`, `bandit`. Critical/High findings block merge; Medium/Low warn only. A per-plugin `.security-exemptions.json` override requires a `reason` (minimum 10 characters) and, for Critical/High, `approved_by`/`ticket`.

### Dated audit trail

`docs/security/audits/` is governed by a CI gate (adapted from AzureClaw's Phase 0 CI gates) requiring a dated audit document whenever a PR touches policy engine, identity, trust, encryption, execution rings, or kill-switch code; 16 such documents exist (2026-04-29 through 2026-06-25), covering topics such as identity-provider-chain, Entra JWT verification, and evaluator-backend fail-closed behavior. `docs/dependency-audits/` is governed by an analogous gate triggered by lockfile changes, requiring dependency-change rationale, CVE relevance, and breaking-change risk assessment; 39 dated documents exist (2026-05-15 through 2026-06-18) across the polyglot stack and vendored subprojects (Foundry AI Gateway PDP, the ACS sync, MCP proxy, Flowise example).

---

## 19. Examples and demos catalog

The toolkit ships 39 top-level example directories under `examples/`, a live-demo collection under `examples/demos/`, a benchmark suite under `benchmarks/prompt-injection/`, and performance-benchmark documents at `docs/BENCHMARKS.md` and `docs/benchmarks/`. Tutorials, the workshop kit, case studies, and AGT Studio are in section 20.

### Framework-governed agent examples

Each wraps a real agent framework with AGT governance, typically shipping a `README.md`, a short `getting_started.py`, a fuller demo script, and a `policies/` directory.

| Example | Framework | What it demonstrates |
|---|---|---|
| `examples/openai-agents-governed/` | OpenAI Agents SDK | 4-agent pipeline (Researcher-Writer-Editor-Publisher); 9 scenarios: tool access (`CapabilityGuardMiddleware`), PII policies (`GovernancePolicyMiddleware`), quality gates (`TrustScorer`), rogue detection, injection defense (8 attacks, all blocked), trust-gated handoff (`HandoffVerifier`), tamper detection |
| `examples/crewai-governed/` | CrewAI | Same crew/scenario structure, adapted to CrewAI delegation governance (`TrustedCrew`); injection defense reports 7/8 blocked |
| `examples/smolagents-governed/` | Hugging Face smolagents | 4-agent research crew; scenario 3 is "Model Safety Gates" (restricts model downloads, blocks code execution) |
| `examples/adk-governed/` | Google ADK (`GoogleADKKernel`) | 9-part walkthrough: blocked-tool enforcement, allowlists, dangerous-content detection, human approval, budget config; policy YAML is illustrative only, scripts configure governance directly through the kernel |
| `examples/deerflow-governed/` | DeerFlow (`AGTGuardrailProvider`) | Normalizes DeerFlow's `GuardrailRequest` into AGT policy context via `PolicyEvaluator`/`AuditLog`; demo shows 10 allowed / 12 denied actions, audit stores `tool_input_sha256` not raw input |
| `examples/openshell-governed/` | OpenShell sandbox | 3 allowed / 3 denied actions; trust decay 1.00 to 0.55 as violations accumulate |
| `examples/cedarling-governed/` | Cedarling (external backend) | Role-based access from request identity, and capability-based access from verified multi-issuer JWTs plus device posture; depends on `cedarling_agentmesh`, not yet on PyPI |
| `examples/maf-integration/` | Microsoft Agent Framework | Six paired Python/.NET scenarios (table below) |
| `examples/mcp-trust-verified-server/` | MCP server | Demo-only trust-gated server: escalating trust thresholds (300/600/800), rate limiting, `MCPSecurityScanner` fingerprinting |
| `examples/citadel-governed-agent/` | Azure APIM (Citadel) | AGT agent-level governance plus Citadel gateway-level governance (rate limiting, JWT validation, cost attribution); audit export via `CitadelAuditExporter` |
| `examples/flowise-governance/` | Flowise (Node.js) | FastAPI sidecar called over HTTP before each tool call; rate limiting, policy checks, hash-chained audit JSONL |

`examples/maf-integration/` scenarios: 01 Loan Processing (Banking: PII blocking, rogue transfer detection), 02 Customer Service (Retail: refund fraud prevention), 03 Healthcare (HIPAA PHI blocking), 04 IT Helpdesk (privilege escalation prevention), 05 DevOps Deploy (deployment gates, storm detection), 06 .NET Extension Validation (validates the shared `Microsoft.Agents` extension, .NET-only). Python uses the published `agent-framework` package and `agent_os.integrations.maf_adapter`; .NET uses `Microsoft.Agents.AI` with `BuildAIAgent(...)`. Shares scenario stories with tutorial `34-maf-integration.md` (section 20) and `examples/demos/maf-integration`.

### Trust, identity, attestation, and receipt examples

| Example | What it demonstrates |
|---|---|
| `examples/intent-auth/` | Intent-based authorization: declare/approve/execute/verify lifecycle, drift detection (trust drops 1.0 to 0.7 on unplanned `delete_file`), child-intent scope inheritance |
| `examples/decision-bom/` | Reconstructs a governance-decision Bill of Materials: partial BOM (audit-only, 60%/3 fields) vs. full BOM (100%/7 categories) |
| `examples/cost-governance/` | Tiered budgets (per-task $2, per-day $20, org $100); alert escalation (WARNING 50%, THROTTLE 85%, KILL 95%); anomaly detection |
| `examples/spendguard-composite/` | Community-contributed, alpha SDK. Composes AGT's in-process `PolicyEngine` with the external Agentic SpendGuard project (Postgres-ledger budget reservation via gRPC sidecar); AGT-denied actions never reach the ledger; `--mock`/`--real` modes |
| `examples/crypto-attestation-governed/` | Ed25519-signed receipts, embedded `PolicyAttestation`, hash-chained audit trail, offline verification; explicitly "not a production API contract" |
| `examples/physical-attestation-governed/` | Governance receipts for IoT/cold-chain sensor data; Cedar-policy thresholds (temperature, humidity, shock, GPS); pure stdlib, no dependencies |
| `examples/reasoning-attestation-governed/` | Captures a sparse-autoencoder feature-activation slice, JCS-canonical envelope (RFC 8785), Ed25519-signed, binds reasoning state to `action_ref` and `policy_sha256`; also "not a production API contract" |
| `examples/mcp-receipt-governed/` | MCP tool-call receipt signing with Cedar policy evaluation; demo: 7 receipts (4 allowed, 3 denied) |

### Multi-agent, pipeline, and data-quality governance

| Example | What it demonstrates |
|---|---|
| `examples/multi-agent-governance/` | Collective constraints: 3 transfers/min rate limit (4th blocked), 2-agent concurrent DB-write cap, alert-only policies |
| `examples/pipeline-governance/` | Governs multi-node distributed LLM inference pipelines: Cedar policy, signed receipts, hash-chained cross-shard trust propagation |
| `examples/data-quality-aware-governance/` | Two-layer governance: policy authorization plus a Data Quality Registry check (freshness, quality score, drift), blocked if either fails |
| `examples/marketplace-governance/` | Sample requiring customization. PR-triggered CI validates plugin manifests, then verifies governance attestation before promotion |
| `examples/github-actions-governance/` | Wires the AGT governance gate into GitHub Actions via `scripts/governance_gate.py`; documents the reusable workflow `agent-governance-gate.yml@main` |
| `examples/agent-mesh/` | Container pointing to `multi-agent-chat/` (governed chat with trust scoring) |
| `examples/agent-sre/` | Container pointing to `basic-runbook/` (SLO monitoring, alerting, cost tracking) |

### Threat-rule and policy-authoring examples

| Example | What it demonstrates |
|---|---|
| `examples/atr-import/` | Community example. Compiles Agent Threat Rules (ATR v2.2.1, 419 rules/10 categories) into per-category `PolicyDocument` YAML |
| `examples/atr-community-rules/` | Ships a 15-rule starter policy and a full 108-rule policy (99.6% precision / 96.9% recall per the README); tests include CVE regression coverage for Semantic Kernel CVE-2026-25592 and CVE-2026-26030 |
| `examples/acs-atr-annotator/` | Enforces ATR through the ACS runtime (section 12): `ATRAnnotator.dispatch(...)` via the in-process `pyatr` engine; `ATRPolicy.evaluate(invocation)` denies at or above `min_severity`; dispatcher exceptions fail closed |
| `examples/aegis-governance-profile/` | Community-contributed, experimental, not AGT-endorsed. Compiles a declarative profile YAML into equivalent Cedar and Rego policies; 60 tests (56 without optional engines) |
| `examples/policy-templates/` | 7 YAML templates: `conflict-resolution.yaml`, `edu-k12.yaml` (FERPA/COPPA/CIPA), `financial-services.yaml` (SOX/PCI DSS/AML), `general-saas.yaml` (OWASP ASI-01 through ASI-10), `healthcare.yaml` (HIPAA), `federation-policy.yaml` (ADR-0007), `wire-protocol-rules.yaml`; industry files carry a "STARTER policy" warning |
| `examples/policies/` | Sample-policy directory: `african-regulatory/`, `india-regulatory/`, `production/`, plus standalone policies (`pii-detection.yaml`, `sql-safety.yaml`, `mcp-security.yaml`, others) |
| `examples/quickstart/` | Scripts across major frameworks: `govern_in_60_seconds.py`, `retrofit_governed.py`, `mcp_receipts_in_60_seconds.py` (no API key), plus LangChain/CrewAI/AutoGen/OpenAI Agents/Google ADK variants; also `aca_research_agent.py`/`aca_sandbox_test.py` (Azure Container Apps sandbox, section 6) and `nono_sandbox_test.py` (Landlock/Seatbelt sandbox) |

### Coding-agent and CLI governance examples

Covered in more depth in section 14.

| Example | What it demonstrates |
|---|---|
| `examples/claude-code-agt/` | Walkthrough of `agent-governance-claude-code`: plugin loading via `--plugin-dir`, prompt blocking via `UserPromptSubmit`, tool review/deny via `PreToolUse`, MCP-backed slash commands |
| `examples/copilot-cli-agt/` | Experimental Copilot CLI extension using the TypeScript AGT SDK (`PolicyEngine`, `ContextPoisoningDetector`, `McpSecurityScanner`, `AuditLogger`); points to the published `agent-governance-copilot-cli` installer |
| `examples/opencode-agt/` | Walkthrough of `agent-governance-opencode`: plugin loading via `opencode.json`, prompt blocking via an `event` hook, tool review/deny via `tool.execute.before`, secret redaction on `tool.execute.after` |

### Gateway/PDP example

`examples/foundry-ai-gateway-pdp/` is an experimental reference sample tracking RFC #2470 and ADR-0026 (sections 2, 16): keeps Microsoft Foundry prompt-based agent traffic inside one governance boundary using Azure API Management as the Policy Enforcement Point and an Azure Function as the Policy Decision Point, via a versioned decision contract (v1.0) including `schemaVersion`, `agentId`, `inputDigest` (SHA-256 of prompt/args), and `correlationId`.

Examples flagged as community-contributed, non-core, or experimental rather than first-party: `atr-import`, `spendguard-composite`, `aegis-governance-profile`, `foundry-ai-gateway-pdp`, `marketplace-governance` (sample only), `copilot-cli-agt` (experimental), and `crypto-attestation-governed`/`reasoning-attestation-governed` (both "not a production API contract").

### `examples/demos/`

Titled "Agent Governance Toolkit: Live Governance Demo," this container emphasizes real LLM calls (OpenAI/Azure OpenAI, not mocked): Policy Enforcement, Capability Sandboxing, Rogue Detection (50-call burst triggers auto-quarantine), Content Filtering, and Audit Trail, run via `python demo/maf_governance_demo.py [--model gpt-4o] [--verbose]`. Subdirectories:

- `governance-dashboard/`: reference Streamlit dashboard (`app.py`, `docker-compose.yml`) with simulated demo data by default. Pages: Fleet Overview, Shadow Agents, Lifecycle Monitor, Policy Feed, Trust Heatmap. Distinguished from the separate "Trust Score Dashboard" at `agent-governance-python/agent-mesh/examples/06-trust-score-dashboard/` (pluggable to live data) and the `DashboardAPI` backend at `agent-governance-python/agent-mesh/src/agentmesh/dashboard/` (live EventBus). Run via `streamlit run app.py`.
- `governed-agent-in-10-min/`: six live demos covering install/health check, sub-millisecond enforcement (10,000 live policy evaluations), a multi-agent loan workflow, zero-trust agent identity (DID handshake, kill switch), MCP tool poisoning detection, and a tamper-proof audit trail; also ships a `bicep/` reference Azure deployment.
- `maf-integration/` (distinct from top-level `examples/maf-integration/`): three single-file MAF wiring demos.
- `openclaw-governed/`: runs the AGT governance sidecar against OpenClaw-style tool calls, verified against API endpoints tested at v3.1.0.
- `presentation/`: keynote and stakeholder-review demos: `agt-live-demo.ipynb` (~8 min), `owasp-contoso-bank.ipynb` (~15 min OWASP Agentic Top 10 walkthrough), `console.html` (self-paced visual map of AGT), and a headless PowerShell verification harness.

### Benchmark suites

`benchmarks/prompt-injection/` is the only benchmark suite at the repo-root `benchmarks/` path, the "Prompt-Injection Evaluation Fixture," explicitly evaluation-only: no runtime behavior changes, no embedding detector, no default blocking policy, no production performance claim. It contains a 280-row labelled smoke corpus (110 attack-labelled, 170 benign-labelled), a deterministic corpus generator/checker, a Rust scorer importing AGT's `agentmesh` crate, and a `run-smoke.sh` reproduction script. The smoke baseline for AGT's Rust `PromptInjectionDetector` (default config) is attack recall 0.0636 (7/110 caught) and benign false-positive rate 0.0941 (16/170 flagged); the README stresses these are fixture-only, not production or general-security benchmark results.

`docs/benchmarks/governance-overhead.md` (Issue #720) measures latency overhead across `agent-os`, `agent-mesh`, and `agent-hypervisor`. Key finding: full-stack governance (policy plus trust plus ring check plus Merkle audit) adds approximately 0.07 ms p50 / 0.42 ms p99 per action, under 0.04% of typical 200-2000 ms LLM call latency.

`docs/BENCHMARKS.md` (top-level, distinct from `docs/benchmarks/`; toolkit v2.1.0, Python 3.13, 10,000 iterations) reports policy evaluation at 0.011 ms p50 for a single rule and 0.030 ms p50 for a 100-rule policy, kernel enforcement allow-path at 0.103 ms p50, and near-linear concurrent throughput scaling (46,329 ops/sec at 50 agents, 47,085 ops/sec at 1,000 agents). Its Security and Red-Team Benchmarks section states AGT does not publish an in-house Attack Success Rate benchmark, instead citing JailbreakBench, Andriushchenko et al. 2024, and the Microsoft AI Red Teaming Agent (section 18): AGT's value is moving allow/deny decisions off the probabilistic model into deterministic application code, not lowering ASR.

Tutorials (`docs/tutorials/`, 69 markdown files), the workshop kit, case studies, and the standalone `docs/demos/conversation-guardian-demo.py` are covered in section 20.

---

## 20. Documentation, tutorials, workshop, case studies, AGT Studio

### Documentation site structure

The documentation site is built with MkDocs Material (`mkdocs.yml` at repo root, `docs_dir: docs`, published to `https://microsoft.github.io/agent-governance-toolkit`, edit URI `edit/main/docs/`). Notable theme features: `navigation.instant`, `navigation.tabs` (sticky), `search.suggest`, `content.code.copy`, permalinked `toc`, and `pymdownx.superfences` with a custom `mermaid` fence. A language switcher links to translated landing pages (Japanese, Korean, Simplified/Traditional Chinese) under `docs/i18n/`.

Published nav top-level sections: Home, Getting Started, Packages (13 pages), Tutorials, Deployment, Security, Compliance (16 framework mapping pages), Conformance, Studio, Specifications (10 formal specs, see section 16), Architecture Decisions (ADR-0001 through 0025 only), and Reference (Benchmarks, Comparison, NIST RFI Mapping, Changelog, Contributing).

Several substantive directories exist on disk but are entirely absent from the mkdocs nav, reachable only via GitHub browsing or direct edit links: `docs/proposals/` (30 files, see section 22), `docs/operations/`, `docs/slo/`, `docs/policies/`, `docs/integrations/`, `docs/releases/`, `docs/package-consolidation/`, `docs/dependency-audits/` (~45 files), `docs/workshop/`, `docs/case-studies/`, and top-level governance files (`RFC_PROCESS.md`, `PUBLISHING.md`, `RELEASE.md`, `TESTING_GUIDE.md`, `ERROR_HANDLING.md`, `COMMUNITY.md`, `ROADMAP.md`, and others). ADR-0026 through 0032 also exist on disk and are tracked in `adr/index.md`'s tables but are not yet added as individual nav entries, even though they include the most roadmap-relevant decisions (the Studio UI decision, ADR-0028, and the MCP dual-stack migration, ADR-0027).

### Tutorials

`docs/tutorials/index.md` fronts roughly 35 tutorial pages surfaced in the mkdocs nav, but the `tutorials/` directory on disk actually contains 55 numbered tutorials plus a `policy-as-code/` sub-series of 7 chapters and standalone guides (`progressive-governance.md`, `retrofit-governance.md`). Only a curated subset is wired into the nav; tutorials 23 through 55 (delegation chains, cost and token budgets, security hardening, SBOM and signing, the MCP scan CLI, and more) exist on disk, cross-linked from other docs, but not all reachable from the sidebar. `docs/ROADMAP.md` cites "46+ tutorials + 7 policy-as-code chapters" as part of the v3.7.0 shipped baseline.

### Workshop kit

`docs/workshop/` is a self-contained 2-hour training kit, distinct from `docs/tutorials/` and `examples/`. `README.md` gives a fixed 2-hour agenda alternating slides and labs. `slides.md` is a 22-slide deck covering "Why Governance Matters," the OWASP AI Security Top 10, the three layers of governance, DIDs, trust scores (0-1000 scale), trust handshakes, human sponsors, and capability scoping. `lab-guide.md` is the participant guide for 3 labs; `facilitator-notes.md` adds timing cues and an FAQ; `prerequisites.md` is a setup checklist (Python 3.10+, VS Code or PyCharm).

Three lab scripts build progressively: `lab1_first_policy.py` uses `agent_os.policies.PolicyEvaluator` to write a YAML policy blocking `execute_code`; `lab2_multi_agent_trust.py` uses `agentmesh.AgentIdentity`/`RiskScorer` and `agentmesh.trust.TrustHandshake` to build two agents, run a handshake, and revoke credentials; `lab3_full_governance_stack.py` combines these plus `agentmesh.governance.audit.AuditLog` into one end-to-end pipeline. The README cross-links to tutorials 01, 02, and 04 plus `quickstart.md`, positioning the workshop as a higher-level teaching artifact layered on the tutorial catalog.

### Case studies

`docs/case-studies/` holds four files: `TEMPLATE.md` ("Case Study Template - Agent Governance in Enterprise Environment") with a metadata block (Title, Organization, Industry, Primary Use Case, AGT Components Deployed, Timeline, Deployment Scale) followed by numbered sections starting with "1. Executive Summary," and three "sample-*" case studies generated from it: `sample-ecommerce-customer-service.md` ("GDPR-Compliant Customer Service Agents at VelvetCart Commerce," pinned to AGT v3.1.0), `sample-financial-trading-compliance.md` ("SEC-Compliant Algorithmic Trading Agents at Merchantlife Trading Group," with a detailed Agent Runtime Sandboxing section on execution isolation, privilege rings, and side-channel mitigations), and `sample-healthcare-prior-authorization.md` ("HIPAA-Compliant Prior Authorization Agents at Cascade Health Partners," sharing the financial study's skeleton). All three are disclaimed as hypothetical, illustrative material ("No real-world company data or metrics are included"), not real customer deployments.

### Release notes archive

`docs/releases/` contains 10 versioned release-notes files spanning v1.0.0 through v3.7.0, separate from the root `CHANGELOG.md`. There is a gap in the numbered sequence (no v2.0.0, no v3.3.0-v3.5.0), and `v3.7.0.md` states it "opens the next development cycle with full release documentation for the v3.6.0 milestone that was previously undocumented," indicating the archive is backfilled retroactively rather than chronological at release time. The notes trace a maturity arc: early releases (v1.0.0 through v2.3.0) carry a "Community Preview... NOT official Microsoft-signed releases" banner; starting at v3.0.0 the banner flips to "Public Preview... Microsoft-signed... production-quality but may have breaking changes before GA." v2.2.0 documents migrating PyPI publishing off GitHub Trusted Publishers onto ESRP; v3.1.0 highlights a unified `agt` CLI and governance dashboard; v3.1.1/v3.2.0 add Signal-protocol end-to-end encrypted agent messaging; v3.7.0 documents a `ToolPolicy` schema upstreamed to `oracle/agent-spec` PR #191, noted as "AGT will adopt... once merged upstream" (forward-looking, not yet implemented). `docs/reference/changelog.md`, wired into the nav as "Changelog," is a literal stub reading "Documentation coming soon"; real changelog content lives in `CHANGELOG.md` and the release-notes files instead.

### Dependency audit log

`docs/dependency-audits/` is a supply-chain audit trail required by the `scripts/ci/vendored-patch-audit.sh` CI gate: any PR changing a lockfile or vendored content must add a dated file (`YYYY-MM-DD-<description>.md`) with three required sections ("Which dependencies changed and why," "Security advisory relevance," "Breaking change risk assessment"). 45 dated audit files are present (2026-05-15 through 2026-06-18), covering Rust, Node/TypeScript, and Python dependency bumps, each following the same template (date, PR number, dependencies-changed table, advisory relevance, risk rating, rollback plan). This is a build-supply-chain governance artifact enforced by CI, distinct from security/threat-model documentation (see section 21).

### Operations and deployment guidance

`docs/operations/` holds two issue-tracked runbooks. `advisory-to-blocking-graduation.md` is a checklist for moving a repository from advisory (warn-only) to blocking (CI-failing) governance, covering prerequisites, policy configuration (`mode: strict`), CI/CD integration, monitoring/rollback thresholds, and a 48-hour post-graduation window. `pre-commit-hook-template.md` ships a drop-in `.pre-commit-config.yaml` with hooks `agt-validate`, `agt-doctor`, `detect-secrets`, `no-stubs` (blocks staged `TODO`/`raise NotImplementedError`), and `no-custom-crypto` (blocks raw crypto imports outside security/test paths), with a 4-week phased rollout mirroring CI.

`docs/deployment/` documents concrete targets with copy-pasteable manifests: Azure Container Apps (sidecar pattern), Azure Foundry Agent Service (in-process MAF middleware claiming p99 governance latency under 0.1ms), AWS ECS/Fargate, Google Cloud GKE, an OpenClaw sidecar guide (caveated that "OpenClaw does not natively call the governance sidecar" and that container images are not yet published to a public registry), and private-endpoint templates (Azure, AWS, GCP) for zero-trust networking. All emphasize no cloud-vendor lock-in.

`docs/slo/` (despite the name, not service-level-objective policy in the SRE sense, but engineering-process "ticket to completion" artifacts for specific upstream contributions) documents methodology for the prompt-injection fixture corpus and an optional, default-off embedding-based prompt-injection evidence signal, caveated as deriving from a synthetic research corpus and "not production guarantees."

### AGT Studio

AGT Studio is a proposed single unified UI for AGT, formalized by ADR-0028 (status `proposed`). Rationale: AGT currently ships seven fragmented UI surfaces (six Streamlit dashboards, IDE extensions, a Chrome extension, and a static pitch page) with no shared contract. A full operator/SOC console alternative was explicitly considered and rejected, since a write-path console would require SSO, RBAC, multi-tenancy, and 24/7 support AGT cannot sustain, and Sentinel/Defender/Foundry already occupy that space.

The formal contract, `docs/studio/engine-api-contract.md` ("Status: Approved for implementation," tracker `microsoft/agent-governance-toolkit#3011` Epic 0 issue 1/32), defines the HTTP API between the Studio SPA (or VS Code webview) and a local `agt serve` engine process. A maintainer note explains it exists in place of a planned "ADR 0029," later reused for an unrelated decision, so it ships as a standalone versioned spec referencing ADR-0028. A companion OpenAPI 3.1 spec lives at `docs/studio/openapi.yaml` but is not itself a nav page.

Key details: transport is HTTP/1.1 or HTTP/2 over loopback (default `127.0.0.1:8080`), URL-versioned under `/api/v1/`; authentication is loopback-exempt but requires a Bearer token (from `~/.config/agt/studio-token`) for non-loopback connections, except `GET /health` and `GET /versions`. Every endpoint carries three `x-capability-flags` (`runtime_mutating`, `user_intent_required`, `read_only_surface`), and exactly one of the 12 cataloged v1 endpoints is mutating: `POST /policy/save`. The other 11 (`/health`, `/policies`, `/policies/{id}`, `/policy/validate`, `/policy/test`, `/audit/log`, `/trust/scores`, `/trust/graph`, `/agents`, `/decisions`, `/versions`) form a read-only allowlist for a guest-viewer, demo, or CI surface. `POST /api/v1/policy/reload` is excluded as a dangerous, no-visible-effect pattern; `GET /api/v1/events` is a reserved but unimplemented WebSocket path that must return `426 Upgrade Required` until a later epic. A standard error envelope defines status and error codes (e.g. `POLICY_NOT_FOUND`, `ENGINE_UNAVAILABLE`) kept separate from the `GovernanceError` codes in `docs/ERROR_HANDLING.md` (`GOV001`-`GOV006`); standard pagination is required on list endpoints.

No implementation exists yet: a repository-wide search for `*studio*` outside `docs/studio/` and the ADR returns nothing, and the contract lists FastAPI implementation, a capability-metadata decorator, a conformance test suite, and WebSocket transport as still-open dependent work. AGT Studio is spec-first: the API surface is locked down before the engine itself is built.

---

## 21. Repository self-governance and supply-chain security

The AGT repository governs its own supply chain with stdlib-heavy Python and bash tooling under `scripts/`, a deterministic GitHub Actions generation system, internal and consumer-facing composite actions, and a dedicated `tests/` tree that red-teams the tooling itself. This layer is distinct from the `agent-governance-python` runtime packages the toolkit ships to end users (see section 3): it governs the AGT repo, not the agents AGT customers deploy. Nearly all scripts are pure-stdlib or stdlib-plus-`urllib` so they run on any GitHub-hosted runner without extra installs, and several document a fail-closed posture: a network failure on a changed dependency is treated as a finding, not a pass.

### `scripts/` governance checks

| Script | Lines | Detects / does |
|---|---|---|
| `governance_gate.py` | 291 | Agent-deployment gate (not code merges): validates policy YAML fields (`audit.enabled`, `pii_scanning.enabled`, `allowed_tools`, `max_tool_calls`), signs an Ed25519 deployment receipt, appends to a JSONL audit trail. Exit `0`/`1`/`2`. |
| `security_scan.py` | 173 | Thin CLI wrapper around `agent_os.security_skills` (`scan_directory`, `scan_file`, `scan_source`); rule logic lives in that package. |
| `credential_audit.py` | 392 | "Credential laundering": citing merged PRs from a target repo as social proof in issues filed across other repos. |
| `contributor_check.py` | 1,475 | Largest script. Reputation checker for coordinated inauthentic behavior via detectors including `check_account_shape`, `check_repo_themes`, `_check_fork_burst`, `check_feature_overlap`, `check_spray_pattern`, `check_credential_spray`. A dampening/allowlist system (`scripts/contributor_check_allowlist.json`, documented as never bypassing code review, only downgrading auto-flags) reduces scoring for established accounts; fails closed to no exemption if the allowlist is missing/invalid. Backed by four dedicated test files, the most heavily tested script in the directory. |
| `cluster_detect.py` | 482 | Maps coordination networks from a seed account via shared forks, thread co-participation, synchronized issue timing. |
| `check_dependency_confusion.py` | 668 | `pip install <name>` where name is unregistered (typosquat guard); installable as `.git/hooks/pre-commit`. |
| `check_dependency_scorecard.py` | 734 | Queries the OSSF Scorecard API for new direct dependencies, network-constrained to the Scorecard API plus `registry.npmjs.org`, `pypi.org`, `crates.io`, redirects not followed. Runs with `contents: read` and no secrets, so it is explicitly labeled advisory. |
| `check_build_hooks.py` | 95 | PRs adding/modifying `setup.py` or `build.rs`, with a documented fix ("the M2 finding") for root-level `build.rs` missed by `**/build.rs` pathspecs. |
| `check_install_scripts.py` | 396 | npm dependencies whose latest version declares install lifecycle scripts, unless allow-listed. Never trusts the lockfile's own hint as a skip signal (fixed bypass "C4"); unwraps nested `node_modules` keys correctly (fixed bypass "C5"). Fail-closed on scan-deadline expiry. |
| `check_vendor_imports.py` | 116 | Unguarded imports of vendor AI/ML frameworks (langchain, openai, anthropic, etc.) in core packages, enforcing "no hard vendor lock-in"; permitted inside `integrations`/`adapters`/`examples`. |
| `check_lockfile_integrity.py` | 1,002 | Second-largest script. Pinned lockfile hashes versus the registry for npm, Cargo, pip (lockfile poisoning); yarn/pnpm/NuGet documented as future work. Exit `0`-`3`. |
| `check_license_headers.py` | 145 | Missing MIT/Microsoft copyright headers on `.py .ts .cs .rs .go`; exempts the vendored ACS `policy-engine` subtree. |
| `check_gov.py` | 86 | Standalone install/health checker for AGT core packages and critical deps. |
| `ci_complete_check.py` | 41 | Smallest script. Reads a GitHub Actions `needs` JSON context from stdin, reports incomplete required jobs. |
| `_supply_chain_common.py` | 373 | Shared helpers (`REGISTRY_TIMEOUT=10s`, size caps, `SAFE_VERSION_RE`/`SAFE_NAME_RE`, `Deadline` budget class) used by the release-age, install-scripts, and build-hooks checks. |
| `check_release_age.py` | 572 | Dependency versions published under 7 days old, on the premise malicious releases are typically caught and yanked within about a week. Structural `tomllib` parsing (fixed gap "the H2 finding"); a 404 on version lookup is a hard failure. |
| `extract_workflow_shell.py` | 153 | Extracts inline bash blocks from workflow YAML for downstream linting via regex line-scanning. |
| `verify_tutorials.py` | 729 | Not a security gate: runs live code samples for tutorials 35-41 of `agentmesh.governance` against the current SDK. |
| `sync-version.py` | 344 | Synchronizes every package manifest to the version in the repo-root `VERSION` file; `--check` for drift detection. |

Supporting subdirectories:

- **`scripts/ci/`**: `changed_lines.py` (scopes checks to changed files/lines), `generate_workflows.py` (deterministic workflow generator, below), `propose_workflow_updates.py` (optional agentic proposer, never writes YAML directly), `build_acs_python_wheel.sh` (builds the ACS wheel in a digest-pinned `manylinux_2_28_x86_64` image with pinned Rust 1.89.0), `no-custom-crypto.sh` (crypto primitives outside designated security modules), `no-stubs.sh` (`TODO`/`FIXME`/`HACK`/`NotImplementedError` markers in added diff lines only), `no-unauthed-registration.sh` (public-key registration endpoints lacking proof-of-possession, CWE-306), `security-audit-required.sh` (requires a dated audit doc for PRs touching capability-introducing security surfaces), `vendored-patch-audit.sh` (requires a dependency-audit doc for lockfile/vendored changes, with a Dependabot minor/patch exemption).
- **`scripts/docker/`**: `dev-entrypoint.sh` (lazy `npm ci`), `run-tests.sh` (`pytest tests/ -q` across 11 Python packages).
- **`scripts/docs/`**: `check_frontmatter.py` (required fields `title`, `last_reviewed`, `owner`, warn-only by default), `check_links.py` (relative link validation with a baseline file for known-broken links).
- **`scripts/tests/`**: 17 pytest files validating the governance scripts, sized so `contributor_check.py` and `check_lockfile_integrity.py`, plus the release-age/install-scripts/dependency-confusion trio, receive the heaviest scrutiny, consistent with inline "M2/C4/C5/H2 finding" labels in their docstrings.
- **`generate_sbom.py`** (163 lines) and **`diff_sbom.py`** (487 lines): `generate_sbom.py` generates CycloneDX JSON SBOMs per package under `packages/`, with `--audit` running a `pip-audit` CVE scan. `diff_sbom.py` diffs two SPDX-JSON SBOMs (a format mismatch with `generate_sbom.py`'s CycloneDX output) and renders a markdown PR-comment report of package changes by ecosystem.

### Governance files and dependency automation

`.github/CODEOWNERS` sets one rule, `* @MohammadHaroonAbuomar @liamcrumm`, requiring approval from either owner on every PR. `.github/dependabot.yml` covers pip (one multi-directory entry across 13 `agent-governance-python/*` packages with a dev-tooling group to avoid resolver conflicts), npm, nuget, cargo, gomod, docker, and github-actions, each on its own weekly schedule. `.github/labeler.yml` applies path-based auto-labeling, including roughly 18 `integration/<name>` labels for framework adapters (see section 15). `.github/copilot-instructions.md` instructs AI agents/Copilot on architecture, PR standards, an "External Contribution Quality Gate," and security rules (SHA-pin actions, digest-pin Docker images, `permissions: contents: read` by default, no `yaml.load()`/`pickle.loads`/`eval`/`shell=True`, a "7-Day Rule" for dependency ages, no mocks/stubs/TODOs in production code, mechanically enforced by `tests/ci/test_no_stubs.py`). `.github/codecov.yml` sets a project target of 60% (threshold 2%) and a patch target of 70%. `.github/pipelines/esrp-publish.yml` is an Azure DevOps pipeline; all PyPI/npm/NuGet/crates.io publishing goes through ESRP Release via ADO, not GitHub Actions.

### Deterministic workflow generation

`.github/ci/` is a code-generation input, not hand-authored CI:

- `.github/ci/actions.toml`: a pinned Actions registry (`checkout`, `setup-python`, `setup-node`, `setup-dotnet`, `rust-toolchain`), each entry a full 40-character commit SHA with a version comment. The generator fails closed on a missing key or non-SHA entry.
- `.github/ci/workflows.toml`: source of truth for generated CI as nested `[[workflow]]`/`[[workflow.job]]`/`[[workflow.job.step]]` TOML tables. Currently defines one workflow, `policy-engine-ci`, with four jobs (`rust`, `python`, `node`, `dotnet`).
- Regeneration: `python3 scripts/ci/generate_workflows.py --write`; CI verifies via `--check`.
- `.github/workflows/policy-engine-ci.yml` is the only generated workflow, marked `# DO NOT EDIT. Generated by scripts/ci/generate_workflows.py.`. The other 38 workflow files are hand-authored.
- `.github/workflows/ci-generation-check.yml` is the meta-guard (hand-authored by design): runs `generate_workflows.py --check` then `pytest tests/ci -q`.

### `.github/workflows/` inventory

39 workflow files group into: AI-assisted automation using the `ai-agent-runner` composite (`ai-contributor-guide.yml`, `ai-owasp-compliance.yml`, `ai-pr-review.yml`, `ai-release-notes.yml`, `ai-repo-health.yml`, `ai-security-scan.yml`, `ai-spec-drafter.yml`); core CI and generation (`ci.yml` main pipeline with a daily cron, `ci-generation-check.yml`, `policy-engine-ci.yml` generated, `quality-gates.yml`, `workflow-lint.yml`, `codeql.yml`, `cflite.yml` ClusterFuzzLite, `benchmarks.yml`); governance and compliance gates (`agent-governance-gate.yml`, `contributor-check.yml`, `dco.yml`, `policy-validation.yml`, `pr-title-check.yml`, `pr-size.yml`, `labeler.yml`); supply-chain and dependency management (`dependency-review.yml`, `supply-chain-check.yml`, `scorecard.yml` OpenSSF Scorecard, `sbom.yml`, `sbom-diff.yml`, `sbom-diff-comment.yml`, `auto-merge-dependabot.yml`, `license-check.yml`, `license-headers.yml`, `secret-scanning.yml`, `weekly-security-audit.yml`); publishing and docs (`publish.yml`, `publish-containers.yml`, `docs.yml` MkDocs to GitHub Pages, `docs-quality.yml`, `spell-check.yml`); and community/misc (`welcome.yml`, `stale.yml`, `sync-atr-community-rules.yml`).

### Internal composite actions

Distinct from the consumer-facing `action/` directory, `.github/actions/` holds two internal-only composites:

1. **`contributor-check/action.yml`**: delegates to `scripts/contributor_check_action.py`. Inputs `github-token`, `checks` (default `profile,credential`), `target-repo`, `risk-threshold` (default `MEDIUM`). Routes GitHub context values through `env:` rather than `${{ }}` shell interpolation, guarding against shell expansion of an attacker-controlled username.
2. **`ai-agent-runner/action.yml`**: fetches PR/issue context, sends it to an LLM, posts results back. Outputs a comment-safe `response` (HTML-escaped, mention-neutralized, truncated to 60 KiB) and a separate base64 `response-shell-safe` variant, via a shared `lib/sanitize.mjs`. The system prompt marks PR/issue content as untrusted input never to be followed as instructions, wrapping it in a fenced `UNTRUSTED_CONTEXT_JSON` block; every posted comment carries an "untrusted AI-generated analysis" disclaimer. Regression tests (`test_regression_a8_run_marker.py`, `test_regression_a10_output_mode.py`, `test_regression_a11_breaking_changes.py`) pin specific findings from a documented red-team review of this action.

### Consumer-facing actions (`action/`)

The versioned, externally-documented surface, referenced as `microsoft/agent-governance-toolkit/action[/subpath]@v2`. All three manifests are composite, authored by Microsoft, and require a mandatory `toolkit-version` input validated against `^[0-9]+\.[0-9]+\.[0-9]+((a|b|rc)[0-9]+)?$` (accepts `3.7.0`, `3.7.0rc1`; rejects floating versions, local identifiers, VCS refs). This exact-pin requirement is documented as a breaking change from a prior floating-version behavior.

1. **`action/action.yml`** ("Agent Governance Verify"): `command` selects `governance-verify` (`agent_compliance.cli.main verify`), `marketplace-verify` (`agent_marketplace.cli_commands verify`), `policy-evaluate` (inline call to `agent_os.policies.PolicyEvaluator.evaluate()`), or `all`. Outputs `status`, `controls-passed`, `controls-total`, `violations`. Exit `0`/`1`.
2. **`action/security-scan/action.yml`**: invokes `agent_compliance.security.scan_plugin_security()`. Scans secrets (detect-secrets), CVEs (pip-audit/npm audit), dangerous patterns (bandit); Critical/High findings block merge. The `findings-count`/`blocking-count` outputs are hardcoded to `"0"` regardless of actual results; only `status` and the process exit code reflect the true outcome.
3. **`action/governance-attestation/action.yml`**: validates a PR body against `required-sections` (default: a 7-item list covering security, privacy, CELA, responsible AI, accessibility, release readiness, org-specific launch gates), requiring exactly one checked checkbox per section via `agent_compliance.governance.validate_attestation()`.

### `tests/` infrastructure

2,212 total lines across four subdirectories plus two root-level files:

- **`tests/ci/`** (14 files): tests the CI meta-tooling, including `test_ai_agent_sanitize.py`, `test_check_dependency_confusion.py`, `test_generate_workflows.py` (against `.github/ci/*.toml`), `test_no_stubs.py`, and a numbered series of red-team regression tests (`test_regression_a1_a9_atr_npm.py`, `test_regression_a2_toolkit_regex.py`, `test_regression_a8_run_marker.py`, `test_regression_a10_output_mode.py`, `test_regression_a11_breaking_changes.py`, `test_regression_ci_test_swallow.py`), each pinned to a specific red-team finding.
- **`tests/redteam/test_asi.py`** (414 lines): "Red Team Simulation Suite for OWASP ASI Starter Packs," 28 `AdversarialScenario` instantiations run via `pytest tests/redteam/ -v`, expecting a 100% block rate for hardened rules and regenerating `docs/ADVERSARIAL-AUDIT-REPORT.md` (see section 18).
- **`tests/smoke/test_imports.py`** (63 lines): lightweight import smoke tests across installed packages.
- **`tests/test_example_smoke.py`** (180 lines): runs each script under `examples/` as a subprocess, asserting exit code 0 and expected output (see section 19).
- **`tests/unit/test_policy_test.py`** (342 lines): tests the `agt test` policy-replay engine (`FixtureResult`, `ReplayReport`, `replay` from `agent_compliance.policy_test`).

### Fixture schema

`schemas/fixture_schema.json` (JSON Schema draft-07, "Policy Replay Fixture"): required fields `id`, `input` (`action`, `agent_did`, `context`), `expected_verdict` (enum `allow`, `deny`, `audit`, `block`, with `audit`/`block` explicitly documented as reserved for future use since current fixtures use only `allow`/`deny`). Optional `resolution_metadata.strategy` is explicitly informational/audit-only, "not used for pass/fail determination." `schemas/validate_fixture.py` is a standalone CLI/library using `jsonschema.Draft7Validator` when available, falling back to a basic required-field check otherwise.

### Docker and pre-commit packaging

The `Dockerfile` base stage is digest-pinned (`python:3.11-slim@sha256:...`, documented as the single source of truth for reproducibility) and installs a pinned OPA CLI binary with SHA-256 verification. The `dev` stage editable-installs the consolidated v4.0.0 packages plus ten more with extras, then builds the native ACS Python binding (`./policy-engine/sdk/python` via `maturin`) and verifies `import agent_control_specification` succeeds; it runs as a non-root `dev` user. The `test` stage overrides `CMD ["pytest"]`. `docker-compose.yml` defines `dev` (interactive), `test` (runs `scripts/docker/run-tests.sh`), and `dashboard` (opt-in profile, runs a Streamlit app from `agent-hypervisor/examples/dashboard/app.py`). `.pre-commit-hooks.yaml` exposes three hooks for external consumers: `validate-policy` (`agent_os.policies.cli validate`), `validate-plugin-manifest` (`agent_marketplace.hooks validate-manifest`), `evaluate-plugin-policy` (`agent_marketplace.hooks evaluate-policy`).

Of everything described in this section, only `action/action.yml`, `action/security-scan/action.yml`, `action/governance-attestation/action.yml`, and `.pre-commit-hooks.yaml` are published, versioned surfaces consumers install or invoke; the `.github/actions/` composites, all 39 workflow files, the `.github/ci/*.toml` generator inputs, the Dockerfile/docker-compose services, and the `tests/` tree are internal automation governing AGT's own development process.

---

## 22. Ideas, proposals, and roadmap

This section catalogs AGT's forward-looking material: the RFC process for internal proposals, the external-submissions tracker in `docs/proposals/`, in-repo design proposals not yet shipped, and `docs/ROADMAP.md`. Throughout, status markers (Draft, Open PR, Planned, Shipped) distinguish committed work from aspiration; ADR statuses and package-status vocabulary are covered in sections 2 and 3.

### RFC process (`docs/RFC_PROCESS.md`)

An RFC is **required** for: adding or removing public API surface, introducing a new package or framework integration, changing the security model, trust boundaries, or cryptographic choices, modifying the policy engine, privilege rings, or delegation chains, or anything affecting backward compatibility. An RFC is **not required** for bug fixes, doc improvements, test additions, dependency updates, or internal refactors that preserve the public API.

Lifecycle: `Draft -> Under Review -> Accepted / Rejected -> Implemented -> Closed`, tracked via GitHub issue labels (`rfc:review`, `rfc:accepted`, `rfc:rejected`). Minimum review period is 7 days once under review; maintainers aim to respond within 5 business days; RFCs with security implications require sign-off from at least two maintainers. A good RFC states the problem, shows concrete API/types, addresses security implications explicitly, considers alternatives, and plans a migration/deprecation path if breaking. Once accepted, the decision is recorded as an ADR (`docs/adr/`, see section 2) linking back to the RFC issue: RFCs capture proposal and discussion, ADRs capture the durable decision record.

Despite this formal process, none of the 30 files in `docs/proposals/` are framed as internal "RFC issue" artifacts in the strict sense (the sole partial exception is the CoSAI/OASIS WS4 proposal, explicitly typed "RFC" for an external standards body). The proposals directory instead functions as an **external submissions tracker** and a **pre-ADR design-doc scratch space**, a different artifact from the internal-RFC-to-ADR pipeline `RFC_PROCESS.md` describes.

### External proposals and submissions (`docs/proposals/`)

`docs/proposals/README.md` ("External Proposals & Submissions Index," last updated April 26, 2026) tracks **30 external submissions** to standards bodies, ecosystems, and frameworks: Standards & Foundations (9, 0 shipped), Microsoft Ecosystem (3, 1 shipped), Framework Integrations (15, 3 shipped), MCP Ecosystem (1, 0 shipped), Agent Infrastructure (2, 2 shipped): **30 total, 6 shipped, 24 open**.

| Proposal | Target | Status | Core idea |
|---|---|---|---|
| `LFAI-PROPOSAL.md` | LF AI & Data Foundation Sandbox (`lfai/proposing-projects#102`) | Open, awaiting TAC review | House AGT as a vendor-neutral open-source governance kernel |
| `COSAI-WS4-PROPOSAL.md` | CoSAI/OASIS WS4 (`cosai-oasis/ws4-secure-design-agentic-systems#42`) | Open, awaiting WS4 review | Document kernel-based runtime governance (Policy Engine, Capability Sandbox/rings, IATP, Kill Switch) as a reusable secure-design pattern |
| `OWASP-ASI-PROPOSAL.md` | OWASP Agent Security Initiative (open PR) | Open, awaiting review | Contribute insecure/secure code pairs mapped to OWASP Agentic Top 10 risks, covering 3 of 10 risks initially |
| `CSA-ATF-PROPOSAL.md` | CSA Agentic Trust Framework v0.1.0 | Active, async assessment in progress | All 15 requirements mapped across 5 ATF pillars, each marked "Full" coverage |
| `MCP-ECOSYSTEM-PROPOSAL.md` | MCP Registry (2 GitHub issues) | Partially shipped | Governance MCP server published on npm + Glama; registry category entry still pending |
| `ORACLE-AGENTSPEC-PROPOSAL.md` | Oracle Agent Spec (`oracle/agent-spec#125`) | Active, under review | Map ring-based capability declarations, DID-based identity, policy references into Oracle's format |
| `A2A-TRUST-EXTENSIONS-PROPOSAL.md` | A2A Protocol (AAIF) | Adapter shipped | Trust adapter implemented at `agent-governance-python/agentmesh-integrations/a2a-protocol/`; trust extensions proposed upstream |
| `NEXUS-TRUST-EXCHANGE-PROPOSAL.md` | "Visa Network for AI Agents" | Pre-alpha | Registry, reputation engine, escrow, arbiter implemented in `agent-governance-python/agent-os/modules/nexus/`, but crypto is a placeholder (XOR) and persistence is in-memory only |
| `STRIPE-MPP-PROPOSAL.md` | Stripe Machine Payments Protocol | Planned, research complete, not started | Map VADP delegation receipts to Stripe MPP Sessions, bridge Nexus escrow to Stripe |
| `ANTHROPIC-INTEGRATION-PROPOSAL.md` | Anthropic ecosystem (3 submissions) | Open/pending, none merged | Agent Governance Skill, Claude Plugin, Governance Cookbook |
| `AUTOGEN-INTEGRATION-PROPOSAL.md` | Microsoft AutoGen (`microsoft/autogen#7212`) | Open PR | `autogen_ext.governance` module with `GovernancePolicy`, content filtering, rate limiting |
| `GITHUB-COPILOT-PROPOSAL.md` | `github/awesome-copilot` (3 PRs) | Shipped, all 3 merged | Agent Governance Skill, Governance Audit Hook, Safety Instructions + Reviewer Agent |
| `CREWAI-INTEGRATION-PROPOSAL.md` | CrewAI | Open PRs | `crewai.governance.GovernancePolicy` for content filtering, tool control, audit |
| `GOOGLE-ADK-PROPOSAL.md` | Google ADK (`google/adk-python#4543`) | Implemented | ADK GovernanceAdapter shipped at `agentmesh-integrations/adk-agentmesh/`; upstream issue still open |
| `HAYSTACK-INTEGRATION-PROPOSAL.md` | Haystack | Shipped | Package at `agentmesh-integrations/haystack-agentmesh/`, accepted upstream |
| `DIFY-INTEGRATION-PROPOSAL.md` | Dify | Shipped, merged PR #2060 | AgentMesh Trust Layer plugin, live on Dify Marketplace |
| `OPENLIT-INTEGRATION-PROPOSAL.md` | OpenLit | Implemented, upstream PR #1062 under review | OTel auto-instrumentation for Agent SRE (`SLO.evaluate()`, `ChaosExperiment`, etc.) |
| `OPENAI-SWARM-PROPOSAL.md` | OpenAI Swarm | Open PR | Trust-verified handoffs via `swarm.contrib.agentmesh.TrustedSwarm` |
| `METAGPT-INTEGRATION-PROPOSAL.md` | MetaGPT | Open PR | Trust layer via `metagpt.ext.agentmesh.TrustedTeam` |

Two in-repo design proposals not part of the external-submissions README table are notable design-doc scratch space: `folder-level-governance.md` (Draft, issue #1348, path-scoped policy discovery with inheritance for `PolicyEvaluator`) and `RAG-GOVERNANCE-PROPOSAL.md` (issue #1700, proposed `agent-rag-governance` package with `RAGGovernor`, `CollectionACL`, `ContentPolicy`; see section 11 for RAG governance detail). A cluster of sandbox-provider design docs (Docker, Azure Container Apps, Hyperlight, MXC, Nono), all Status: Draft, define a common `SandboxProvider` ABC and are covered in section 6. Two evidence/receipts proposals, `MYCELIUM-EXTERNAL-ANCHOR-PROPOSAL.md` (Draft v3, `EvidenceAnchor` ABC for third-party tamper-evidence verification, issue #2208) and `verifiable-compliance-receipts.md` (Draft, signed compliance receipts with hash chains), extend the compliance and audit story from section 8.

### Roadmap (`docs/ROADMAP.md`)

The document states plainly: "items are not commitments." The header still reads "Current Release: v3.7.0 (Public Preview)," which is stale relative to the repo's actual `VERSION` file (5.0.0) and `CHANGELOG.md` (see section 2's version-posture discrepancy).

**Shipped** (per the roadmap's own list, itself possibly stale): 14 Python core packages plus 20+ framework integrations; 5 SDK languages; 10 formal RFC-2119 specifications with 992 conformance tests; 25 ADRs; 46+ tutorials plus 7 policy-as-code chapters; 13,000+ tests; 10/10 OWASP Agentic coverage claimed; OpenSSF Best Practices 100%; contributor-reputation-screening GitHub Action; unified `agt` CLI (`verify`, `red-team`, `doctor`, `lint-policy`); 12-vector PromptDefense evaluator; OpenClaw sidecar; GHCR container images; `GovernanceEventSink` SPI with circuit breaker.

Forward-looking items are organized into three unchecked, explicitly aspirational horizons:

- **Near-term (next 1-2 releases)**: policy hot-reload without agent restart; Cedar policy language **production** support (implying current Cedar support is non-production); OPA/Rego integration hardening; multi-tenant policy isolation; Entra ID agent identity bridge; SPIFFE/SVID **production** deployment guide; ML-DSA-65 (post-quantum) signing **production** support; Helm chart v1.0 with production defaults; Agent SRE dashboard (Grafana templates); Shadow AI discovery scanner **production** support; ISO 42001 mapping completion; EU AI Act Annex IV automated evidence generation; SOC 2 audit-trail export tooling.
- **Medium-term (3-6 months)**: multi-agent delegation-chain verification; economic scope limits (budget governance); constitutional constraint layer as a community extension (ties to ADR-0006, still "proposed"); agent behavior anomaly detection via trust scoring; foundation project submissions (ties to the LFAI/AAIF proposals); CoSAI/OASIS WS4 reference implementation; cross-project spec alignment.
- **Long-term (6-12 months)**: federated trust across organizational boundaries; formal verification of policy evaluation; hardware-backed agent identity (TPM/SGX).

Roadmap influence mechanism: vote (thumbs-up) on GitHub issues, open a GitHub Discussion, submit an ADR for architectural proposals, or contribute a PR, described as "the strongest signal of priority."

### Package-consolidation proposal

`docs/package-consolidation/PROPOSAL.md` (tracks issue #2482, dated 2026-05-23) is a distinct, still-open architectural proposal, not yet implemented and pending an RFC-process community feedback period per its own closing line, to shrink the repo from 45 Python packages (11 with any PyPI presence) down to 5 top-level distributions: `agent-governance-toolkit` (meta-package), `agent-governance-toolkit-core`, `agent-governance-toolkit-integrations`, `agent-governance-toolkit-cli`, and `agent-governance-toolkit-protocols`. Source code does not move, only packaging metadata; old package names become "thin aliases." Named packages proposed to stay standalone: `agent-discovery`, `agentmesh-lightning`, `agent-rag-governance`, `agentmesh-drift`, `agentmesh-observability`, `agentmesh-marketplace`. Ten unpublished internal kernel modules (`agentmesh-message-bus`, `agentmesh-tool-registry`, `agentmesh-context`, etc.) are proposed to remain internal-only. This proposal itself illustrates the formal 5-value status vocabulary defined in `docs/packages/index.md` (Shipped, Compatibility, Experimental, Proposed, Vendor integration) used throughout the toolkit to separate aspiration from delivery. Note that a related, larger consolidation (45 packages into 5 distributions) is documented as **already released** in CHANGELOG.md's v4.0.0 entry (see section 3); the package-consolidation proposal in `docs/package-consolidation/` should not be conflated with that shipped change, as the two describe overlapping but not identical restructuring efforts at different points in the repository's history.

### Signals for distinguishing idea from shipped code

Recurring textual markers separate proposals from implemented functionality across this whole area: explicit status badges (Planned, Draft, Draft v3, Active, Open PR/issue awaiting review) in the proposals directory; ADR status of `proposed` versus `accepted` (section 2); the `docs/packages/index.md` "Proposed" label; and inline caveats such as "not yet published," "placeholder crypto," "in-memory only," "research complete, implementation not started," embedded directly in individual proposal documents (for example, Nexus Trust Exchange's crypto and persistence gaps, or the Stripe MPP proposal's explicit "not started" status). Conversely, proposals marked Shipped or Implemented (GitHub Copilot, Dify, Haystack, Google ADK, OpenLit, the A2A adapter) consistently cite concrete in-repo file paths as evidence, which is the pattern that reliably separates real functionality from aspirational proposal text throughout the documentation tree.

---

## 23. Project governance, community, and licensing

### Repo-level governance documents

`GOVERNANCE.md` (decision-making and roles), `docs/CHARTER.md` (LF Projects Technical Charter template), `MAINTAINERS.md` (maintainer roster), `SECURITY.md` (vulnerability reporting SLAs), `CODE_OF_CONDUCT.md` (Microsoft Open Source Code of Conduct), `ANTITRUST.md` (competition-law guidance), and `TRADEMARKS.md` (trademark usage policy).

`GOVERNANCE.md` states four principles: open participation, transparent decision-making (architectural decisions discussed publicly via GitHub Issues/Discussions), merit-based advancement, and an explicit "vendor neutrality goal": "The project is working toward multi-organization maintainership to ensure no single vendor controls the project's direction" (an aspiration, not yet fully achieved).

### Contributor ladder

- **Contributor**: anyone submitting a PR, issue, or discussion; must agree to the Code of Conduct and sign the Microsoft CLA (`cla.opensource.microsoft.com`).
- **Reviewer**: can approve PRs in their area but cannot merge without maintainer approval. Path: 3+ merged PRs in an area plus 1+ months of active review/triage.
- **Maintainer**: write access, can merge, participates in architecture decisions. Path: 5+ merged PRs, 2+ months of sustained contribution, active issue triage, nomination by an existing maintainer, consensus confirmation. Maintainers are code owners in `.github/CODEOWNERS`, required to approve every PR before merge.
- **Project Lead**: sets technical direction, resolves disputes, represents the project externally.

### Decision-making and voting

Routine PRs need one maintainer approval. Significant changes (architecture, public API surface, security model, governance scope) are discussed publicly via GitHub Issues first, seeking "rough consensus," with unresolved disputes decided by the project lead. Architecture/API changes and new-maintainer nominations require consensus with 50% maintainer quorum; governance document changes require 2 approvals; project lead succession requires a 2/3 supermajority with 75% quorum.

Succession: a lead vacancy (60+ days inactive) triggers a Core Maintainer supermajority election within 30 days, with the longest-serving Core Maintainer as interim lead; if Core Maintainers drop below three, remaining maintainers must confirm a replacement within 30 days, during which no architecture or governance decisions may be made. Emeritus status applies after 3+ months of inactivity, and maintainers must disclose and recuse from conflicts of interest. Releases follow SemVer (`docs/RELEASE.md`), automated via GitHub Actions with trusted publishing and SLSA build provenance.

### Charter (LF Projects format)

`docs/CHARTER.md` is a Technical Charter for "Agent Governance Toolkit, a Series of LF Projects, LLC," establishing a Technical Steering Committee (TSC) responsible for all technical oversight, with voting members initially the maintainers in `MAINTAINERS.md`. Responsibilities include technical direction, approving project proposals (incubation, deprecation, scope changes), organizing sub-projects/working groups, appointing standards liaisons, and establishing community/security policies. Voting: one vote per member; quorum is 50% present; meeting decisions need a majority of attendees; votes without a meeting need a majority of all members, with unresolved votes referable to the LF Series Manager.

IP policy: copyright is retained by contributors (no CLA-style assignment); MIT is the license for all inbound contributions; DCO sign-off is required; alternative licenses require a two-thirds TSC vote exception, as does amending the charter itself, subject to LF Projects approval.

This is explicitly LF Projects boilerplate, signaling an intended or in-progress foundation transition rather than a completed series agreement: `GOVERNANCE.md` still treats "if applicable" foundation escalation as conditional. `docs/FAQ.md` states "Microsoft has stated the aspiration to move it into a foundation home for shared community stewardship," citing engagement in the OWASP Agent Security Initiative, LF AI & Data Foundation, and CoSAI working groups.

### Maintainers

`MAINTAINERS.md` ("Last updated: May 2026") lists Imran Siddique (Microsoft, `@imran-siddique`) as Project Lead since March 2026, and five Core Maintainers: Jack Batzner (Microsoft), Elton Carr (Microsoft), Kevin Knapp (MythologIQ), Nishar Miya (Dayos), and Prashan Sapkota (Robert Half Inc.). Three of five are non-Microsoft, evidencing real progress toward vendor neutrality. Package Maintainers (registry publish rights) are Siddique, Batzner, and Carr, all Microsoft employees; the sole Emeritus Maintainer is Andrew Lee Rubinger (Aileron). Stated goal: "We especially welcome maintainers from non-Microsoft organizations to strengthen the project's vendor-neutral governance."

### Security reporting, code of conduct, antitrust, and trademarks

`SECURITY.md` is referenced from the README and `GOVERNANCE.md`: "Security vulnerabilities should be reported via SECURITY.md, not through public issues" (tooling: CodeQL, Gitleaks, ClusterFuzzLite, Dependabot, OpenSSF Scorecard; see sections 18 and 21). `CODE_OF_CONDUCT.md` adopts the Microsoft Open Source Code of Conduct. `ANTITRUST.md` prohibits discussion of pricing, market or customer allocation, boycotts, or competitor coordination in any project venue, standard FOSS-foundation boilerplate consistent with the LF Projects charter framing. `TRADEMARKS.md` requires Microsoft trademark/logo usage to follow Microsoft's Trademark & Brand Guidelines and must not imply Microsoft sponsorship in modified versions.

### Licensing

The project is MIT-licensed (`LICENSE`), with zero Azure/Microsoft dependencies in core packages; the policy engine, identity, trust scoring, and execution rings work fully offline, with cloud integrations (Azure AI Foundry deployment guide, Entra ID adapter) offered as optional, separate packages.

### Official sources and impersonation warning

The README warns that the only official sources are the GitHub repo, the `microsoft.github.io` docs site, PyPI user `agentgovtoolkit`, `@microsoft/agent-governance-sdk` on npm, `Microsoft.AgentGovernance.*` on NuGet, and the crates.io crates: "The project team does not maintain or endorse any third-party websites, packages, or documentation sites claiming to be official," requesting impersonation reports via `SECURITY.md`.

### Community and adoption

`docs/ADOPTERS.md` is a self-reported registry across three tiers: Production (Microsoft internal AI agent platform and engineering tools, Dayos), Evaluation/Pilot (Nobulex, GitHub `awesome-copilot`, an Azure internal project, chamber, MythologIQ Labs LLC, GenAI-Gurus, Provedit, Vortex MSP), and Academic/Research (the Data Quality-Aware Agent Governance project).

The community influences direction by voting on GitHub issues, opening a Discussion, submitting an ADR (`docs/adr/`), or contributing PRs, "the strongest signal of priority." No Discord or chat-platform channel is documented; engagement is GitHub-centric plus the standards-body working groups noted above.

---

## 24. Relationship to TrogonAi (this repository)

This repository hosts TrogonAi, a distributed agentic platform for coordinating autonomous AI agents across services and runtimes (Rust workspace under `rsworkspace/`, NATS/JetStream substrate). AGT and TrogonAi attack the same problem class, making autonomous agents governable in production, from opposite ends: AGT is a governance library an agent process imports (in-process middleware, policy objects, framework adapters), while TrogonAi is an infrastructure platform where governance emerges from explicit service boundaries (NATS subject permissions, standalone gateway services, WASM sandboxes). This section maps the two stacks against each other, based on a source-level survey of both.

### 24.1 Conceptual mapping

An important correction to the obvious first guess: TrogonAi's `trogon-decider` crates are not a policy engine. A decider is a typed event-sourcing (CQRS/ES) primitive: `Decider::decide(state, command)` either produces new events or rejects with a domain error, and `trogon-decider-runtime::CommandExecution` persists results to JetStream streams under optimistic-concurrency preconditions. It answers "does this command produce valid new facts", not "is this action allowed". The true architectural analog of AGT's policy layer is the `a2a-gateway` tiered pipeline.

| AGT component | TrogonAi counterpart | Relationship |
|---|---|---|
| ACS policy runtime (`policy-engine/`, PDP/PEP split, fail-closed verdicts at 8 intervention points) | `a2a-gateway` policy pipeline: AAuth, SpiceDB Tier 1, declarative Tier 1, CEL Tier 2, WASM Tier 3 (`crates/a2a-gateway/src/policy/`) | Same shape: I/O-free stateless decision point, caller enforces. Different policy languages (Rego/Cedar vs CEL) |
| Agent OS `PolicyEvaluator` (YAML rules, allow/deny/require_approval) | `Tier2CelEvaluator` (`tier2_cel/evaluator.rs`): flat directory of `.cel` files, hot-reload, first-false-wins, every error branch mapped to `Tier2Decision::Deny` | Both fail closed; AGT by try/except discipline, TrogonAi by hand-mapped error branches |
| AgentMesh identity (self-sovereign `did:mesh:*`, sponsor, lineage) | `a2a-auth-callout` / `trogon-aauth-verify`: OIDC/mTLS-derived NATS identities, AAuth `aa-agent+jwt` with `cnf.jwk` proof of possession | Philosophical split: self-asserted portable credential vs authority-issued, per-hop-verified grant |
| Delegation chains (`ScopeChain`, depth 5-10, capability narrowing) | `trogon-identity-types::act_chain` (depth 8, cycle detection) | Structurally similar; TrogonAi's is a thinner wire type without built-in narrowing verification |
| Trust scoring (0-1000, decay, tiers) | None | Largest identity-side gap: TrogonAi authorization is binary/relational (SpiceDB, CEL), no reputation concept |
| MCP Security Gateway (tool poisoning, drift, typosquatting, hidden instructions) | None on the MCP path: `mcp-nats` is a pure rmcp-over-NATS transport with zero scanning | AGT's most directly importable capability (see 24.3) |
| `MCPResponseScanner` (fixed Python regex/keyword rules) | `a2a-redaction` Tier 3: Ed25519-signed WASM redaction modules, wasmtime fuel/memory bounds | Same intent, inverted mechanism: AGT ships detectors without a sandbox; TrogonAi ships a sandbox without detectors |
| Agent Runtime privilege rings, command denylist | Decider WASM components: `assert_zero_imports` verifies the compiled artifact declares zero host imports before instantiation (`trogon-decider-sim/src/import_check.rs`) | TrogonAi enforces isolation at the artifact level, AGT at the API-wrapper level |
| Hypervisor Merkle/hash-chained audit, Decision BOM | JetStream itself: append-only, replicated, sequence-numbered streams (ADR 0013 treats stream sequence as authoritative) | Provenance-by-cryptographic-proof vs provenance-by-construction |
| Agent SRE (kill switch, SLOs, error budgets, circuit breakers) | None (grep for `KillSwitch|CircuitBreaker|ErrorBudget` across crates: zero hits); `trogon-telemetry`/`trogon-semconv` provide the instrumentation substrate only | Clear gap on TrogonAi's side; NATS primitives suggest a stronger design (see 24.4) |
| Compliance mappings (OWASP ASI, NIST AI RMF, EU AI Act, SOC 2) | None; TrogonAi's governance docs are 15 architectural ADRs | Different genres: deployed-agent regulatory posture vs codebase decision governance |
| Supply-chain scripts, ~41 CI workflows (one TOML-generated), contributor screening | SHA-pinned actions, `permissions: contents: read`, `enforce-cargo-license`, Dependabot; no CODEOWNERS, no SECURITY.md, no SBOM | AGT much broader; TrogonAi's smaller surface is tightly pinned |

### 24.2 The deepest architectural divergences

**Isolation by default vs isolation as advice.** AGT's own threat model recommends running the policy engine "as a separate process or sidecar, not embedded in the agent's own process", but its default packaged form is same-process middleware, and its docs concede that direct stdlib calls bypass it entirely. TrogonAi makes the boundary the default: `a2a-gateway` is a standalone NATS-addressed service, and a caller cannot bypass it because JWT-mint time permissions (`IssuedPermissions::default_for_caller`) grant publish only to `a2a.gateway.>`, never to `a2a.agents.>`. Enforcement lives in the NATS server's permission system, outside any process an agent could compromise. The same holds between platform services: gateway, scheduler, and ARD interact only across authenticated JetStream boundaries.

**Structural vs disciplined fail-closed.** AGT enforces fail-closed with wrapped exception handling, and its history shows the fragility: issue #2992 was a real bug where an errored Rego/Cedar backend fell through to default-allow. TrogonAi's decider path has no default-accept branch to fall into: `decide` returns `Result<Decision, DecideError>` and nothing persists unless the `Ok` path and the write precondition both succeed. The type system removes the bug class AGT patched reactively.

**Determinism as engineering guard vs stated property.** ACS states determinism as an invariant but treats Rego/Cedar/custom dispatchers as black boxes. TrogonAi's guest SDK actively prevents a concrete nondeterminism source: guest deciders must use `BTreeMap` rather than `HashMap` because protobuf map decoding on `wasip2` seeds a hasher from `wasi:random`, which would silently break replay.

**Where AGT is ahead.** AGT has formal RFC 2119 specifications with 992+ conformance tests, an 11-spec catalog, threat-rule corpora, red-team fixtures, compliance mappings, and a dedicated MCP security layer. TrogonAi has none of these genres yet: no normative spec documents for what `a2a-auth-callout` guarantees, no trust-scoring model, no MCP-side scanning, no compliance posture documents, and no SRE/kill-switch layer.

### 24.3 What TrogonAi could adopt from AGT (MIT-licensed; TrogonAi crates are Apache-2.0)

Ordered roughly by effort-to-value:

1. **An `mcp-gateway` crate mirroring `a2a-gateway`'s shape** (highest value). `mcp-nats` deliberately stays policy-free, like `a2a-nats`. A gateway subscribing to `mcp.server.{server_id}.>` could intercept `tools/list` responses (scan-at-discovery for poisoning, typosquatting, hidden instructions) and `tools/call` request/response pairs (drift and response scanning), reusing the existing `WasmtimeSubstrate`. AGT's `MCPSecurityScanner` algorithms port cheaply: the typosquat check (Levenshtein distance 1-2, min length 4) and rug-pull fingerprinting (SHA-256 over description plus schema) are pure functions, and MCP-SECURITY-GATEWAY-1.0's 127-test conformance checklist is a usable acceptance outline without importing any Python.
2. **Fixture-replay testing for Tier-2 CEL.** AGT's `agt test` pattern (`schemas/fixture_schema.json`, `{id, input, expected_verdict}`, exit 1 on mismatch) is a days-not-weeks port given `trogon-decider-test` already implements the same discipline for deciders, including reusable human/TAP output.
3. **A `lint-policy` analog for `.cel` bundles**: parse each rule, verify it references only the five bound variables (`request`, `caller`, `agent`, `task`, `headers`), warn on duplicates and contradictions, mirroring `agt lint-policy` value without Rego/Cedar machinery.
4. **Supply-chain scripts**: `check_lockfile_integrity.py`, `check_dependency_confusion.py` (guards the `trogon-*`/`trogonai-*` prefix namespace if crates are ever published), and `check_release_age.py` (flags under-7-day-old dependencies) are stdlib-only and adapt to `Cargo.lock`/crates.io with modest changes. TrogonAi's monthly grouped Dependabot PR currently has no freshness or scorecard gating. AGT's generate-workflows-from-TOML pattern with a `--check` CI gate also composes well with TrogonAi's already-SHA-pinned style.
5. **Dynamic policy conditions**: a `Tier2DynamicContext` (time windows, token/cost budgets) alongside `Tier2EvaluationContext`, following DYNAMIC-POLICY-CONDITIONS-1.0's additive design rather than encoding budgets inside CEL expressions. Also worth borrowing: ACS's explicit resource limits (`core/src/limits.rs`: input nesting depth, payload caps) since `Tier2EvaluationContext` has no documented caps on `headers`/`params`, a DoS-shaped gap.
6. **The ATR rule corpus** (419 MIT-licensed threat rules in 10 categories, a curated 108-rule set with CVE regression tests) as bootstrap content for CEL predicates; the taxonomy transfers even where the Rego/YAML compiler does not.
7. **Documentation genres, not code**: the compliance-mapping document shape (framework article to Full/Partial/Gap with a named component as evidence, plus the "self-assessment, not certification" disclaimer), the RFC 2119 spec structure for formalizing what `a2a-auth-callout` and `trogon-aauth-verify` already guarantee in code, and AUDIT-COMPLIANCE-1.0's three-tier conformance vocabulary (`trogon-telemetry`/`trogon-semconv` roughly satisfy Levels 1-2 today; Level 3 tamper evidence is absent).

### 24.4 What TrogonAi already does better

- **Mechanically verified sandbox isolation**: `assert_zero_imports` proves a compiled decider component cannot reach any host function, clock, randomness, or network, by construction. Nothing in AGT runs policy or scanning logic in a memory-isolated sandbox; all three of its stacks explicitly disclaim OS-level isolation.
- **Sandboxed, signed last-mile content control**: `a2a-redaction` enforces host-side fuel (10M per call), memory (16 MB), and output bounds on Ed25519-signed WASM modules, and models "the detector abstained" as a typed outcome (`Tier3Refusal`). AGT's response scanner is unsandboxed Python regex with no abstention signal.
- **Compile-time engineering discipline**: workspace-wide `unwrap_used`/`panic`/`expect_used = deny` plus 11 custom Dylint lints (including five that force generated semconv constants at call sites) and a Weaver/Rego semconv CI gate. AGT's equivalent discipline lives in review checklists and "Public Preview" labels, and the drift shows (stub methods that silently no-op, version-string mismatches, README/code divergences documented throughout this document).
- **Anti-enumeration and wire hardening**: opaque six-value `DenialCategory` responses with detail only in server logs, duplicate-security-header rejection, and real RFC 9421-style proof-of-possession over canonical NATS envelopes, where AGT's IATP trust handshake is still a same-process simulation harness.

### 24.5 A kill-switch and audit design sketch on TrogonAi primitives

AGT's kill switch is an in-process object whose enforcement depends on the target agent's own registered termination callback running honestly. TrogonAi's primitives support a stronger design: model the kill switch as a decider aggregate (a `KILL_SWITCH_EVENTS` JetStream stream recording `AgentKilled`/`AgentRevived` events, reusing AGT's `KillReason` taxonomy), project current state into a NATS KV bucket the way ARD projects its catalog, and enforce at the NATS subject-permission layer by revoking publish/consume on the killed agent's subject prefix. Similarly, a tamper-evident audit could add an incremental SHA-256 hash chain (as headers or a companion KV keyed by stream sequence) over JetStream's already durable, replicated, ordered log, achieving AGT's Level-3 audit target on a substrate that is persistent by default, where AGT's shipped commitment engine is admittedly in-memory only.

### 24.6 Bottom line

The systems are complementary rather than competitive. TrogonAi has the stronger enforcement substrate: real process and network boundaries, artifact-verified WASM sandboxing, type-level fail-closed semantics, compile-time discipline. AGT has the broader governance content: threat taxonomies and rule corpora, MCP-specific security algorithms, formal specs and conformance suites, compliance mappings, SRE vocabulary, and supply-chain tooling. The highest-leverage moves for TrogonAi are content imports into its own substrate (an `mcp-gateway` crate carrying AGT's scanning algorithms, ATR rules as CEL predicates, fixture-replay and lint tooling for `.cel` bundles, supply-chain CI scripts) and genre imports for its documentation (specs, compliance mappings, conformance tiers). What should not be imported wholesale: AGT's Python runtime objects (rewrites beat FFI given JetStream equivalents), the Rego/Cedar dual-backend model (duplicates what CEL already covers here), and the numeric trust-score subsystem (a large stateful commitment that deserves its own proposal if ever wanted).
