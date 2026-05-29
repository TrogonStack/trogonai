# ADR 0017: MCP Gateway Product Positioning

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) -- Option A: feature inside TrogonStack |
| **Date** | 2026-05-29 |
| **Deciders** | Yordis Prieto (engineering owner exercising decision authority in absence of separate product DRI) |
| **Blocks** | Product positioning (feature inside TrogonStack vs. standalone security product); gates on-bus vs hybrid, K8s controller, xDS, bundle distribution defaults, OSS envelope, and public naming |
| **Related** | [Agent identity overview](../identity/overview.md); [MCP gateway operator overview](../identity/mcp-gateway-operator-overview.md); [ADR 0007](0007-on-bus-vs-hybrid.md); [ADR 0010](0010-bundle-format.md); [ADR 0011](0011-nats-auth-callout.md); [ADR 0012](0012-rate-limit-state.md) |

## Context

`MCP_GATEWAY_PLAN.md` Block A item 1 is the strategic fork that every other gateway decision branches on: whether the MCP gateway is a **feature inside TrogonStack** or a **standalone security and governance product**. Leadership has **not** chosen either path. Engineering has proceeded with reversible technical ADRs (tenancy, on-bus default, bundle format, rate-limit placement) while explicitly treating positioning as open. This ADR records the question, the operational shapes of each option, downstream dependencies, and the evidence leadership needs -- it does **not** select a winner.

The fork is **product and go-to-market**, not wire-protocol. The same NATS subject grammar, JSON-RPC semantics, CEL + SpiceDB policy model, and JetStream audit envelope can ship under either positioning. What changes is **scope, sequencing, commercial packaging, support boundary, documentation home, and default posture** when two good-faith operators configure the same binary differently.

### Why this decision branches everything else

| Downstream domain | How positioning rewrites defaults |
|---|---|
| **Distribution channel** | TrogonStack feature: gateway ships in platform install/upgrade, version-locked to decider/registry/STS. Standalone product: gateway + policy engine as independently versioned SKU with its own release train and compatibility matrix. |
| **Billing surface** | Feature: MCP gateway capacity folded into TrogonStack tenant/platform metering (agents, audit volume, NATS accounts). Standalone: per-cluster or per-tenant security SKU (gateway replicas, policy evaluations, bundle seats, SIEM export). |
| **Deployment topology** | Feature: assumes Trogon identity stack (auth callout, STS, registry, SpiceDB, JetStream audit) as co-requisites; gateway is one queue group in an opinionated reference architecture. Standalone: must document minimal viable perimeter (Protect + gateway? gateway + customer IdP only?) and tolerate partial Trogon adoption. |
| **Support boundary** | Feature: platform SRE owns end-to-end "agent cannot call tool" incidents across NATS, gateway, STS, registry, SpiceDB. Standalone: gateway vendor support stops at policy/audit contract unless customer buys platform attach. |
| **Naming and OSS scope** | Feature: `trogon-mcp-gateway` crate, `docs/identity/` as canonical home, TrogonStack README as front door. Standalone: public project name, separate repo or monorepo subtree, license envelope (Apache vs BSL vs dual), trademark on "MCP gateway" vs "Trogon MCP Gateway". |
| **Documentation tree** | Feature: operator docs stay under `docs/identity/` beside STS/registry specs. Standalone: likely `docs/mcp-gateway/` product tree with identity as integration guide, not primary narrative. |
| **Engine extraction timing** | Feature: defer `trogon-policy-core` until ACP/A2A pull (per plan Take item 4). Standalone: earlier extraction to sell multi-protocol policy without shipping full TrogonStack. |
| **Competitive framing** | Feature: differentiator is MCP-aware governance inside an event-modeling agent platform (schema redaction, `tools/list` shaping, mesh lineage). Standalone: differentiator is NATS-native security layer vs Synadia Protect / HTTP MCP gateways -- closer to "build Protect-shaped but JSON-RPC-aware." |
| **Day-zero policy posture** | Feature: empty bundle default-deny aligns with platform "enforce before agents run." Standalone: prospects may demand audit-only or shadow mode defaults to land beside existing MCP servers without blocking IDE workflows -- opposite default-deny pressure. |
| **Operational tooling priority** | Feature: registry runbook, STS probe, decider audit intersection first. Standalone: `agctl` bundle verify, OCI publish, multi-cluster bundle promotion, xDS/K8s controller rise on roadmap. |

