# OWASP Agentic Top 10 Mapping

> **Disclaimer**: This document is an internal self-assessment, not a
> certification or third-party audit. It records how TrogonAi's own
> gateway, identity, and redaction crates align with the OWASP Top 10 for
> Agentic Applications (ASI01-ASI10, 2026 edition, the "ASI" taxonomy as
> referenced by the Agent Governance Toolkit). Organizations deploying
> TrogonAi must perform their own compliance assessment with qualified
> auditors before relying on any rating here.

**Reference:** [OWASP Top 10 for Agentic Applications (2026)](https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026/)

**Scope:** the Rust crates under `rsworkspace/crates/` that make up TrogonAi's
A2A gateway (`a2a-gateway`, `a2a-redaction`, `a2a-auth-callout`), identity
stack (`trogon-aauth-verify`, `trogon-identity-types`, `a2a-identity-types`),
webhook ingress (`trogon-gateway`), and the MCP transport layer (`mcp-nats`
and friends). Every rating cites a specific crate, file, and (where useful) a
named type or function that was read directly as evidence; nothing below is
inferred from a crate's name or README alone.

**Rating legend:**

- **Full**: a concrete, currently-shipping control closes the risk for the
  scope in which TrogonAi operates.
- **Partial**: a real control exists but covers only part of the risk, or
  depends on operator configuration, or the analogous mechanism exists for a
  neighboring protocol but not the one the risk describes.
- **Gap**: no control exists in `rsworkspace/crates` today. Where a work
  item is already tracked, it is cited by ID from
  `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md`.

## Coverage summary

| ASI ID | Risk | Rating | Primary evidence |
|--------|------|--------|-------------------|
| ASI01 | Agent Goal Hijack | Partial | `a2a-gateway` Tier 2 CEL (`tier2_cel::evaluator`), Tier 1 declarative and SpiceDB gates |
| ASI02 | Tool Misuse and Exploitation | Partial | `a2a-gateway` three-tier pipeline; no equivalent for MCP (`mcp-nats`) |
| ASI03 | Identity and Privilege Abuse | Full | `a2a-auth-callout` (`IssuedPermissions`, `DenialCategory`), `trogon-aauth-verify` (`nats_pop`, `TokenVerifier`) |
| ASI04 | Agentic Supply Chain Vulnerabilities | Partial | `.github/workflows/sbom.yml`, `.github/workflows/scorecard.yml`, `.github/CODEOWNERS`, `SECURITY.md` (present in the working tree, not yet merged) |
| ASI05 | Unexpected Code Execution | Full | `a2a-redaction` Wasm sandbox (`GUEST_FUEL_PER_CALL`, `MAX_STORE_MEMORY_BYTES`), signed bundles |
| ASI06 | Memory and Context Poisoning | Gap | No memory store, context integrity, or tamper-evident audit chain exists (tracked as WI-20) |
| ASI07 | Insecure Inter-Agent Communication | Full | `a2a-auth-callout` NATS auth callout, `trogon-aauth-verify::nats_pop` proof-of-possession, `NatsHeaders::new_checked` |
| ASI08 | Cascading Agent Failures | Partial | Per-caller inflight gate in `a2a-gateway` (`gw_pull_backpressure.rs`); no circuit breaker (tracked as WI-19, WI-21) |
| ASI09 | Human-Agent Trust Exploitation | Gap | No approval workflow or human-in-the-loop gate; opaque denial responses exist but do not address this risk |
| ASI10 | Rogue Agents | Gap | No behavior monitoring, quarantine, or kill switch (tracked as WI-19) |

**TrogonAi coverage: 3/10 Full, 4/10 Partial, 3/10 Gap.**

This is a materially different profile from AGT's own self-reported 7/10
Full, 3/10 Partial, 0/10 Gap. The difference is largely architectural, not a
sign that TrogonAi is less mature: TrogonAi's strongest controls sit at the
network/protocol boundary (NATS-authenticated, cryptographically verified,
process-isolated) rather than as in-process behavioral middleware, so ASI03,
ASI05, and ASI07 are covered more strongly than AGT's own equivalents. But
TrogonAi has no dynamic agent-behavior-layer controls today (ASI06, ASI09,
ASI10 remain Gaps, and ASI08 is only a static concurrency bound, not a
reactive circuit breaker), because nothing in `rsworkspace/crates` currently
models an "agent" as a monitored, rate-limited, or killable runtime entity
the way AGT's Agent OS and Agent Mesh do. That is the honest gap this
document exists to record.

