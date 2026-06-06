# Agent Gateway in Trogon — Decision

**Audience:** technical (engineers, architects)
**Status:** decision-support — proposed decision in §10, not yet ratified
**Source branch for the gateway work:** `yordis/agentgateway` (`TrogonStack/trogonai`)
**Evidence note:** capability claims below are drawn from crate names and commit
messages on the branch unless stated otherwise; items marked *(unverified)* were not
confirmed by reading the implementation. See §4 for a maturity caveat.

---

## 1. TL;DR

Trogon is intended to ship in two delivery models:

- a **CLI** that each user installs and runs on their own machine, and
- a **hosted service** (in development) that we (or a customer) operate, where agents run
  server-side on behalf of many users.

The **Agent Gateway** is the control layer that makes the **hosted** model secure,
multi-tenant, and auditable. It is what turns Trogon from "a tool you install" into
"a service an enterprise can buy."

The key conclusion of this document:

- **CLI (local) → the Agent Gateway's *core purpose* does not apply.** OS-level isolation
  already provides per-user separation and identity, which is the gateway's main job.
  Adopting the full gateway there is over-engineering — though a couple of leaf features
  (DLP, audit) could still add value; see §7.
- **Hosted service → the Agent Gateway is the missing half**, not an optional extra.
  Identity, tenant isolation, and audit stop being "nice to have" and become
  requirements the moment we run agents on behalf of third parties.

It is **not a separate product or a fork**: the gateway and the CLI share the same core
(runners, NATS substrate). The gateway is interposed only in the service deployment and
bypassed in the local CLI. One codebase, two delivery modes.

---

## 2. The distinction that decides everything

Whether the gateway is useful is **not** a function of how many users there are. It is a
function of **where the agent runs** and **whose trust boundary it sits in**.

### Local (single-tenant)

Trogon runs on the user's machine, as a CLI. The agent *is* the user: it uses their
credentials, their filesystem, their network. One install = one person.

- If the agent misbehaves, it only affects that user's own resources — resources they
  already had full access to anyway. No privilege escalation.
- **The operating system provides isolation for free**: user A's account cannot touch
  user B's, because they are on different machines.
- There is nobody to isolate, so the gateway's core purpose has nothing to protect.

### Hosted (multi-tenant)

Trogon runs on **our** infrastructure (servers / Kubernetes). Many different users come
in through a **web** front end and request work. Agents are orchestrated **server-side**,
not on the user's machine. This is the "services-as-software" model — the customer asks
for an outcome and a fleet of agents executes it on our backend; the user never touches
the runners. (Some vendors, e.g. 8090, are reported to operate this way, though we have
not verified their exact delivery model.)

- Agents for user A and user B run on the same infrastructure, sharing our network and
  our secrets. **The OS no longer isolates A from B** — they live in the same backend.
- You must **rebuild that isolation in software**. That software is the Agent Gateway:
  per-agent identity, policy at an enforced chokepoint, tenant scoping.

### The "N CLIs vs 1 backend" insight

A common objection: *"several users can just install the CLI and use it like the web
platform — so what's the difference?"*

At the **UX level** they are equivalent: a user asks, an agent does the work. The
difference is entirely in the backend trust model:

- **N local CLIs = N independent, isolated single-tenant systems.** The OS gives you the
  isolation. No gateway needed. The "multi-user" property is an illusion of aggregation;
  technically these are N isolated systems.
- **1 hosted backend serving N users = N tenants inside ONE trust domain we operate.**
  You must recreate, in software, the isolation the OS used to give you for free. The
  gateway is how you recreate it.

> Distributing the CLI is therefore a **legitimate architecture that avoids the gateway
> entirely** — each user is their own tenant for free. You only choose hosted (and pay
> for the gateway, see §6) when you have a reason *not* to distribute the CLI: long-running
> or heavy orchestration, customers who must not install anything, centralized
> state/memory, a buyer who demands one audited chokepoint, or agents that access central
> resources instead of the user's laptop.

---

## 3. Where the gateway sits — two frontiers

The gateway is not a single edge. It operates on **two** boundaries, and the more
important one is *not* the user-facing one.

```
              ① ingress                         ② egress / lateral
 [user/web] ───────────► [   TROGON BACKEND   ] ───────────► [ MCP tools, other A2A
            frontend-proxy   │  agents run here              agents, external APIs,
            (auth, tenant)   │                               shared resources ]
                             │
                             └─ mcp-gateway / a2a-gateway
                                (per-tool authz, redaction, audit, token-exchange)
```