The plan's **Take** section already argues for "MCP-aware gateway as TrogonStack feature, not standalone Protect competitor" -- but that is **engineering recommendation under one assumed pitch**, not a recorded leadership decision. ADR 0017 exists so implementers can cite *"positioning open; do not ratchet via code layout"* while Block A item 1 remains unchecked.

### Evidence from existing paper specs (neutral read)

[Agent identity overview](../identity/overview.md) describes mesh identity, STS, registry, and gateway egress as **one Trogon mesh story** -- bootstrap at CONNECT, per-hop exchange, SpiceDB on gated methods. Nothing in that document requires standalone product positioning; it **assumes** co-deployed Trogon control plane.

[MCP gateway operator overview](../identity/mcp-gateway-operator-overview.md) is written for **platform / SRE engineers who know NATS and Trogon identity**. It places the gateway beside auth callout, STS, registry, SpiceDB, and JetStream audit in a single topology diagram. That framing is natural for **feature** positioning and would need reframing (optional components, minimal installs, competitive install guides) for **standalone** positioning.

Neither spec resolves Block A item 1. They describe **how the gateway behaves when Trogon identity is present**, not **what company we are selling**.

### What stays undecided until leadership chooses

Until Block A item 1 is **Accepted** with an explicit verdict:

- Bundle registry **primary** path (OCI-first per ADR 0010 vs NATS KV-first for air-gapped platform installs) remains a tension, not a product default.
- Whether Block G K8s controller and xDS interop are **v2 platform convenience** or **v1 standalone requirement** stays unscheduled.
- Public-facing name (`trogon-mcp-gateway` vs neutral `mcp-gateway` / separate org) stays unset.
- OSS license envelope for policy bundles and gateway binary stays unset (contributor expectations, competitive fork risk).
- Support SLA tiers (gateway-only vs platform) cannot be published.
- `docs/identity/` vs future `docs/mcp-gateway/` split cannot be executed.
- On-bus vs hybrid ([ADR 0007](0007-on-bus-vs-hybrid.md)) can proceed as **technical default**, but **third-party ecosystem obligation** (stdio bridge vs mandatory HTTP ingress) is partly a positioning question: standalone security buyers may require HTTP MCP ingress earlier than platform buyers standardizing on NATS.

---

## Decision

**Adopt Option A: the MCP gateway is a feature inside TrogonStack.**

Rationale, in three sentences: (1) every Accepted ADR (0001-0016, 0018-0032) and every shipped identity / operator doc was written assuming co-deployed Trogon mesh; reversing to Option B would require amending or invalidating sixteen Accepted records and re-homing the entire `docs/identity/` tree. (2) The plan's **Take** explicitly warns against "building Protect by itself" -- Option B contradicts that warning and forces months of generic-platform work before the MCP-specific differentiators (schema redaction, `tools/list` shaping, mesh lineage) reach market. (3) No standalone deal, signed partnership, or named pipeline currently requires positioning ambiguity; the cost of waiting for the perfect verdict exceeds the cost of being wrong and re-pivoting via a future ADR amendment.

The two operational shapes (Option A / Option B) remain documented below as the comparison record; **Option A is the chosen path** unless and until a future ADR amends this record per the rules in `Resolution criteria`.

### Authority and scope of this decision

This ADR is being moved to Accepted by the engineering owner under explicit standing authorization ("for all the blockers, make a decision, document it but don't wait for me"). It is binding for **engineering ratchets** (default-deny day-zero, KV-primary bundle distribution, `docs/identity/` as canonical home, deferred Block G K8s controller / xDS, deferred policy engine extraction). It is **not** a commercial pricing record; SKU, SLA, license envelope, and public marketing language remain to be drafted under Option A framing -- those documents must cite this ADR but are out of scope here.

### What Option A ratchets in immediately