---

## ASI01 - Agent Goal Hijack

**Risk:** adversarial input (prompt injection, delimiter confusion, role
hijack) overrides an agent's intended goal.

**What exists:** `a2a-gateway` sits in front of every A2A method call and
runs a three-tier authorization pipeline before a request reaches an agent:
Tier 1 relational authorization via SpiceDB (`rsworkspace/crates/a2a-gateway/src/policy/spicedb_tier1.rs`,
`SpiceDbTier1Gate` trait, backed by `authzed::v1` relationship checks), Tier 1
declarative rules (`policy/tier1_declarative/evaluator.rs`), and Tier 2 CEL
(`policy/tier2_cel/evaluator.rs`, `Tier2CelEvaluator::evaluate()` returning
`Tier2Decision::Allow` or `Tier2Decision::Deny{rule}`). All four error paths
in `evaluate()` (bundle-lock poisoning, refresh failure, non-boolean CEL
result, rule execution error) return `Tier2Decision::Deny`, verified by
reading `evaluator.rs` directly - there is no fall-through-to-allow branch.
Operators can author CEL rules that reject requests matching injection or
role-hijack patterns, using the bound `request`/`caller`/`agent`/`task`/
`headers` variables.

**Why Partial, not Full:** Tier 2 CEL is a general-purpose boolean
expression evaluator, not a goal-hijack detector. There is no shipped rule
corpus, pattern library, or prompt-injection classifier - an operator has to
author the CEL rules themselves. `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md` WI-11
("ATR threat-rule corpus ingestion") tracks porting the OWASP ASI-tagged
detection-rule taxonomy into CEL predicates; until that lands, ASI01
coverage depends entirely on what each deployment writes by hand.

**Evidence:** `rsworkspace/crates/a2a-gateway/src/policy/tier2_cel/evaluator.rs`
(`Tier2CelEvaluator::evaluate`), `rsworkspace/crates/a2a-gateway/src/policy/spicedb_tier1.rs`
(`SpiceDbTier1Gate`), `rsworkspace/crates/a2a-gateway/src/policy/tier1_declarative/evaluator.rs`.

---

## ASI02 - Tool Misuse and Exploitation

**Risk:** an agent invokes tools in unintended, unauthorized, or dangerous
ways.

**What exists:** for A2A traffic, `a2a-gateway`'s Tier 1/Tier 2 pipeline
(same evidence as ASI01) authorizes each method call before it reaches an
agent, and Tier 3 redaction (`policy/tier3_redaction/`, backed by
`a2a-redaction`) can rewrite or block outbound artifact content per skill.
`a2a-pack::validate_agent_card_on_read` (`rsworkspace/crates/a2a-pack/src/agent_card_read.rs`,
line 35) validates every AgentCard against its JSON Schema at read time and
rejects malformed cards, closing one tool-definition-integrity angle for A2A.