**① User ↔ backend (ingress).** The `frontend-proxy` is a tenant-routed HTTP reverse
proxy; it routes the incoming request to the right tenant and (per the branch's OAuth
work) participates in authenticating it *(exact behavior inferred from crate name +
commits, not read in full)*. This is standard web auth — not the distinctive part.

**② Agent ↔ tools/resources (egress / lateral).** This is where the bulk of the gateway
lives (`mcp-gateway`, `a2a-gateway`). The agent is already *inside* our backend; the risk
is what it **calls outward**: which tool it executes, with what identity, what data it
exfiltrates (redaction/DLP), which token it uses (STS / token-exchange), and tenant
isolation (agent for A must never reach resources for B).

> Why this distinction matters: ingress auth is classic web security. What makes an
> *agent* gateway special is the egress boundary — an agent is not a passive user; it
> makes autonomous decisions and calls tools on its own. That is where a chokepoint is
> needed, which is why `mcp-gateway` / `a2a-gateway` / identity are the heart of the work,
> not the reverse proxy at the front.
>
> Note: the chokepoint is only as strong as its enforcement. It is "inviolable" only if
> egress is actually forced through it — i.e. network policy denies agents any direct
> route to tools. The branch ships a `networkpolicy.yaml` template for this, but the
> guarantee is conditional on that enforcement being deployed, not automatic.

---

## 4. What the Agent Gateway actually is

It is the **server-side control plane** for Trogon: the layer that exists for when you do
not trust the host running the agent and need a chokepoint with identity, policy, and
audit. Concretely, from the `yordis/agentgateway` branch:

**Identity / auth (core):**

- AAuth v0 (HTTP + NATS): person service, verifier, agent SDK.
- WIF / RFC 8693 token exchange, TTL-caching JWKS resolver, DPoP / `cnf.jwk` binding,
  JetStream-KV-backed ReplayStore.
- OAuth `authorization_code` + PKCE-S256 (for Cursor / Claude Code / Gemini CLI-style
  clients).
- STS minter and token-exchange runtime.

**Gateways & policy:**

- mTLS TLS 1.3 edge with a SPIFFE-aware verifier and hot reload.
- MCP gateway: per-tool allow-list, toxic tool-combination policy, YAML-driven redaction,
  SIEM-shaped audit envelopes, per-client tool rename, default-deny.
- A2A gateway: per-skill policy bundles, shadow mode, ingress audit; A2A v0.3 conformance.
- `frontend-proxy`: tenant-routed HTTP reverse proxy (ingress in front of MCP/A2A).
- Content-scan plugin (published contract + a default no-op scanner).

**Deployment:**

- Helm charts + CRDs (`EnterpriseAgentgatewayPolicy`, `AgentgatewayBackend`,
  `MCPGatewayConfig`) and Kubernetes controllers that project to NATS KV.
- NATS/NSC bootstrap scripts; A2A SDK in TypeScript (`tsworkspace`).