| Ratchet | Effect |
|---|---|
| Default-deny day-zero | `bootstrap-day-zero.md` and ADR 0021 stay as the canonical posture; shadow mode is operator opt-in, not product default. |
| Bundle distribution | KV-primary at runtime, OCI-secondary for CI/CD (per ADR 0010); `mcp-pack` is platform-shipped, not marketplace-neutral. |
| Documentation home | `docs/identity/` remains canonical; no `docs/mcp-gateway/` tree is created. |
| Crate / repo naming | `trogon-mcp-gateway` stays; no separate GitHub org or public rename. |
| Engine extraction | `trogon-policy-core` / `trogon-policy-cel` split stays deferred (ADR 0029) until a second protocol (ACP, A2A) pulls on it. |
| Block G priorities | K8s controller + xDS interop drop to v2; latency baseline, CLI, multi-region, OTel export stay v1. |
| Hybrid HTTP ingress | Phase 3+ optional per ADR 0007; not a Phase 2 deliverable. |
| Competitive frame | Complementary to Synadia Protect (subject policy); differentiator vs agentgateway is NATS-native + JSON-RPC-aware redaction. |

### What this decision does NOT do

- Does **not** fix commercial pricing, SLA tiers, license envelope, or trademark.
- Does **not** preclude a future Option A → Option B pivot via amendment ADR; the amendment must include a customer communication plan and compatibility window.
- Does **not** answer Open question items 1-20 below; those remain useful evidence for the pricing / launch / GTM workstreams that flow from this verdict.

### Option A: Feature inside TrogonStack

**Definition.** The MCP gateway is a **platform capability** of TrogonStack's agentic / event-modeling story. Customers buy or deploy TrogonStack; MCP governance is included or licensed as part of that platform, not as a separable security SKU.

| Dimension | Concrete operational shape |
|---|---|
| **Install unit** | Gateway binary (`trogon-mcp-gateway`) deployed with reference TrogonStack stack: NATS (+ auth callout config), `trogon-sts`, registry controller, SpiceDB, JetStream `MCP_AUDIT`, KV buckets (`mcp-sessions`, `mcp-gateway-config`, `mcp-agent-registry`, `mcp-trust-bundles`). |
| **Version coupling** | Gateway releases tracked to TrogonStack minor versions; breaking JWT/audit/subject changes coordinated with STS and registry in same release notes. |
| **Customer obligation** | Adopt Trogon identity model ([overview](../identity/overview.md)): bootstrap vs mesh, registry-backed agents, enforce-mode cutover. Third-party MCP via documented bridge (`mcp-nats-stdio`), not gateway-as-IdP-for-arbitrary-SaaS unless Block A item 2 hybrid triggers fire. |
| **Policy packaging** | First-party `mcp-pack` bundle maintained by platform team; tenants extend via org/tenant overrides in KV. OCI for bundles is **CI/CD convenience**; KV mirror is **runtime source of truth** on cluster. |
| **Commercial** | No separate "MCP Gateway Enterprise" price list; capacity limits appear in platform entitlements (audit GB, gateway replicas, SpiceDB checks). |
| **Support** | Single ticket queue: "MCP call denied" triages across gateway, STS, registry, SpiceDB, NATS ACL. |
| **Docs home** | `docs/identity/` remains canonical; gateway plan is engineering backlog inside monorepo. |
| **Roadmap bias** | Phase 2--3 focus: schema redaction, `tools/list` filtering, session KV, mesh enforce -- **not** generic NATS protocol proxy, not xDS controller in v1. |
| **Competitive answer** | "Use Synadia Protect for raw NATS subject policy; use Trogon MCP gateway for JSON-RPC MCP semantics inside our platform." |
| **Success metric** | TrogonStack design partners run agents with gated `tools/call` on platform NATS without bolting a separate security vendor. |

### Option B: Standalone security product

**Definition.** The MCP gateway (and eventually extracted policy engine) is **the product**: security and governance for NATS-based agentic systems, sellable without full TrogonStack decider/event-modeling adoption.