**Why Partial, not Full:** there is no equivalent authorization layer for
MCP traffic. `mcp-nats` (`rsworkspace/crates/mcp-nats/Cargo.toml`) is
confirmed to be a pure `rmcp`-over-NATS transport binding - its dependency
list is `async-nats`, `bytes`, `futures`, `rmcp`, `jsonrpc-nats`,
`trogon-nats`, `trogon-semconv`, `trogon-std`, with no CEL, Wasm, or
scanning dependency anywhere in the crate. There is no MCP equivalent of
`a2a-gateway`'s Tier 1/2/3 pipeline: an MCP `tools/call` is not authorized,
scanned, or redacted anywhere in `rsworkspace/crates` today. This is the
gap tracked by WI-01 through WI-06 (a proposed `mcp-gateway` crate mirroring
`a2a-gateway`'s shape, plus typosquat/rug-pull/drift/CVE detection ported
from AGT's `MCPSecurityScanner`), none of which have landed yet.

**Evidence:** `rsworkspace/crates/a2a-gateway/src/policy/` (Tier 1/2/3
modules), `rsworkspace/crates/a2a-pack/src/agent_card_read.rs`,
`rsworkspace/crates/mcp-nats/Cargo.toml` (absence of policy dependencies).

---

## ASI03 - Identity and Privilege Abuse

**Risk:** an agent acquires privileges beyond its intended role, or a
spoofed identity is accepted as legitimate.

**What exists:** `a2a-auth-callout` is a NATS auth-callout service that
mints scoped JWTs at connect time. `IssuedPermissions::default_for_caller`
(`rsworkspace/crates/a2a-auth-callout/src/permissions.rs`, line 60) derives
the default publish/subscribe permission set for a given `CallerId`, scoping
it to the caller's own subject space rather than granting broad access.
Denial responses use an opaque six-variant `DenialCategory`
(`rsworkspace/crates/a2a-auth-callout/src/denial_category.rs`, lines 5-12:
`InvalidCredentials`, `UnknownAccount`, `InvalidRequest`,
`VerifierUnavailable`, `InternalError`, `ServiceUnavailable`), so a caller
probing for misconfiguration only ever sees a generic category, with full
detail logged server-side only.

`trogon-aauth-verify` provides the counterpart on the verification side:
`TokenVerifier` (`rsworkspace/crates/trogon-aauth-verify/src/lib.rs`, line
28) accepts an explicit algorithm allow-list of ES256, ES384, and
EdDSA/Ed25519 only (`nats_pop.rs`, line 84 comment: "only ES256, ES384,
EdDSA/Ed25519 are accepted today" - this is a curated allow-list, not
Ed25519-only, and not "any algorithm the underlying JWT library supports"),
and enforces nonce replay protection via a pluggable `ReplayStore` with a
floor on the replay-window TTL (`nats_pop.rs`, lines 19, 103-107).
Separately, `trogon-identity-types` defines a bounded delegation-chain type,
`ActChainEntry` with `MAX_ACT_CHAIN_DEPTH = 8`
(`rsworkspace/crates/trogon-identity-types/src/act_chain.rs`, line 3) and
cycle detection via `act_chain_has_loop` (line 20) on `(agent_id, wkl)`
pairs - but a workspace-wide search shows these are only re-exported from
`trogon-identity-types::lib.rs` today and are not yet called from
`a2a-gateway` or `a2a-auth-callout`. This rating does not rely on the
delegation-chain guard being wired in; it is scored below as a forward
primitive, not active enforcement.

**Why Full:** the combination of scoped-at-mint-time permissions
(`IssuedPermissions`), an opaque denial-category surface that leaks no
verification detail to the caller, an explicit curated algorithm allow-list,
and nonce-based replay protection - all independently verified in source -
closes the identity/privilege-escalation surface described by this risk for
the protocols TrogonAi actually mediates (A2A/AAuth over NATS). This does
not extend to MCP identity (see ASI02), and the bounded-delegation-chain
type described above is not yet load-bearing; this rating would be
downgraded to Partial if the JWT verification or permission-scoping paths
were found to have a gap the delegation-chain type was meant to cover, which
they are not.

**Evidence:** `rsworkspace/crates/a2a-auth-callout/src/permissions.rs` (`IssuedPermissions::default_for_caller`,
line 60), `rsworkspace/crates/a2a-auth-callout/src/denial_category.rs` (lines
5-12), `rsworkspace/crates/trogon-aauth-verify/src/lib.rs` (line 28),
`rsworkspace/crates/trogon-aauth-verify/src/nats_pop.rs` (lines 19, 84,
103-107), `rsworkspace/crates/trogon-identity-types/src/act_chain.rs` (line
3).

---

## ASI04 - Agentic Supply Chain Vulnerabilities

**Risk:** compromised dependencies, plugins, or sub-agents inject malicious
behavior through the supply chain.

**What exists:** at the time of this assessment, the working tree contains
`.github/workflows/sbom.yml` (CycloneDX SBOM generation via `cargo
cyclonedx --format json --all`), `.github/workflows/scorecard.yml` (OSSF Scorecard), `.github/CODEOWNERS`,
and `SECURITY.md` (vulnerability reporting process). All four are new,
untracked files (`git status` shows them as `??`, not yet committed to
`main`), corresponding to work item WI-13. CI also SHA-pins every
third-party GitHub Action in `.github/workflows/ci-rust.yml` and sets
`permissions: contents: read` at workflow and job level; a custom composite
action `.github/actions/enforce-cargo-license` gates license compliance at
the crate-manifest level; and `rsworkspace/deny.toml` runs `cargo deny check
bans` in CI (`ci-rust.yml`) to fail the build if `native-tls`, `openssl`, or
`openssl-sys` re-enter the dependency graph behind any feature flag,
enforcing ADR 0015's TLS-stack policy mechanically rather than by review.

**Why Partial, not Full:** the SBOM/Scorecard/CODEOWNERS/SECURITY.md
additions are present locally but not yet merged, so they are not a
verified, running control at the time of writing - re-check this rating
once they land on `main`. Independent of merge status, there is still no
dependency-confusion guard for the `trogon-*`/`trogonai-*` crate namespace,
no lockfile-hash integrity check against the crates.io registry, and no
freshness gate on newly-published transitive dependencies (Dependabot ships
one grouped monthly PR with no age or reputation gating). These are tracked
as WI-12. There is also no tool-provenance or rug-pull detection for MCP
servers registered at runtime (see ASI02, WI-02).

**Evidence:** `.github/workflows/sbom.yml`, `.github/workflows/scorecard.yml`,
`.github/CODEOWNERS`, `SECURITY.md` (all untracked in `git status` as of this
assessment), `.github/workflows/ci-rust.yml` (SHA-pinned actions, `cargo
deny check bans` step), `rsworkspace/deny.toml`,
`.github/actions/enforce-cargo-license`, `.github/dependabot.yml` (cargo +
github-actions, monthly, grouped, no freshness gate).

---

## ASI05 - Unexpected Code Execution

**Risk:** agent-driven code paths achieve arbitrary code execution beyond
their intended sandbox.

**What exists:** `a2a-redaction`'s Tier 3 pipeline runs untrusted
redaction/rewrite logic inside a `wasmtime` sandbox with host-enforced,
not guest-cooperative, resource bounds. Reading
`rsworkspace/crates/a2a-redaction/src/wasm/engine.rs` directly: fuel
consumption is enabled (`config.consume_fuel(true)`) with a hard cap
`GUEST_FUEL_PER_CALL: u64 = 10_000_000` (line 20) and a memory cap
`MAX_STORE_MEMORY_BYTES: usize = 16 * 1024 * 1024` (line 25) enforced via
`StoreLimitsBuilder::new().memory_size(...)`. Guest modules must additionally
be Ed25519-signed to load: `verify_signed_bundle` in
`rsworkspace/crates/a2a-redaction/src/signed_bundle/verify.rs` (line 51)
checks the bundle signature against an `Ed25519PublicKey` before the Wasm
module is ever instantiated. A compromised or malicious redaction module can
exceed neither CPU (fuel-limited) nor memory (host-limited store), and an
unsigned module cannot load at all in the default configuration.

**Why Full:** this is a host-side, mechanically-enforced sandbox boundary
(not a documented convention an operator must apply), matching or exceeding
the bar this risk describes for the code-execution surface that exists in
TrogonAi today (Tier 3 redaction guest modules). TrogonAi does not currently
run arbitrary agent-authored code anywhere outside this sandbox and the
separately-documented WASM Component Model isolation used by
`trogon-decider-wit`'s guest deciders (verified zero-import components via
`trogon-decider-sim::import_check::assert_zero_imports`), which is a
narrower CQRS/event-sourcing concern outside this document's gateway/
identity/redaction scope but reinforces the same sandboxing posture.

