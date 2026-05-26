# Enterprise Requirements Extracted From AGT

These are the customer-facing controls AGT's spec set implies. We surface them here so we can decide which we adopt as Tier-0 (must ship) vs Tier-1 (backlog) for TrogonStack.

## A. Identity & Access

A1. **Verifiable workload identity** — every agent has a globally unique, cryptographically-rooted identity, separable from any human's identity. (NATS account NKEY meets this; we still need a DID-shaped public identifier for cross-org interop.)

A2. **Sponsor / chain of custody** — creation of an agent is attested by a sponsor whose identity is itself verifiable. Auditors can replay from a single agent ID back to the human or process that authorised its existence.

A3. **Short-lived credentials with automatic rotation** — no static long-lived API keys at the agent boundary. Default ≤ 15 minutes.

A4. **Delegation with monotonic narrowing** — when an agent acts on behalf of a user or another agent, every credential downstream has strictly ≤ scope, ≤ trust, ≤ TTL of its parent.

A5. **Interop with SPIFFE / OIDC / SAML** — enterprise IdPs are non-negotiable. SVIDs must be accepted as principal evidence.

A6. **Kill switch** — operator can revoke any single agent, tenant or capability instantly and synchronously. Active calls fail closed.

A7. **Quarantine state** — operator can soft-isolate an agent (forced R3, synchronous audit) without killing it, for forensic investigation.

## B. Authorization & Policy

B1. **Declarative policy with deterministic operators** — closed, total operator set, no side effects, reproducible decisions.

B2. **Policy hierarchy with deny immutability** — child policy cannot relax parent deny; corporate floor stays the floor.

B3. **Explicit conflict-resolution semantics** — policy authors choose `DENY_OVERRIDES` / `ALLOW_OVERRIDES` / `PRIORITY_FIRST_MATCH` / `MOST_SPECIFIC_WINS` and the choice is visible.

B4. **Fail-closed PDP** — any error in policy evaluation denies the action.

B5. **Policy versioning + signed bundles** — every decision references a version; bundles signed; rollback supported.

B6. **Canary / staged rollout** — new policies deployable to a percentage or a labelled cohort before global enable.

B7. **Approval workflow for sensitive actions** — privileged action classes require explicit (human or service) sign-off recorded in audit.

B8. **Trust-tier-aware decisions** — policy may key on trust tier and ring without re-deriving from rewards each time.

## C. Runtime Mediation (MCP / A2A)

C1. **Two-stage mediation** — every tool call is policed pre-execution and post-execution.

C2. **Per-tool rate limits and quotas** — defaults plus tenant overrides.

C3. **Sensitive-tool elevation prompts** — surface to operator UI; do not silently allow.

C4. **Response content scanners** — prompt injection, credential leak, PII, exfil URLs, hidden instructions, description injection.

C5. **Tool supply-chain controls** — signed catalog, drift detection across 8 dimensions, CVE feed.

C6. **Schema validation** — argument size, type, required fields; large payloads (>1 MB) rejected by default.

C7. **Circuit breakers per downstream MCP server** — automatic isolation of failing servers.

C8. **CVE auto-quarantine** — affected servers blocked until operator clears.

## D. Execution Control

D1. **Ring-based privilege model** — each agent assigned to a ring; ring caps action classes and quotas.

D2. **Time-boxed elevation** — explicit, auditable, default 300 s.

D3. **Action classification** — read / write / external / privileged is a first-class field on every action.

D4. **Saga orchestration with compensation** — multi-step actions are reversible; non-compensable steps explicitly flagged.

## E. Audit & Compliance

E1. **Tamper-evident audit log** — hash chain, periodic signed root, externally anchorable.

E2. **Decision Bill-of-Materials** — every governance decision links to the artifacts (policy versions, trust snapshot, catalog version) it depended on.

E3. **Verifiable compliance receipts** — pre and post signed receipts per action; reproducible via RFC 8785 JCS canonicalisation.

E4. **Per-tenant audit isolation** — no joins across tenants by default.

E5. **Retention + WORM** — operator declares retention; mid-retention deletion needs override and meta-audit.

E6. **Search / export** — auditors can query and export per-tenant records.

E7. **Crosswalk to standards** — OWASP Agentic Top 10, NIST AI RMF, EU AI Act, SOC 2, ISO 42001.

## F. SRE & Reliability

F1. **SLOs declared per agent and tool**, with budget burn alarms.

F2. **Chaos testing** — governance must still emit correct denials/audits under injected failure.

F3. **Golden traces** — known-good baselines; live traces flagged when they deviate structurally.

F4. **Artifact signing pipeline** — every shippable artifact (policy, catalog, redactor bundle, baseline) signed by the build system.

## G. Data Protection

G1. **End-to-end agent encryption** — for sensitive workflows, payloads opaque to gateway / bus operator.

G2. **PII / credential scrubbing** at gateway boundary.

G3. **Data residency tagging** — actions can be restricted to specified regions; egress out of region requires explicit policy match.

G4. **Egress allowlist by DNS / CIDR** — agents cannot reach arbitrary external endpoints.

## H. Tenancy

H1. **Strong tenant boundary** — separate trust store, separate policy bundle, separate audit stream, separate egress policy, separate redaction bundle.

H2. **Tenant-scoped operator roles** — operator powers do not cross tenants without an explicit cross-tenant grant.

H3. **Tenant kill switch** — operator can stop all activity for one tenant without affecting others.

## I. Observability

I1. **Structured decision logs** — both allow and deny decisions, both pre- and post-execution scans.

I2. **OpenTelemetry traces** spanning gateway → policy → downstream → response scan.

I3. **Drift / poisoning dashboards** — surface scanner alerts with tenancy + agent dimensions.

I4. **Trust score telemetry** — score over time, KL-divergence alerts, network propagation graph.

## J. Lifecycle

J1. **Agent onboarding workflow** — sponsor → key issuance → trust bootstrap → policy assignment → ring assignment.

J2. **Agent offboarding** — credential revocation, audit retention preserved, trust history archived.

J3. **Policy lifecycle** — author → review → sign → canary → promote → deprecate.

J4. **Tool catalog lifecycle** — register → sign → diff vs prior → drift review → approve → publish → quarantine.