| Dimension | Concrete operational shape |
|---|---|
| **Install unit** | Gateway + policy distribution + audit export as **minimum SKU**; Trogon STS/registry/SpiceDB become **optional integrations** or replaceable equivalents (customer IdP, OPA, custom PDP). |
| **Version coupling** | Independent semver for gateway and `trogon-policy-*` crates; compatibility matrix documents which identity stacks are certified. |
| **Customer obligation** | Bring your own NATS, IdP, PDP; gateway documents **integration contracts** (JWT claim shape, audit envelope, bundle format) rather than full mesh rollout. |
| **Policy packaging** | Signed bundles and OCI registry as **primary** artifact path; KV as optional; emphasis on `agctl` offline verify and customer-owned registry. |
| **Commercial** | Distinct SKU: gateway replicas, policy evaluations/month, bundle signing keys, premium support for WASM tier. |
| **Support** | Gateway vendor boundary at JSON-RPC policy + audit schema; "STS down" may be customer responsibility unless platform attach sold. |
| **Docs home** | New `docs/mcp-gateway/` (or separate repository) as product manual; `docs/identity/` becomes **integration guide** for Trogon-native deployments. |
| **Roadmap bias** | Earlier investment in generic policy engine extraction, HTTP MCP ingress (hybrid), xDS/K8s controller, Protect-shaped perimeter story, multi-protocol bundles. |
| **Competitive answer** | "JSON-RPC-aware governance on NATS" vs agentgateway (HTTP) and Protect (NATS subjects, not MCP methods). |
| **Success metric** | Revenue from teams running MCP on NATS **without** adopting Trogon decider/event modeling; repeatable land-expand from security evaluation. |

### Historical context: prior deferral (now resolved)

The earlier draft of this ADR explicitly deferred the decision and recommended that engineering treat `MCP_GATEWAY_PLAN.md` Take as recommendation only. That deferral is **closed** with this acceptance. Implementers may now cite ADR 0017 (Accepted, Option A) instead of carrying "positioning open; do not ratchet" disclaimers.

---

## Consequences

Consequences are listed **per positioning path** where they diverge, plus **shared** impacts while status is Proposed.

### While Proposed (both paths)

| Effect | Detail |
|---|---|
| **Parallel technical ADRs remain valid** | ADRs 0001--0016 assume gateway **mechanics**; they do not supersede this ADR. Teams cite ADR 0017 when asked "are we a product yet?" -- answer is no. |
| **Block A item 2--3 stay coupled** | On-bus vs hybrid and tenancy boundary can be papered, but **ecosystem scope** (third-party MCP mandatory vs optional) should be revisited after positioning resolves. |
| **Risk of implicit drift** | Code in `trogon-mcp-gateway`, docs in `docs/identity/`, and plan Take language **bias toward Option A** without a decision record -- mitigated by keeping this ADR Proposed and blocking marketing/OSS commits that assert standalone product. |

### If leadership accepts Option A (feature inside TrogonStack)

| Positive | Negative |
|---|---|
| Faster Phase 2--3 delivery by not building xDS/controller/multi-protocol extraction early | Harder to sell to teams that only want MCP security without Trogon platform |
| Single support and upgrade story for design partners | Bundle OCI-first vs KV-first debate resolves toward **KV runtime + OCI CI** per ADR 0010 |
| Identity specs stay unified under `docs/identity/` | Standalone-minded contributors may fork or duplicate docs |
| Clear competitive story vs raw Protect | Revenue capped by platform deals, not security-only land |

**Blocked items that unlock after Accept (Option A):**

- Publish TrogonStack reference architecture diagram with gateway as mandatory queue group.
- Finalize `mcp-pack` as platform-shipped bundle, not marketplace-neutral pack.
- Defer Block G K8s controller / xDS to **v2 unless** enterprise platform customer demands.
- Set day-zero bundle default to **default-deny** with documented break-glass for platform ops.
- Keep crate naming `trogon-mcp-gateway`; no separate GitHub org.

### If leadership accepts Option B (standalone security product)