The design (captured in the branch's `GCP_TODO.md`) deliberately targets functional
parity with Google's Gemini Enterprise Agent Gateway and Solo.io's MCP token-exchange
spec, **without** taking GCP-native dependencies (Cloud IAP, VPC-SC, Model Armor by name,
`client_credentials` grants that WIF already covers).

> **Maturity caveat.** The feature list above is read from commit messages, not from a
> code audit. Wiring level varies: the branch's most recent commit explicitly notes that a
> prior commit "shipped types and unit-tested building blocks but left the runtime
> unwired," and then wires the token-exchange runtime and CRD controllers. Treat the list
> as *what the branch is building toward*, with end-to-end readiness to be confirmed
> per-component before relying on it.

---

## 5. Advantages — and exactly where each one applies

To make "where" concrete, here is the lifecycle of a single hosted request, with each
gateway touchpoint numbered. The component paths cited come from the branch's
`GCP_TODO.md` evidence column and commit messages, not from a personal code read.

### 5.1 The hosted request lifecycle

```
  [user/web]
      │  ① TLS 1.3 + tenant resolution
      ▼
  frontend-proxy ──────────────────────────────────── mTLS edge, SPIFFE verifier
      │  ② user token ──RFC 8693 exchange──► agent-scoped token (STS)
      ▼
  runner / agent session  (gets a SPIFFE SVID minted for THIS tenant+session)
      │  agent decides to call a tool
      ▼
  mcp-gateway / a2a-gateway   ◄── the egress chokepoint, where most controls live
      │  ③ verify agent identity (jkt/SVID)
      │  ④ PreAuthz: per-tool allow-list · toxic-combo · default-deny · tenant scope
      │  ⑤ PreCall: redact outbound args · mint mesh token for the call (STS)
      ▼
  [ MCP tool / external API / other A2A agent ]
      │  response
      ▼
  mcp-gateway
      │  ⑥ PostCall: redact tool output (DLP) before it returns to the agent/user
      │  ⑦ emit SIEM audit envelope  ──►  traffic-view
      ▼
  back to the agent → user
```

### 5.2 Each advantage, pinned to a touchpoint

**1. Per-agent identity (SPIFFE / WIF / STS token-exchange)**
- *What:* every agent session runs as a verifiable cryptographic identity (a SPIFFE
  SVID scoped to tenant+session), not a shared bearer token. The end user's token is
  swapped for an agent-scoped one via RFC 8693 exchange.
- *Where it applies:* touchpoints **②** (user token → agent token at the front,
  `trogon-sts/src/exchange.rs`) and **③** (the agent presents its identity on every
  egress call; the SPIFFE-aware verifier checks it). The SVID is minted at runner/session
  startup (`trogon-sts/src/workload_svid.rs`).
- *Why it matters here:* without it, "agent did X" is unattributable in a shared backend —
  you cannot tell whose agent acted, and you cannot scope what it may do.

**2. Centralized policy (per-tool allow-list, toxic tool-combination, default-deny)**
- *What:* one place decides what an agent may invoke, before it invokes it.
- *Where it applies:* touchpoint **④**, the `PreAuthz` stage inside the egress gateway
  (`trogon-mcp-gateway/src/authz.rs`, evaluated per `tools/call` and `tools/list`).
  Default-deny lives in `policy/hierarchical.rs`. This is the exact logical point that the
  local CLI handles with a `PreToolUse` hook — moved to a chokepoint the runner can't skip
  (conditional on network enforcement, §3).
- *Why it matters here:* in a hosted backend you cannot trust the runner to police itself;
  the decision has to sit outside it.

**3. Tenant isolation**
- *What:* customer A's agent can never reach customer B's tools, data, or region.
- *Where it applies:* both frontiers — tagged at **①** (frontend-proxy resolves tenant)
  and enforced at **④** (`tenancy_boundary.rs` scopes which servers/tools the tenant may
  see; default-deny on unknown server; the region router restricts reachable backends).
- *Why it matters here:* this is the literal software replacement for the OS boundary you
  lose the moment two tenants share one backend.

**4. Redaction / DLP**
- *What:* strip or mask sensitive fields so they don't leak through tool I/O.
- *Where it applies:* touchpoint **⑤** on outbound arguments and **⑥** on tool responses —
  the `PluginStage::{PreCall, PostCall}` seam (`trogon-mcp-gateway/src/plugin/mod.rs`),
  plus JSONPath field redaction in `a2a-redaction/`. Ships as a wire contract + default
  no-op scanner; pluggable for a real PII/ML scanner.
- *Why it matters here:* in SaaS, a tool response can carry another tenant's or a third
  party's data; the gateway is the choke where it gets scrubbed centrally.

**5. SIEM-shaped audit + traffic-view**
- *What:* a structured, queryable record of every decision and call.
- *Where it applies:* touchpoint **⑦** — every gateway decision emits an audit envelope to
  JetStream on `{prefix}.audit.{outcome}.{direction}.{method_root}`
  (`trogon-mcp-gateway/src/audit.rs`); `trogon-traffic-view` consumes it (top-N, chain
  explorer, Postgres-backed).
- *Why it matters here:* it is the concrete answer to "what did the agent do with my
  data?" — the question every enterprise buyer's security team asks.

**6. Token-exchange for egress**
- *What:* mint short-lived, audience-scoped mesh tokens per outbound call instead of
  handing agents long-lived static secrets.
- *Where it applies:* touchpoint **⑤**, at the moment of the outbound call
  (`trogon-mcp-gateway/src/egress/` mint + cache, backed by STS).
- *Why it matters here:* it shrinks the blast radius — a leaked token expires fast and is
  scoped to one audience, versus a stolen static API key.

**7. mTLS edge (TLS 1.3, SPIFFE-aware)**
- *What:* terminate TLS 1.3 at the edge and verify peer identity by SPIFFE, with hot
  reload of material.
- *Where it applies:* touchpoint **①** (ingress termination) and on the gateway↔backend
  mesh hops. The branch's own `GCP_TODO.md` calls this the single biggest security gap it
  closes.
- *Why it matters here:* it authenticates *connections*, the layer beneath token auth, so
  an attacker on the network can't impersonate a backend.

**8. Helm / CRDs / Kubernetes packaging**
- *What:* policy authored as Kubernetes CRDs (`EnterpriseAgentgatewayPolicy`,
  `AgentgatewayBackend`, `MCPGatewayConfig`); controllers project it into NATS KV that the
  gateway reads.
- *Where it applies:* **deploy/operate time, not request time.** This is what lets the same
  backend ship as SaaS (our cloud) *or* BYOC (the customer's cluster, §8).
- *Why it matters here:* it is the difference between "a service we run" and "a product a
  customer can install in their own VPC" — and the latter is often a hard enterprise
  requirement.

> Reading the table against §3: every runtime advantage (1–7) lands on the **egress
> frontier ②** except identity-at-the-front and mTLS, which also touch the **ingress
> frontier ①**. Advantage 8 is orthogonal — it is how the whole thing is deployed. This is
> the concrete version of "the bulk of the gateway lives at the egress boundary."

---

## 6. Costs and tradeoffs of adoption

The gateway is not free. An honest decision weighs these against §5:

- **Egress latency.** Routing every tool call through a network gateway (`mcp-gateway`)
  adds hops versus an in-process hook. Agents are tool-call-heavy, so this is a per-call
  tax on the hot path; it needs measuring, not assuming-away. The local CLI deliberately
  avoids it by using in-process hooks.
- **Operational surface.** Running the hosted control plane means operating NATS JetStream
  KV, the CRD controllers, the STS minter, a JWKS publisher, and mTLS with hot reload —
  plus their bootstrap (NSC) and upgrades. That is a standing ops cost and an on-call
  burden, whoever runs it (us in SaaS, the customer in BYOC).
- **Upstream-parity debt.** The design tracks two external specs (Google Gemini Agent
  Gateway, Solo.io MCP token-exchange). Keeping "parity" is continuous maintenance as
  those specs move.
- **Build/maturity cost.** Per §4, several components are still being wired end-to-end.
  Adopting now means co-developing, not just consuming a finished dependency.
- **Cognitive/security complexity.** SPIFFE trust domains, WIF mappings, CEL policies, and
  CRDs are powerful but raise the bar for whoever configures and debugs them. Misconfig in
  an auth/isolation layer is itself a security risk.

These costs are the reason the CLI path should **not** carry the gateway, and the reason
the hosted path should adopt it **incrementally** (see §10) rather than wholesale.

---

## 7. Scope boundary (what gives this credibility)

Being explicit about what is *not* needed is what makes the case credible.

**CLI local → does NOT need the gateway.** Everything it would add is either already
present or does not apply:

| Gateway piece | In the CLI |
|---|---|
| SPIFFE / WIF / STS identity | **N/A** — the agent uses the user's own credentials; OAuth PKCE for MCP already exists. |
| MCP/A2A gateway (centralized policy) | **Already covered** by `PreToolUse` / `PostToolUse` hooks, permission modes, the LLM classifier, and protected paths. In-process hooks are also better suited locally: being co-located with execution, they cannot be network-bypassed the way a remote gateway could (a moot point locally, since there is no remote gateway anyway). |
| Execution isolation | **Already covered** by `trogon-wasm-runtime` (per-session sandbox). *(Out of scope for this doc.)* |
| traffic-view / SIEM audit | **N/A** — service-operator observability, not single-developer concern. |
| Helm / CRDs / k8s | **N/A** — it is a CLI, not a deployment. |

**Hosted service → the gateway is the missing half.** Once we run agents on shared
infrastructure on behalf of third parties, identity, isolation, and audit are
requirements, not options.

**Optional middle ground:** even in a local context, two *leaf* pieces could add value
if ever wanted — the **content-scan / redaction (DLP on tool outputs)** seam and the
**audit envelopes** for traceability. (This is the reason §1 says the gateway's *core
purpose* — not literally everything it contains — fails to apply locally.) Everything else
(SPIFFE, A2A, k8s) should not be adopted without the multi-tenant case.

---

## 8. Deployment models for the hosted offering

When a customer "buys" the hosted product, the gateway supports three shapes — and "where
does it run?" is the central question that determines how much of the gateway you need:

1. **SaaS (vendor cloud, multi-tenant).** Users come in via web; agents run on our shared
   infrastructure. We operate the gateway, identity, and isolation. Purest hosted form.
2. **Dedicated / BYOC (single-tenant in the customer's cloud).** We deploy the stack into
   the customer's own VPC/Kubernetes for data residency or compliance. Still hosted —
   someone operates a control plane — but it runs on their infrastructure. **This is
   exactly why the Helm charts + CRDs + controllers exist**: they make the service
   deployable into the customer's cluster.
3. **Artifact delivery (factory extreme).** The factory runs on our side and we hand over
   the resulting software, which then runs wherever the customer wants. The factory
   itself is never shipped.

In all three, "where it executes" is what defines how much of the Agent Gateway is
required.

---

## 9. Integration feasibility

Bringing the gateway into the product is **low collision-risk in the crate sources**, but
the workspace-level integration is real, partly-unverified work:

- **Disjoint domains.** `platform`/`platform-pro` own the runtime/client side (CLI,
  runners, compactor, MCP client, subagents, hooks, permissions, sources, scheduler,
  vault). `yordis/agentgateway` owns the gateway side (mcp-gateway, a2a-*, aauth-*, sts,
  identity-types, agent-registry + controllers, frontend-proxy, traffic-view, gateway-*).
  None of the gateway's crates exist in `platform` today — they are net-new and additive.
- **Shared substrate, barely diverged.** Both branches share 16 base crates
  (`acp-nats*`, `mcp-nats*`, `trogon-decider*`, `trogon-nats`, `trogon-telemetry`,
  `trogon-std`, `trogon-gateway`, `trogon-service-config`). A sample of 6 of them differed
  by only 1–6 lines; the rest were not individually diffed but are expected to be similar.
  The integration seam exists at the NATS subject layer; it does not have to be invented.
- **The hook seam already lines up.** Trogon's `PreToolUse` / `PostToolUse` hooks are the
  same logical point as the gateway's `PluginStage::PreAuthz / PreCall / PostCall`. Same
  point, two implementations — local hook vs. centralized gateway. This is what makes the
  hybrid clean rather than two products.

**Not yet verified (do before committing to a timeline):**

- Whether the agentgateway workspace **compiles against `platform`'s pinned dependency
  versions** (both use exact `=x.y.z` pins and deny-all lints; version unification and
  `Cargo.lock` reconciliation can be non-trivial).
- Feature-flag unification across the merged workspace.
- End-to-end readiness of the components flagged in §4.

**Suggested phasing** (leaf-first to minimize merge surface):

1. Identity crates first (`aauth-*`, `sts`, `identity-types`) — they are leaves.
2. `mcp-gateway` next.
3. A2A (`a2a-gateway` + discovery + SDK) last.

The real effort is **wiring, not merging**: making the runners actually mint and present
identity, and making the local hooks delegate to the gateway on the service path.

---

## 10. Proposed decision & next steps

This document does not yet record a ratified decision. The proposed position:

> **Adopt the Agent Gateway for the hosted service path only, incrementally, starting with
> the identity crates — and keep the local CLI gateway-free.**

To turn this into a real decision, the following are open and need an owner:

- [ ] **Confirm product direction:** is the hosted service a committed deliverable, and in
  which shape (SaaS / BYOC / both)? — *owner: TBD*
- [ ] **Spike the workspace merge** of the identity crates onto `platform` and confirm it
  compiles under the pinned deps / deny lints. — *owner: TBD*
- [ ] **Measure egress latency** of `mcp-gateway` on a representative tool-call workload
  vs. the in-process hook baseline. — *owner: TBD*
- [ ] **Confirm end-to-end maturity** of the §4 components intended for phase 1. — *owner: TBD*
- [ ] **Name the operator** of the control plane (us vs. customer) per deployment model and
  size the ops cost. — *owner: TBD*

Once these are answered, ratify (or revise) the proposed decision above and update the
status line.

---

## 11. Bottom line

For the **local CLI**, ship the core plus hooks — no gateway. For the **hosted service**,
that same core plus the Agent Gateway at the two frontiers is what makes Trogon a
multi-tenant product you can sell — at the cost of egress latency and a standing
control-plane to operate. One codebase, two delivery modes, the gateway interposed only
where agents run on infrastructure you operate.
