# Actor chain (`act_chain`)

**Status:** DRAFT — for human review before wire-format pin in `MCP_GATEWAY_PLAN.md`.

**Related:** `PENDING_TODO.md` Block 2.2, Block 1.3 (claim schema); `MCP_GATEWAY_PLAN.md` § Wire-Format Pins (headers, audit, CEL).

---

## Purpose

The `act_chain` JWT claim records **who acted on whose behalf across N delegation hops**. It answers audit and policy questions that perimeter `sub` alone cannot:

- Who was the **originator** (human, batch job, or root agent)?
- Which **agents** participated, in order, and on which **workloads**?
- When was each hop **recorded**?

Every mesh token carries the full chain. Gateways, backends, and CEL policies verify it; audit envelopes persist it for lineage queries (“what did agent X do on behalf of user Y?”).

---

## Schema

### Claim location

| Surface | Name | Type |
|---|---|---|
| JWT claim | `act_chain` | JSON array |
| NATS header (egress) | `mcp-act-chain` | compact JSON string (same array) |
| Audit envelope | `caller.act_chain` | JSON array (see [Audit embedding](#audit-envelope-embedding)) |

### Entry shape

Each element is an object with exactly these fields:

| Field | Type | Required | Semantics |
|---|---|---|---|
| `sub` | string | yes | Principal at this hop (`user:alice`, `agent:oncall`, `job:nightly-sync`). Stable across token refreshes. |
| `agent_id` | string | no | Registry agent identifier when this hop is agent-originated. **Omitted** for human UI callers, documented non-agent sentinels, and batch schedulers without a registered agent. |
| `wkl` | string | yes | Workload identity (SPIFFE ID) that executed this hop, or a documented sentinel (e.g. `human`, `batch`) when attestation does not apply. |
| `iat` | number | yes | Unix timestamp (seconds) when **this hop was appended** to the chain. Set by the STS or bootstrap issuer at append time — not the outer JWT `iat` unless they coincide. |

No other fields are permitted in v1. Unknown fields cause rejection in `enforce` mode.

### Ordering

- **Index 0** — oldest hop: the **originator** (typically the end user or root job principal).
- **Last index** — **immediate caller** for the token bearing the chain: the identity that obtained this token and is about to invoke the next hop.
- Order is strictly chronological by append; entries are **never reordered, rewritten, or removed** after append.

### Relationship to outer JWT

The outer JWT’s `sub`, `agent_id`, and `wkl` describe the **current** actor (last hop). The last `act_chain` entry MUST be consistent with those claims:

```
act_chain[-1].sub      == jwt.sub
act_chain[-1].agent_id == jwt.agent_id   (both absent or both equal)
act_chain[-1].wkl      == jwt.wkl
```

Mismatch → reject (see [Failure modes](#failure-modes)).

### Example: four-hop delegation (`user → A → B → C → backend`)

Agent C holds a mesh token with `aud=backend-github` and calls the MCP gateway. The chain has **four entries** — one per delegation hop; the backend is the **target**, not a chain entry.

```json
{
  "sub": "agent:acme/oncall-responder",
  "agent_id": "acme/oncall-responder",
  "agent_version": "3.2.1",
  "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder",
  "aud": "backend-github",
  "iss": "https://sts.acme.local",
  "act_chain": [
    {
      "sub": "user:alice@acme.com",
      "wkl": "human",
      "iat": 1748341200
    },
    {
      "sub": "agent:acme/triage-router",
      "agent_id": "acme/triage-router",
      "wkl": "spiffe://acme.local/ns/prod/sa/triage-router",
      "iat": 1748341201
    },
    {
      "sub": "agent:acme/incident-analyst",
      "agent_id": "acme/incident-analyst",
      "wkl": "spiffe://acme.local/ns/prod/sa/incident-analyst",
      "iat": 1748341202
    },
    {
      "sub": "agent:acme/oncall-responder",
      "agent_id": "acme/oncall-responder",
      "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder",
      "iat": 1748341203
    }
  ]
}
```

**Note (gateway egress):** When the gateway mints a downstream token (Block 2.4), it appends **one more** entry for itself before egress to the backend. That path is out of scope for the four-hop acceptance test above but uses the same append-only rules.

### Example: batch / non-interactive originator

Non-interactive callers (cron jobs, scheduled workflows, replay tools) have no human at index 0. Per [ADR 0002](../adr/0002-identity-layers.md), the originator hop uses `wkl: "sentinel:batch"` with `auth_method: "service-account"` on the bootstrap JWT, and the `sub` follows the documented batch-principal convention `job:{job-id}` (or the runner's service-account `sub` when one exists). `agent_id` stays absent at index 0 — a scheduler is not a registered agent.

```json
{
  "act_chain": [
    {
      "sub": "job:nightly-reconcile-2026-05-29",
      "wkl": "sentinel:batch",
      "iat": 1748390400
    },
    {
      "sub": "agent:acme/reconcile-runner",
      "agent_id": "acme/reconcile-runner",
      "wkl": "spiffe://acme.local/ns/prod/sa/reconcile-runner",
      "iat": 1748390401
    }
  ]
}
```

The bootstrap JWT for this originator carries `auth_method: "service-account"` and is minted by a documented batch principal in the tenant's originator allow-list (cron service registry, workflow engine signer). STS treats the originator entry the same as a human-OIDC entry for chain-resolution purposes: no registry lookup at index 0, sentinel `wkl` must be in the documented allow-list, no `agent_id` field expected.

---

## Signing model

Two options were considered:

| Option | Mechanism | Pros | Cons |
|---|---|---|---|
| **(a) Per-entry signatures** | Each STS signs the entry it appends (`sig` field per object). Verifiers check a chain of signatures. | Detects tampering of historical entries if outer JWT is re-signed maliciously; supports future offline notarization. | +~200–400 B per hop; STS must sign N times; verifier must track STS key history per entry; complicates CEL/header projection. |
| **(b) Integrity by outer JWT only** | The STS signs the entire JWT; `act_chain` is an unsigned JSON array inside the signed payload. | Minimal size; one signature per exchange; matches RFC 8693 token-exchange shape; verifiers already trust STS. | Compromise of STS or signing key affects all hops in that token; no independent proof of individual entries outside the JWT. |

### Recommendation: **(b) outer-JWT integrity only**

**Justification:**

1. **Threat model alignment.** Every hop already requires a valid STS-issued (or bootstrap) JWT. Forging or truncating `act_chain` without invalidating the signature is impossible. The relevant threat is a **malicious or buggy STS** — per-entry signatures do not help if the attacker controls the STS that mints the outer token.
2. **Size and latency.** At depth 8, per-entry ES256 signatures add roughly 1.5–3 KB to the JWT — material for NATS headers, CEL evaluation, and Uber’s STS P99 &lt; 40 ms budget. Outer-only keeps the hot path one verify + one parse.
3. **Operational simplicity.** One signing operation per exchange; no JWKS lookup per entry; audit stores the same array the verifier already parsed.

Per-entry signatures remain a documented **future extension** (optional `sig` field ignored in v1) if cross-organization notarization or third-party chain witnesses become a requirement.

---

## Propagation rules

1. **Bootstrap (first hop).** The initial mesh or bootstrap token contains a single-entry `act_chain` describing the originator. Humans via OIDC: `{ sub, wkl: "human", iat }` without `agent_id`. Agents: include `agent_id` and attested `wkl`.
2. **STS exchange (subsequent hops).** On every successful token exchange (`PENDING_TODO.md` Block 2.1), the STS:
   - Copies the inbound `act_chain` verbatim.
   - Appends one new entry for the **exchanger** (inbound token’s current actor: `sub`, `agent_id`, `wkl`, fresh `iat`).
   - Refuses append if `len(act_chain) >= max_depth` (default 8).
3. **Gateway egress.** When the gateway mints a downstream token (Block 2.4), it follows the same append-only rule for the gateway hop.
4. **Preservation.** Downstream tokens MUST carry the **full** chain. Middle agents MUST NOT strip, summarize, or replace entries.
5. **Audit.** Every gateway decision and STS exchange serializes the **full** `act_chain` into the audit envelope (success and failure).
6. **Client prohibition.** Clients MUST NOT supply `act_chain` via header or JWT on ingress; the gateway strips/forges (see [Header projection](#header-projection)).

---

## Verification rules

Verifiers: MCP gateway (ingress + egress), backend MCP servers (optional), A2A SDK `serve()` handler.

### 1. Structural validation

- `act_chain` MUST be present when `MCP_GATEWAY_AGENT_IDENTITY=enforce` (may be absent in `off`; logged in `shadow`).
- MUST be a JSON array with `1 <= length <= max_depth`.
- Each element MUST be an object with required fields and valid types.
- `iat` values MUST be monotonically non-decreasing by index (equal allowed within clock skew).
- Last entry MUST match outer JWT `sub` / `agent_id` / `wkl`.

### 2. Registry resolution — **at receipt**

Each entry with an `agent_id` MUST resolve to a **currently active** agent record in the registry at verification time. Each entry’s `sub` MUST be an allowed principal for that agent (or documented sentinel principal for non-agent originators).

**Rationale for at-receipt (not at-`iat`):**

- Revoked agents must not continue to delegate even if their hop was recorded before revocation.
- Avoids requiring a versioned registry history store in v1.
- Matches fail-closed security posture: “is this chain acceptable **now**?”

**Exception (shadow / audit only):** Audit consumers MAY annotate entries with `agent_status_at_verify: revoked` for forensics without changing the verification outcome in enforce mode.

For entries **without** `agent_id` (human/batch originator), verify `sub` against the tenant’s documented originator allow-list (OIDC issuer mapping, batch job registry) — exact mechanism is Block 0 / registry ADR.

### 3. Workload binding

For agent entries, `wkl` MUST appear in the agent record’s `allowed_workloads` at receipt. For the originator hop, `wkl` sentinel values (`human`, `batch`, …) MUST be in the documented allow-list.

### 4. Maximum depth

| Setting | Default | Scope |
|---|---|---|
| `act_chain.max_depth` | **8** | Bundle / gateway config |

Depth counts **entries**, not HTTP/NATS hops. A chain of length 8 is the hard reject threshold; length 9 is never minted.

**Why 8:** Covers typical `user → orchestrator → specialist → tool → backend` paths with headroom; bounds JWT size and loop search; aligns with `PENDING_TODO.md`. Tenants may lower via bundle; raising requires ADR (blast-radius tradeoff).

### 5. Loop detection

Reject if the same **`(agent_id, wkl)` pair** appears more than once among entries **where `agent_id` is present**.

```
∀ i < j : (agent_id[i], wkl[i]) ≠ (agent_id[j], wkl[j])   when agent_id[i] and agent_id[j] are both set
```

Human-only hops (`agent_id` absent) are excluded from loop detection — the same user may appear only once as originator at index 0 by construction; re-entry as a later hop is a separate open question.

Loop detection runs at **every** verifier (gateway ingress, STS pre-mint, backend optional).

### 6. Rollout modes

| Mode | Behavior |
|---|---|
| `off` | Ignore `act_chain` for authz; omit from audit or log as opaque. |
| `shadow` | Run full validation; log violations; do **not** reject. |
| `enforce` | Violations → JSON-RPC error (see below). |

Mirrors `MCP_GATEWAY_AGENT_IDENTITY` phasing (`PENDING_TODO.md` Block 6).

---

## CEL surface

Pinned additions to the `jwt.*` namespace (`MCP_GATEWAY_PLAN.md` §8):

| Variable / function | Type | Semantics |
|---|---|---|
| `jwt.act_chain` | `list<map<string, dynamic>>` | Parsed chain; empty list if absent and mode allows. |
| `chain.depth()` | `int` | `size(jwt.act_chain)` |
| `chain.originator()` | `map<string, dynamic>` | First entry; error if chain empty. |
| `chain.contains(agent_id)` | `bool` | True if any entry’s `agent_id` equals the argument. |

**Examples:**

```cel
// Deny if chain is deeper than policy allows for this tool
jwt.act_chain.size() > 5 && mcp.tool.name == "delete_repository"

// Require on-call agent in lineage for sensitive tools
chain.contains("acme/oncall-responder") || deny("missing on-call agent in act_chain")

// Rate-limit by originator, not immediate caller
rate.acquire("tenant", chain.originator().sub, 100, duration("1h"))
```

Host implementations MUST expose helpers under a `chain.*` import bound to `jwt.act_chain` for the current message.

---

## Header projection

Backends that do not parse JWTs receive the chain on egress headers. Same rules as `mcp-caller-sub`: **gateway-set, ingress-stripped**.

### Row for `MCP_GATEWAY_PLAN.md` header table (after `mcp-instance-id`)

| Header | Direction | Type | Source | Purpose |
|---|---|---|---|---|
| `mcp-act-chain` | gateway → backend | compact JSON string | gateway, from JWT `act_chain` | Full delegation lineage for backend logging and optional local policy. **Not authoritative** — backend MUST NOT trust over the validated egress JWT when both are present; header is a convenience projection. Stripped/overwritten on ingress. |

**Compact JSON:** no insignificant whitespace; keys in order `sub`, `agent_id`, `wkl`, `iat`; omit `agent_id` when absent.

**Ingress hardening (addition to §1 rule):** Drop client-supplied `mcp-act-chain`, `agent_id`, `wkl`, and JWT `act_chain` claim on edge ingress; only STS/gateway-minted values propagate.

---

## Audit envelope embedding

Per `MCP_GATEWAY_PLAN.md` §7 (audit envelope schema), embed the chain under `caller`:

```json
{
  "schema": "trogon.mcp.audit/v1",
  "ts": "2026-05-27T12:00:00Z",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "caller": {
    "sub": "agent:acme/oncall-responder",
    "via": "mesh:sts",
    "roles": ["agent"],
    "agent_id": "acme/oncall-responder",
    "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder",
    "act_chain": [
      { "sub": "user:alice@acme.com", "wkl": "human", "iat": 1748341200 },
      { "sub": "agent:acme/triage-router", "agent_id": "acme/triage-router", "wkl": "spiffe://acme.local/ns/prod/sa/triage-router", "iat": 1748341201 },
      { "sub": "agent:acme/incident-analyst", "agent_id": "acme/incident-analyst", "wkl": "spiffe://acme.local/ns/prod/sa/incident-analyst", "iat": 1748341202 },
      { "sub": "agent:acme/oncall-responder", "agent_id": "acme/oncall-responder", "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder", "iat": 1748341203 }
    ],
    "originator_sub": "user:alice@acme.com",
    "chain_depth": 4
  },
  "decision": "allow",
  "latency_us": 1820
}
```

- `caller.act_chain` — full array (same as JWT).
- `caller.originator_sub` — denormalized from `act_chain[0].sub` for indexing (`PENDING_TODO.md` Block 4).
- `caller.chain_depth` — denormalized `len(act_chain)` for dashboards.

STS exchange audit events (`mcp.audit.sts.*`) include the same `act_chain` on both request (inbound token) and response (minted token) sides.

---

## Failure modes

New JSON-RPC codes in Trogon allocation `-32100` … `-32199` (to be pinned in `MCP_GATEWAY_PLAN.md` §6):

| Code | Symbol | Condition | Outcome |
|---|---|---|---|
| `-32111` | `act_chain_missing` | `enforce` mode; claim absent or empty | Reject request; audit `decision: deny`, `error.code: act_chain_missing`. |
| `-32112` | `act_chain_malformed` | Invalid JSON shape, unknown fields, non-monotonic `iat`, last entry ≠ JWT claims | Reject; audit with `rule_fired: act_chain_structural`. |
| `-32113` | `act_chain_depth_exceeded` | `len(act_chain) > max_depth` | Reject; STS MUST NOT mint. Gateway rejects inbound. |
| `-32114` | `act_chain_loop_detected` | Duplicate `(agent_id, wkl)` | Reject; audit includes conflicting indices. |
| `-32115` | `act_chain_principal_unknown` | Registry lookup failure, revoked agent, `wkl ∉ allowed_workloads`, unknown originator sentinel | Reject; audit `error.detail: entry_index`. |

**`data` shape (stable):**

```json
{
  "trace_id": "…",
  "code": "act_chain_loop_detected",
  "entry_index": 3,
  "agent_id": "acme/oncall-responder",
  "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder"
}
```

In **`shadow`** mode, the same conditions emit audit/`extra` warnings with `would_deny: true` but return the underlying policy result.

---

## Open questions

The list below is the surviving v2-and-beyond set; the questions removed are resolved in `PENDING_TODO.md` "Decisions log" (depth, batch originator, purpose refinement, STS user exposure, OAuth-MCP) or tracked under `PENDING_TODO.md` "Open work" (gateway-as-registered-agent).

1. **Per-entry signatures.** Defer to v2 unless cross-org witnessing is required before GA.
2. **Human re-entry.** Can the same `user:…` appear at index &gt; 0 (e.g. human-in-the-loop approval mid-chain)?
3. **Historical registry at `iat`.** Needed for compliance replay ("was this agent allowed **then**?") — would require registry event stream; not v1.
4. **CEL on missing `agent_id`.** Should `chain.contains()` match on `sub` when `agent_id` is absent?

---

## Acceptance criteria (from Block 2.2)

- [ ] A four-hop call (`user → A → B → C → backend`) produces `act_chain` length **4** at the backend JWT and in audit.
- [ ] Any missing, malformed, over-depth, or looping chain is rejected in `enforce` mode with stable error codes.
- [ ] CEL rules can reference `jwt.act_chain`, `chain.depth()`, `chain.originator()`, `chain.contains()`.
- [ ] `mcp-act-chain` header is set on egress and stripped on ingress.
- [ ] Full chain appears in audit `caller.act_chain` on every gateway decision.