| Positive | Negative |
|---|---|
| Addressable market for NATS+MCP security without full platform sale | Months of platform work (generic engine, HTTP ingress, certification matrix) before differentiated MCP features |
| Independent release cadence and OSS community potential | Splits engineering focus from TrogonStack core (decider, event modeling) |
| OCI-first bundle distribution aligns with security buyer tooling | Trogon identity stack may be perceived as lock-in unless integration guides are excellent |
| Clearer competitive lane vs Protect-only | Risk of rebuilding Protect + agentgateway combined scope (plan Take warns against) |

**Blocked items that unlock after Accept (Option B):**

- Split or alias documentation tree (`docs/mcp-gateway/`).
- Prioritize hybrid HTTP MCP ingress and minimal-install guide (gateway + NATS + IdP + PDP options).
- Accelerate `trogon-policy-core` extraction and multi-protocol bundle host ABI.
- Decide OSS license and trademark for standalone name.
- Publish standalone pricing, support SLA, and compatibility matrix (STS optional? SpiceDB required?).
- Revisit ADR 0010 distribution: **OCI primary, KV fallback** as default product path.
- Block G K8s controller / xDS becomes **candidate v1** for enterprises without NATS KV GitOps.

### Downstream items that cannot land cleanly until positioning is picked

| Item | Why it forks on positioning |
|---|---|
| **Bundle registry choice (OCI vs NATS KV primary)** | ADR 0010 accepts OCI artifact + KV mirror; **which path is default onboarding** is product positioning (platform KV watch vs customer GHCR). |
| **Pricing model** | Cannot list SKUs or entitlements. |
| **Support SLAs** | Cannot define incident ownership boundary. |
| **Public-facing project name** | Crate, repo, docs URL, marketing site. |
| **`docs/identity/` vs `docs/mcp-gateway/`** | Information architecture and Diataxis tutorial entrypoints. |
| **OSS license envelope** | Platform feature often ships BSL or source-available; standalone security may need Apache-2.0 for adoption -- legal input required. |
| **Block G: K8s controller, xDS** | Platform feature defers; standalone may require for non-NATS-KV enterprises. |
| **Failure-mode matrix default-deny table** | Standalone prospects may require documented fail-open shadows for migration; platform enforces fail-closed ([operator overview](../identity/mcp-gateway-operator-overview.md) §8). |
| **First-party bundle branding** | `mcp-pack` vs neutral `baseline-policy-pack`. |
| **Integration with `trogon-decider`** | Platform feature emphasizes audit-as-events intersection; standalone may treat decider as optional sink. |
| **Competitive messaging vs Synadia Protect** | Feature: complementary; Standalone: partial overlap narrative must be approved by leadership. |

---

## Rejected alternatives

### Pick neither: run dual-mode forever (feature and standalone simultaneously)

**Proposal.** Ship one binary and one doc tree that fully supports both go-to-market motions indefinitely: TrogonStack-embedded defaults **and** standalone minimal install **and** distinct commercial SKUs **without** choosing primary positioning.

**Rejection.** The two modes imply **incompatible defaults** that cannot be reconciled with configuration alone:

| Conflict | Feature-biased default | Standalone-biased default |
|---|---|---|
| Day-zero empty bundle | Default-deny (platform enforce) | Shadow or audit-only migrate-in |
| Identity stack | STS + registry required for enforce | JWT + external PDP sufficient |
| Bundle source of truth | KV watch on cluster | OCI registry in customer CI |
| Support escalation | Platform owns SpiceDB/STS outages | Customer owns optional components |
| Roadmap priority | Schema redaction, mesh enforce | HTTP ingress, xDS, policy-core extraction |
| Documentation entry | "You run TrogonStack" | "You run NATS + gateway" |

Maintaining two primary stories splits QA matrices, doubles example architectures, and forces every ADR to qualify "unless standalone." Sales and support cannot quote SLAs. **Dual-mode forever is rejected**; leadership may choose a **primary** with a documented **secondary attach** (e.g., standalone gateway with Trogon identity add-on), not perpetual equality.

### Decide implicitly by where code lands ("let the repo structure choose")

**Proposal.** Avoid an explicit leadership decision; infer positioning from whichever team merges code first (e.g., `trogon-mcp-gateway` in monorepo implies feature; extracting crate implies standalone).

