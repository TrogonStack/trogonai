# How to enable shadow mode and migrate to enforce

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** platform operators migrating a Trogon MCP gateway deployment to default-deny enforce posture without a big-bang outage.

**Related:** [Bootstrap / day-zero](bootstrap-day-zero.md) · [Failure mode matrix](failure-mode-matrix.md) · [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [Audit envelope reference](reference-audit-envelope.md) · [How to write a bundle](howto-write-bundle.md) · [How to integrate third-party MCP](howto-integrate-third-party-mcp.md) · [Agent identity overview](overview.md)

| Placeholder | Example |
|---|---|
| Tenant | `acme` |
| Prefix | `mcp` (`MCP_PREFIX`) |
| Smoke `server_id` | `github` |

---

## Goal

Run the gateway in **policy shadow mode**: gated calls are fully evaluated (CEL, SpiceDB, merge) and audited, but **client responses stay unchanged**. After validating the shadow deny rate, cut over to **enforce** via one KV update and hot-reload. **Enforce + default-deny day-zero** is the resting state ([ADR 0021](../adr/0021-bootstrap-day-zero.md), [ADR 0017](../adr/0017-product-positioning.md)).

---

## When to use this

- Migrating from **no gateway** or Phase 1 dev bypass (`MCP_GATEWAY_SPICEDB_ENDPOINT` unset allow-all).
- **Pre-launch** or **post-bundle-change** validation against production-shaped traffic.

**Not for:** mesh-identity shadow only ([overview.md § Rollout modes](overview.md#rollout-modes), [ADR 0003](../adr/0003-bootstrap-vs-mesh-tokens.md)); **permanent** audit-only posture (shadow leaves data unprotected); tenants still in **day-zero** without a loaded bundle ([bootstrap-day-zero.md](bootstrap-day-zero.md)).

---

## Prerequisites

1. Co-deployed Trogon mesh: gateway on `{prefix}.gateway.request.>`, JetStream `MCP_AUDIT`, KV `mcp-gateway-config`, SpiceDB up ([mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md)).
2. Signed bundle at `mcp-gateway-config/bundle/active`; SpiceDB MCP schema + smoke tuples seeded ([howto-integrate-third-party-mcp.md §4](howto-integrate-third-party-mcp.md#step-4--publish-a-minimal-policy)).
3. Representative gated traffic (`tools/call`, `resources/read`).
4. `nats` CLI + audit consumer on `mcp.audit.>`.

---

## Steps

### 1. Record baseline posture

```bash
export TENANT=acme MCP_PREFIX=mcp

nats kv get mcp-gateway-config bundle/active
agctl mcp health --output json
curl -sS "http://127.0.0.1:8080/readyz" | jq '.bootstrap, .checks'  # (proposed — ADR 0021)
```

Finish [bootstrap-day-zero.md §3](bootstrap-day-zero.md#3-end-to-end-bootstrap-sequence) if `tenant_initialised` is false. Note `MCP_GATEWAY_AGENT_IDENTITY` — identity shadow is orthogonal ([failure-mode-matrix.md row 13](failure-mode-matrix.md#failure-mode-matrix)).

---

### 2. Enable policy shadow mode

Policy shadow is **operator opt-in**, not the product default ([ADR 0017](../adr/0017-product-positioning.md)). Request-path semantics mirror callback **`audit`** in [ADR 0020](../adr/0020-bidirectional-enforcement.md): evaluate, audit, do not block on policy deny.

| Knob | Values | Default | Status |
|---|---|---|---|
| KV `mcp-gateway-config/{tenant}/policy_enforcement` | `shadow` \| `enforce` | `enforce` | **(proposed)** — ADR 0020 pins `reverse_direction_enforcement` for callbacks |
| Env `MCP_GATEWAY_POLICY_ENFORCEMENT` | `shadow` \| `enforce` | unset → `enforce` | **(proposed)** — fleet bootstrap; KV wins |

```bash
cat > /tmp/policy-enforcement-shadow.json <<EOF
{
  "mode": "shadow",
  "tenant": "${TENANT}",
  "reason": "CHG-48291 pre-enforce soak",
  "updated_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "updated_by": "human:alice@acme.com"
}
EOF

nats kv put mcp-gateway-config "${TENANT}/policy_enforcement" @/tmp/policy-enforcement-shadow.json
# (proposed — ADR 0020)
```

Until KV watch lands, set `MCP_GATEWAY_POLICY_ENFORCEMENT=shadow` on the gateway Deployment and rolling-restart **(proposed — ADR 0021 config pattern)**.

**Shadow vs enforce on the request path (proposed contract):**

| Stage | Shadow | Enforce |
|---|---|---|
| CEL / SpiceDB / merge | Full evaluation | Full evaluation |
| Policy deny → client | Response **unchanged**; forward as if allowed | `-32100` / `-32108` ([reference-error-codes.md](reference-error-codes.md)) |
| Backend publish on policy deny | Still occurs | Blocked (FM invariant 3) |
| SpiceDB unreachable | Still **CLOSED** `-32107` | Same ([failure-mode-matrix.md row 1](failure-mode-matrix.md#failure-mode-matrix)) |

Expected: gateway logs `policy_enforcement=shadow` **(proposed)** within KV watch SLO (< 5 s P99).

---

### 3. Tail audit and read shadow fields

```bash
nats sub "mcp.audit.>" --count 50 | jq 'select(.extra.enforcement_mode == "shadow" or .extra.would_deny == true)'
```

| Field | Shadow | Enforce |
|---|---|---|
| `extra.enforcement_mode` | `"shadow"` | `"enforce"` |
| `extra.would_deny` | `true` when evaluation would deny | absent / `false` |
| `extra.shadow_decision` | `"allow"` \| `"deny"` | absent |
| `outcome` | Often `"allow"` when block suppressed **(proposed)** | `"deny"` on policy deny |
| `rules_fired`, `policy_merge.*` | When merge active **(proposed — [hierarchical-policy-merge.md §8](hierarchical-policy-merge.md#8-audit))** | Same |

Example **(proposed envelope)**:

```json
{
  "outcome": "allow",
  "jsonrpc_method": "tools/call",
  "tenant": "acme",
  "extra": {
    "enforcement_mode": "shadow",
    "would_deny": true,
    "shadow_decision": "deny",
    "rules_fired": ["tenant/acme-block-secrets"]
  }
}
```

Dedupe STS identity dual-emit (`wkl_unattested` deny alongside success — [reference-audit-envelope.md §5.4](reference-audit-envelope.md#54-shadow-attestation-dual-emit)) when counting policy denies.

---

### 4. Count shadow denies before cutover

Soak **≥ 7 days** at production volume ([overview.md § Rollout modes](overview.md#rollout-modes), [ADR 0003](../adr/0003-bootstrap-vs-mesh-tokens.md)).

```bash
nats sub "mcp.audit.>" --count 1000 | \
  jq -s '[.[] | select(.extra.would_deny == true)] | length'
```

| Signal | Action |
|---|---|
| `would_deny` ≈ 0 unexpectedly | Bundle/SpiceDB too permissive — fix before enforce |
| High `would_deny` on known-good agents | False positives — tune bundle/tuples ([howto-write-bundle.md](howto-write-bundle.md)) |
| Stable, explained rate | Proceed |
| Sustained `bootstrap_mode: true` | Finish bootstrap ([ADR 0021](../adr/0021-bootstrap-day-zero.md)) |

Negative probe — expect audit `would_deny: true`, client success in shadow; `-32100` in enforce:

```bash
nats request "mcp.gateway.request.github.tools.call" \
  '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"delete_repository","arguments":{}}}' \
  --header "Authorization: Bearer ${MESH_TOKEN}" --timeout 15s | jq .
```

---

### 5. Cut over to enforce (atomic KV + hot-reload)

```bash
nats kv history mcp-gateway-config "${TENANT}/policy_enforcement" | tail -3

cat > /tmp/policy-enforcement-enforce.json <<EOF
{
  "mode": "enforce",
  "tenant": "${TENANT}",
  "reason": "CHG-48291 shadow soak complete",
  "updated_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "updated_by": "human:alice@acme.com"
}
EOF

nats kv put mcp-gateway-config "${TENANT}/policy_enforcement" @/tmp/policy-enforcement-enforce.json

# (proposed — ADR 0026)
nats pub mcp.control.policy.enforcement.reload '{"tenant":"'"${TENANT}"'","mode":"enforce"}'
```

Hot-reload follows bundle pointer semantics ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md), [wasm-bundle-format.md §8.5](wasm-bundle-format.md#85-hot-swap-semantics)) — no pod restart when KV watch is active.

Flip `MCP_GATEWAY_AGENT_IDENTITY=enforce` in a **separate** change if needed ([overview.md](overview.md)).

Post-cutover smoke: allowed tool succeeds; denied tool returns `-32100`; audit `deny` without `would_deny`. Deny-path audit-before-reply when Pin 7 lands ([audit_delivery.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/audit_delivery.rs)).

---

## Verify

- [ ] KV `"mode":"enforce"` for tenant.
- [ ] Denied `tools/call` → `-32100`; backend never sees denied calls.
- [ ] Audit lacks `extra.would_deny` on new denies.
- [ ] No sustained `bootstrap_mode: true` ([ADR 0021](../adr/0021-bootstrap-day-zero.md)).
- [ ] Deleting `bundle/active` → default-deny day-zero, not shadow ([bootstrap-day-zero.md §7.3](bootstrap-day-zero.md#73-hard-reset-to-day-zero-compromise)).

---

## Rollback / undo

1. Repoint KV to shadow JSON (keep file from Step 2) or `MCP_GATEWAY_POLICY_ENFORCEMENT=shadow` **(proposed)** + rolling restart.
2. Confirm clients recover; audit regains `would_deny`. **Shadow = unprotected data** — time-box rollback.
3. Fix bundle (digest rollback [howto-write-bundle.md § Rollback](howto-write-bundle.md#rollback)) or SpiceDB; re-soak.
4. Stricter alternative: `bootstrap_mode=deny` ([ADR 0021](../adr/0021-bootstrap-day-zero.md)) — denies all MCP methods.
5. Compromise: delete `bundle/active` → day-zero default-deny ([failure-mode-matrix.md row 5](failure-mode-matrix.md#failure-mode-matrix)).

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| No `would_deny` in audit | Shadow not active; SpiceDB unset allow-all | Enable knob **(proposed)**; set `MCP_GATEWAY_SPICEDB_ENDPOINT` |
| `-32100` while shadow | Enforce still on; identity reject | Check KV key + fleet env |
| Cutover ignored | Watch lag; env override | Wait 30 s; remove env conflict |
| Deny storm after enforce | Missing tuples / tight rules | Rollback; fix bundle |
| Audit silent | Stream missing / backlog | [FM row 14](failure-mode-matrix.md#failure-mode-matrix); `nats stream info MCP_AUDIT` |

---

## Risks

- **Do not run shadow permanently** — gated traffic is unprotected ([ADR 0017](../adr/0017-product-positioning.md)).
- Low traffic → false confidence; require volume + negative probes.
- Identity + policy shadow together need separate rollback plans.

---

## Related

- **ADRs:** [0021](../adr/0021-bootstrap-day-zero.md) · [0024](../adr/0024-failure-mode-matrix.md) · [0017](../adr/0017-product-positioning.md) · [0020](../adr/0020-bidirectional-enforcement.md) · [0026](../adr/0026-bundle-hot-swap-rollback.md) · [0003](../adr/0003-bootstrap-vs-mesh-tokens.md)
- **Reference:** [failure-mode-matrix.md](failure-mode-matrix.md) · [reference-audit-envelope.md](reference-audit-envelope.md) · [hierarchical-policy-merge.md §8](hierarchical-policy-merge.md#8-audit)
- **How-tos:** [howto-write-bundle.md](howto-write-bundle.md) · [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md) · [bootstrap-day-zero.md](bootstrap-day-zero.md)

### Open questions **(proposed)**

1. Exact KV key shape — confirm with Block D bundle loader.
2. Env vs KV precedence — mirror [ADR 0021](../adr/0021-bootstrap-day-zero.md) `bootstrap_mode` rules.
3. `tools/list` shaping dry-run fields — [ADR 0015](../adr/0015-tools-list-filtering.md).

---

*Diátaxis **how-to**. Mesh identity shadow: [overview.md § Rollout modes](overview.md#rollout-modes). First bundle: [bootstrap-day-zero.md](bootstrap-day-zero.md).*