**Evidence:** `rsworkspace/crates/a2a-redaction/src/wasm/engine.rs` (lines
20, 25, 33, 57, 62-63), `rsworkspace/crates/a2a-redaction/src/signed_bundle/verify.rs`
(line 51, `verify_signed_bundle`).

---

## ASI06 - Memory and Context Poisoning

**Risk:** persistent memory, context, or conversation state is manipulated
to corrupt an agent's future decisions.

**What exists:** nothing in `rsworkspace/crates` implements a memory store,
context-integrity check, or tamper-evident audit trail for agent state.
`a2a-gateway` does publish an audit trail today -
`rsworkspace/crates/a2a-gateway/src/audit_ingress.rs` defines
`IngressAuditOutcome::{Allow, Deny}` records addressed to
`{prefix}.a2a.audit.{outcome}.ingress.{skill}`, and
`rsworkspace/crates/a2a-gateway/src/runtime/audit_publish.rs` (`spawn_gateway_audit_publish`)
- but that module's own doc comment states the publish is "fire-and-forget"
and "best-effort" (lines 5, 21): a publish failure is logged as a
`tracing::warn` and does not block or fail the underlying request. This
gives an audit trail when it succeeds, but no durability or integrity
guarantee when it does not, and no cryptographic linkage between entries.
NATS JetStream itself provides an append-only, sequenced, durably-replicated
log substrate for streams that do use it (`trogon-scheduler`, `ard-nats`,
the decider event stores), which gives provenance-by-construction - a
persisted message cannot be silently rewritten in place - but this is not
the same guarantee as a cryptographic hash chain that lets a third party
independently verify integrity without trusting the storage layer, and it
does not apply to the gateway's best-effort audit publish specifically. A
direct search for hash-chain, Merkle, or tamper-evidence logic in
`rsworkspace/crates` finds no audit-chain implementation; the closest
related artifact is ADR 0013, which treats `Trogon-Origin-Stream-Sequence`
as an authoritative ordering header, not an integrity proof.