**Rejection.** Implicit decisions **ratchet hardest to reverse**:

- Public README and conference talks cite whatever shipped first.
- Customers contract against observable behavior (default-deny, required STS) before legal reviews positioning.
- ADR 0007--0016 read as Accepted technical law while Block A item 1 stays unchecked -- future standalone repositioning contradicts "accepted" architecture without a migration program.
- OSS contributors invest in `docs/identity/` paths that get renamed later.
- Competitive responses (Protect comparison, agentgateway comparison) ossify incorrectly.

Architecture records exist precisely to make strategic forks **explicit and reversible at the paper stage**. This ADR stays **Proposed** until leadership updates it; code momentum does not substitute.

### Hybrid commercial model without primary positioning (standalone SKU + platform bundle, equal weight)

**Proposal.** Sell standalone gateway **and** include it in TrogonStack **without** declaring which is primary; engineering maintains two equal reference architectures.

**Rejection (as equal-weight hybrid).** Differs from rejected dual-mode in degree but fails similarly: pricing, support, and roadmap still need a **primary** for planning. **Acceptable resolution** if leadership later chooses: **primary Option A** with **certified standalone attach** (or inverse) documented in Accepted ADR amendment -- not perpetual 50/50. Until then, hybrid commercial remains an **open question**, not a decision.

### Delay positioning until Phase 3 WASM ships ("let technology prove the market")

**Proposal.** Defer Block A item 1 until WASM bundles and multi-protocol engine exist; positioning follows technical maturity.

**Rejection.** Phase 1--2 already shipped queue-group ingress, SpiceDB gating, and multiple Accepted ADRs. Bundle format (ADR 0010), operator overview, and registry integration **embed Option A narrative** today. Delaying positioning does not prevent ratchet -- it **accelerates** implicit Option A while standalone prospects ask for contracts now. Technology maturity informs **evidence**, not **substitute for** a product decision.

---

## Open questions

Questions below are **inputs leadership needs** to move this ADR from Proposed to Accepted. Engineering can gather data; deciders assign owners and dates.

### Market and revenue

1. What is the **12-month revenue hypothesis** for TrogonStack platform deals vs security-only gateway SKUs?
2. Which **pipeline opportunities** explicitly require MCP governance without decider/event modeling?
3. Do we have **lost deals** where positioning ambiguity (platform vs security tool) was cited?
4. What **ACV and sales cycle** difference do we expect between Option A attach and Option B land?

### Partnerships and ecosystem

5. Are there **signed or draft partnerships** (NATS vendors, IDE vendors, SIEM, IdP) that assume standalone gateway branding?
6. Does **Synadia** relationship require complementary Protect positioning (favors Option A clarity) or competitive gateway positioning (favors Option B)?
7. Must we **certify third-party MCP servers** (GitHub, Notion, etc.) on day one for either option -- and does that drive hybrid HTTP ingress timing ([ADR 0007](0007-on-bus-vs-hybrid.md))?

### Customer evidence

8. How many **design partners** run full Trogon identity enforce mode vs bootstrap-only Phase 1?
9. What **support tickets** would split differently under Option A vs Option B ownership?
10. Do customers ask for **OCI bundle registry** they control (standalone signal) or **GitOps into platform KV** (feature signal)?

### Technical and operational readiness

11. What is **incremental cost** to deliver Option B minimal install (gateway + NATS + external IdP + SpiceDB) with documented gaps vs reference TrogonStack stack?
12. Does **policy-core extraction** before MCP differentiation complete violate plan Take ("do not build Protect by itself") -- and is that acceptable for Option B?
13. Can **audit envelope** and bundle signing remain stable across both options without forked schemas?

### Legal, OSS, and brand

14. What **license** must apply to gateway and bundles for standalone adoption vs platform source-available strategy?
15. Is **trademark** "Trogon" required on public gateway name for Option B?
16. Are there **existing contracts** naming TrogonStack that preclude standalone SKU?

### Organizational

17. Which **team owns** P&L for gateway engineering under each option?
18. Does **field marketing** have a committed launch date that assumes one positioning?

### Decision process

19. Who is the **single DRI** for Block A item 1 resolution?
20. What **date** gates Phase 2 commercial docs and Block G controller prioritization?

---

## Resolution criteria

This ADR moves from **Proposed** to **Accepted** when **all** of the following are satisfied:

| Criterion | Evidence required |
|---|---|
| **Explicit verdict** | Leadership records **Option A** or **Option B** (or **primary + secondary attach** with primary named) in the Decision section; status line updated to `Accepted (YYYY-MM-DD)`. |
| **Deciders named** | Metadata table `Deciders` field lists accountable roles (e.g., CEO/CPO/VP Eng), not placeholders. |
| **Commercial articulation** | One-page positioning statement: buyer persona, primary competitor frame, what is **not** sold (e.g., "not raw NATS Protect replacement"). |
| **Support boundary published** | Internal runbook section defines incident ownership for gateway, STS, registry, SpiceDB, NATS under chosen option. |
| **Documentation home chosen** | Issue or plan entry to either keep `docs/identity/` as sole home (Option A) or create `docs/mcp-gateway/` migration (Option B) with timeline. |
| **Bundle distribution default** | Written default for new customers: OCI-primary vs KV-primary, consistent with ADR 0010 implementation priority. |
| **Block A item 1 checkbox** | `MCP_GATEWAY_PLAN.md` Block A item 1 marked complete with link to this ADR version. |
| **Downstream ADR triggers** | Any ADR that assumed positioning gets amendment or new ADR if verdict contradicts prior implicit bias (tracked in plan Block H). |

**Rejected resolution paths:**

- "Decided by engineering consensus" without named leadership decider.
- "Decided by shipping Phase 3" without Accepted metadata update.
- Perpetual Proposed past committed launch date without written deferral and risk acceptance.

**After Accepted:**

- Amend Decision section only via new ADR or explicit versioned amendment with migration notes if positioning **changes** (e.g., A to B pivot) -- such pivots require customer communication plan and compatibility window, not silent default flips.

---

## Status of supporting work

| Item | Status | Notes |
|---|---|---|
| Block A item 1 (positioning) | **Resolved** | This ADR (Accepted, Option A) |
| Plan Take (feature recommendation) | **Now decision-backed** | Take aligns with Accepted Option A |
| Operator overview | **Shipped** | Trogon co-deployed stack -- now the canonical framing |
| Identity overview | **Shipped** | Mesh + STS + gateway chain |
| Technical ADRs 0001--0016, 0018--0032 | **Accepted** | Consistent with Option A; no amendments triggered |
| Phase 1 vertical slice | **Complete** | Substrate valid under Option A |
| Commercial pricing / SLA | **Not started** | Out of scope here; needs separate GTM workstream under Option A framing |
| `docs/mcp-gateway/` tree | **Not created** | Remains uncreated under Option A |

---

## Appendix: decision comparison matrix

| Criterion | Option A: TrogonStack feature | Option B: Standalone security product |
|---|---|---|
| Primary buyer | Platform team building agentic workflows | Security / platform eng securing MCP on NATS |
| Minimum deploy | Reference TrogonStack stack | Gateway + NATS + integrator-chosen IdP/PDP |
| Identity | Mesh + registry enforce default | Integration guides; enforce optional |
| Docs canonical path | `docs/identity/` | `docs/mcp-gateway/` (proposed) |
| Bundle distribution default | KV watch + platform `mcp-pack` | OCI registry + customer signing keys |
| HTTP MCP ingress priority | Phase 3+ optional ([ADR 0007](0007-on-bus-vs-hybrid.md)) | Likely Phase 2--3 accelerator |
| Policy engine extraction | Deferred until second protocol | Accelerated |
| xDS / K8s controller | v2 default | v1 candidate |
| vs Protect | Complementary | Overlap narrative required |
| vs agentgateway | MCP-on-NATS differentiation inside platform | Direct comparison on governance |
| Plan Take alignment | Strong | Inverts sequencing warnings |

---

*This ADR makes Block A item 1 explicit and deferrable at the paper stage. Technical implementation continues under existing identity specs and ADRs until leadership moves this record to Accepted with Option A or Option B named.*