**Why Gap:** there is no mechanism anywhere in the surveyed crates that
detects or prevents tampering with persisted agent-relevant state, and no
context/memory sandbox analogous to what this risk describes. This is
tracked as WI-20 ("Tamper-evident audit chain over JetStream" - an
incremental SHA-256 hash chain stored as message headers or a companion KV
bucket, with a stream-replay `verify_chain()` check) and WI-18 (assessing
`trogon-telemetry`/`trogon-semconv` against a three-tier audit-conformance
vocabulary). Neither has landed.

**Evidence of absence:** no crate under `rsworkspace/crates` matching
memory-guard, context-integrity, or hash-chain naming;
`rsworkspace/crates/a2a-gateway/src/runtime/audit_publish.rs` (lines 5, 21,
"fire-and-forget"/"best-effort") documents the current audit path's own
limits; `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md` WI-18 and WI-20 confirm this is
a known, tracked gap rather than an unexamined one.

---

## ASI07 - Insecure Inter-Agent Communication

**Risk:** messages between agents lack authentication, integrity
verification, or replay protection.

**What exists:** every NATS message that carries security-sensitive
identity claims is authenticated end to end. `a2a-auth-callout` performs the
NATS auth-callout handshake (subscriber, JWT minting) at connect time.
`trogon-aauth-verify::nats_pop` implements RFC 9421-style signature
verification over the canonical NATS message envelope (subject, reply,
content-digest, signature-params with JWK-thumbprint keyid) - this is a real
cryptographic binding checked against the agent's actual `cnf.jwk`, not a
same-process simulation. The same module defends against header smuggling:
`NatsHeaders::new_checked` (`rsworkspace/crates/trogon-aauth-verify/src/nats_pop.rs`,
line 147) rejects a request if any security-sensitive header
(case-insensitively) appears more than once, via
`ensure_no_duplicate_security_headers` (lines 152, 155, and enforced again
at line 218 on the raw pre-view headers so a caller cannot bypass the check
by constructing an unchecked `NatsHeaders` value first). Nonce replay
protection (see ASI03) closes the replay angle specifically.

**Why Full:** authentication, message-integrity binding, replay protection,
and header-smuggling defense are all present and independently verified in
source, covering the inter-agent transport TrogonAi actually uses (NATS).
This is scoped to A2A/AAuth-mediated traffic; MCP transport
(`mcp-nats`) inherits NATS-level authentication but has no protocol-specific
scanning layer of its own (see ASI02).

**Evidence:** `rsworkspace/crates/a2a-auth-callout/src/subscriber.rs`,
`rsworkspace/crates/trogon-aauth-verify/src/nats_pop.rs` (lines 124, 147,
152, 155, 218).

---

## ASI08 - Cascading Agent Failures

**Risk:** a failure or fault in one agent or component propagates
uncontrolled through the system.

**What exists:** `a2a-gateway` bounds per-caller concurrency at the pull
consumer layer. `rsworkspace/crates/a2a-gateway/src/gw_pull_backpressure.rs`
defines `DEFAULT_MAX_INFLIGHT_PER_CALLER: usize = 32` (line 39) and a
`CallerInflightGate` (line 342) whose `try_acquire` (line 361) refuses to
admit more than the configured number of concurrent in-flight requests per
caller, NAK-ing (not blocking) when the gate is full so one noisy caller
cannot starve others. `acp-nats` has an analogous per-connection
`InFlightSlotGuard`. JetStream's own `AckPolicy`/`MAX_ACK_PENDING` bounds add
a second layer of flow control at the consumer level.

**Why Partial, not Full or Gap:** a per-caller admission-control gate is a
genuine fault-isolation primitive (it stops one caller's burst from
consuming unbounded gateway resources), but it is a static concurrency cap,
not a circuit breaker: there is no trip/open/half-open state machine that
reacts to a rising failure rate, no automatic recovery/backoff policy, and a
direct search for `CircuitBreaker`, `RateLimiter`, or `TokenBucket` across
`rsworkspace/crates` returns no matches. `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md`
states plainly: "No kill switch, circuit breaker, SLO object, or
tamper-evident audit exists in `rsworkspace/crates` today," which is
accurate for the specific mechanism this risk names even though the
adjacent backpressure control exists. WI-19 (kill switch) and WI-21
(SLO-as-code) track closing the remainder.

**Evidence:** `rsworkspace/crates/a2a-gateway/src/gw_pull_backpressure.rs`
(lines 39, 342, 361), `rsworkspace/crates/acp-nats/src/in_flight_slot_guard.rs`;
absence of any `CircuitBreaker`/`RateLimiter` type confirmed by direct
search across `rsworkspace/crates`.

---

## ASI09 - Human-Agent Trust Exploitation

**Risk:** humans over-trust agent output, or an agent exploits human
oversight gaps to skip required validation or approval.

**What exists:** `a2a-auth-callout`'s opaque `DenialCategory` responses (see
ASI03) prevent an attacker from learning why a request was denied, which is
a defensive property but not a human-approval control. Nothing in
`rsworkspace/crates` implements an approval-workflow gate, a
human-in-the-loop checkpoint for high-risk or irreversible actions, or a
"reversibility" check comparable to what this risk targets.

**Why Gap:** there is no evidence of any control addressing this risk
specifically. Unlike ASI06 and ASI08, this gap is not yet named in
`MS_AGENT_GOV_TOOLKIT_WORKITEMS.md` under a work item ID - it should be
scoped as a new item if TrogonAi decides to build an approval-gate
primitive (for example, as a decider-pattern aggregate requiring an
`ApprovalGranted` event before certain command types can execute, mirroring
the kill-switch design proposed for WI-19).

**Evidence of absence:** no approval-workflow, human-in-the-loop, or
reversibility-check type found under `rsworkspace/crates`.

---

## ASI10 - Rogue Agents

**Risk:** an agent deviates from its intended behavior and continues acting
after it should have been stopped.

**What exists:** no behavior-monitoring, anomaly-detection, quarantine, or
kill-switch mechanism exists in `rsworkspace/crates`. The strongest adjacent
primitive is architectural: `a2a-auth-callout`'s `IssuedPermissions` scope
what subjects a caller can publish/subscribe to at connect time, so a
compromised agent's blast radius is bounded by its granted NATS permissions
rather than unbounded - but this is a static authorization boundary, not a
dynamic behavioral kill mechanism that can react to an agent going rogue
after being granted permissions.

**Why Gap:** WI-19 explicitly proposes closing this: a `KILL_SWITCH_EVENTS`
JetStream stream recording `AgentKilled`/`AgentRevived` events, projected
into a NATS KV bucket the way `ard-nats` projects its catalog, with
enforcement via revoking publish/consume on the killed agent's subject
prefix - a structurally stronger design than a callback-based kill switch
that depends on the target agent honestly running its own termination
handler. This design has not been implemented; there is no current way to
kill or quarantine a misbehaving agent in this codebase.

**Evidence of absence:** no behavior-monitor, quarantine, or kill-switch
type found under `rsworkspace/crates`; `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md`
WI-19 confirms this is a tracked, scoped, not-yet-built gap.

---

## Notes on method

Every Full or Partial rating above cites a file path and, where practical, a
specific type or function name that was read directly from source during
this assessment (not inferred from crate names, README summaries, or the
per-domain analysis notes that fed into this document's background
research). Where a claim could not be verified against source, the rating
was set to Gap or Partial rather than Full. Ratings for ASI04 in particular
should be re-checked once the currently-untracked SBOM/Scorecard/CODEOWNERS/
SECURITY.md additions are committed and merged, since a file's presence in a
local working tree is not the same as a running, merged control.

This document does not cover TrogonAi's decider/event-sourcing runtime
(`trogon-decider*`) or scheduler (`trogon-scheduler*`) crates in depth; those
are CQRS/ES primitives rather than agent-action gateways, and a separate
assessment would be needed if they are brought into scope for a future
revision.
