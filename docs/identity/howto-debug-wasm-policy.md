# How to debug a WASM policy component crash

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** bundle authors shipping Tier-3 WASM policy components on a co-deployed Trogon MCP gateway (Phase 3). You know how to build and publish a bundle; this guide is for when `evaluate` traps in production.

**Related:** [How to write a CEL-only policy bundle](howto-write-bundle.md) · [WASM bundle format](wasm-bundle-format.md) · [MCP policy WIT sketch](mcp-policy-wit-sketch.md) · [Host ABI reference](reference-host-abi.md) · [Failure mode matrix](failure-mode-matrix.md) · [MCP gateway operator overview](mcp-gateway-operator-overview.md)

| Placeholder | Example value |
|---|---|
| Tenant | `acme` |
| Bundle slug | `acme/risk-gate` |
| Component id | `risk-evaluator` |
| Manifest digest | `sha256:abc123…` |
| Target WIT | `trogon:mcp-policy@0.1.0` |
| NATS prefix | `mcp` |

Replace these with your deployment values before running commands.

---

## Goal

Identify why your WASM policy component trapped during a live `tools/call`, correlate the failure across client JSON-RPC, JetStream audit, and OpenTelemetry spans, reproduce it offline with `trogon-gateway-ctl bundle dry-run`, and either patch the bundle or roll back to the prior digest.

---

## When to use this

Use this guide when:

- Clients receive JSON-RPC **`-32101`** with symbol **`policy_fault`** and `error.data.tier == "wasm"` ([reference-error-codes.md](reference-error-codes.md), [mcp-policy-wit-sketch.md § Error mapping](mcp-policy-wit-sketch.md#error-mapping)).
- Audit subjects match **`mcp.audit.error.request.{method_root}`** (for example `mcp.audit.error.request.tools`) with proposed `decision_reason` in the `wasm_*` family (`wasm_trap`, `wasm_fuel_exhausted`, `wasm_memory_limit`, …) per [ADR 0025](../adr/0025-wasmtime-component-pooling.md) §6.
- Traces show span **`mcp.gateway.wasm.evaluate`** in **ERROR** with `wasm.error.kind` set ([ADR 0032](../adr/0032-tracing-wasm-boundary.md) §6).

When **not** to use this:

- **`-32100` `policy_deny`** — intentional guest `deny(reason)`; see [failure-mode-matrix.md row 1](failure-mode-matrix.md#failure-mode-matrix).
- **`-32109` `audience_mismatch`** — JWT `aud` drift, not a Wasmtime trap ([reference-error-codes.md §3](reference-error-codes.md#3-full-code-table-wire-format-pin-6)).
- **`-32108` `no_policy`** — no active bundle pointer ([failure-mode-matrix.md row 5](failure-mode-matrix.md#failure-mode-matrix)).
- Load-time rejection (bad signature, WIT link failure) — prior bundle still serves; see [wasm-bundle-format.md §9](wasm-bundle-format.md#92-manifest-missing-or-invalid) and [howto-write-bundle.md § Rollback](howto-write-bundle.md#rollback).

For NATS or MCP basics, read [overview.md](overview.md) first.

---

## Prerequisites

1. **Phase 3 gateway with WASM pooling enabled** on the Trogon mesh ([mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md)). Default: `MCP_WASM_POOL_ENABLED=1` **(proposed — ADR 0025)**.

2. **Active signed bundle** whose manifest lists your component under `components[]` with `target_wit = trogon:mcp-policy@0.1.0` ([ADR 0010](../adr/0010-bundle-format.md), [wasm-bundle-format.md §3](wasm-bundle-format.md#3-oci-artifact-layout)).

3. **Observability access:** JetStream audit consumer on `mcp.audit.>`, OTel backend with gateway service name, and (optional) admin API for live inspect.

4. **`trogon-gateway-ctl` on PATH** — canonical operator CLI per [ADR 0030](../adr/0030-gateway-ctl-cli-surface.md). Build when the crate lands:

   ```bash
   cargo build -p trogon-gateway-ctl --release
   export PATH="$PWD/rsworkspace/target/release:$PATH"
   ```

5. **Local bundle tree** matching the promoted digest (same `manifest.toml`, `components/*.wasm`, signatures).

6. **One failing request fixture** — JSON-RPC id, `trace_id`, and approximate timestamp from the client error.

---

## Steps

### 1. Confirm the symptom is a WASM trap (not auth or deny)

1. Capture the client JSON-RPC error. WASM engine faults use **Pin 6 code `-32101`**, not `-32109`:

   ```json
   {
     "jsonrpc": "2.0",
     "id": "req-8842",
     "error": {
       "code": -32101,
       "message": "policy_fault",
       "data": {
         "trace_id": "0af7651916cd43dd8448eb211c80319c",
         "tier": "wasm",
         "error": "wasm_fuel_exhausted",
         "policy_bundle_digest": "sha256:abc123…",
         "component_id": "risk-evaluator",
         "wit_version": "0.1.0"
       }
     }
   }
   ```

   **Expected:** `code == -32101`, `data.tier == "wasm"`, `data.trace_id` present ([reference-error-codes.md §2](reference-error-codes.md#2-trace_id-requirement)). Shape aligns with [ADR 0025 §6](../adr/0025-wasmtime-component-pooling.md) `error.data` for WASM paths.

2. If the code is `-32109`, stop — that is **`audience_mismatch`** ([jwt-claim-schema.md](jwt-claim-schema.md)); use [sts-exchange.md](sts-exchange.md) instead.

---

### 2. Pull the audit envelope for the same `trace_id`

1. Subscribe or query JetStream for the error audit family:

   ```bash
   export TRACE_ID=0af7651916cd43dd8448eb211c80319c
   export MCP_PREFIX=mcp

   nats sub "${MCP_PREFIX}.audit.error.request.>" --count 20 | \
     jq --arg tid "$TRACE_ID" 'select(.trace_id == $tid or .request_id != null)'
   ```

   **Expected:** subject `mcp.audit.error.request.tools` (or matching `method_root`), wire `outcome: "error"` ([reference-audit-envelope.md §3.1](reference-audit-envelope.md#31-gateway)). Proposed header fields when unified ([mcp-policy-wit-sketch.md § Error mapping](mcp-policy-wit-sketch.md#error-mapping)):

   | Field | Example |
   |---|---|
   | `decision_reason` | `wasm_trap` \| `wasm_fuel_exhausted` \| `wasm_memory_limit` \| `wasm_import_limit` |
   | `policy_bundle_digest` | `sha256:abc123…` |
   | `component_id` | `risk-evaluator` |

2. Record **`policy_bundle_digest`** and **`component_id`** — you need both for offline repro and rollback targeting.

---

### 3. Read the OTel span at the WASM boundary

1. In your trace UI, open trace `0af7651916cd43dd8448eb211c80319c` and expand **`mcp.gateway.authz` → `mcp.gateway.wasm.evaluate`** ([ADR 0032 §2](../adr/0032-tracing-wasm-boundary.md#2-span-hierarchy-and-ownership)).

2. On trap, the host sets **ERROR** and these attributes ([ADR 0032 §5–§6](../adr/0032-tracing-wasm-boundary.md#5-host-attached-span-attributes-automatic)):

   | Attribute | Purpose |
   |---|---|
   | `policy.tier` | `wasm` |
   | `wasm.component` | Manifest `components[].id` |
   | `wasm.wit_version` | e.g. `0.1.0` |
   | `wasm.bundle_generation` | Hot-swap generation **(proposed)** |
   | `wasm.error.kind` | `trap` \| `fuel_exhausted` \| `memory_limit` \| `import_limit` |
   | `wasm.stack_excerpt` | Wasmtime stack trace, max 512 bytes, redacted **(proposed)** |
   | `wasm.fuel_consumed` | Fuel meter delta **(proposed)** |

3. Optional live trace lookup when admin API is enabled **(proposed — ADR 0030)**:

   ```bash
   trogon-gateway-ctl trace request \
     --id req-8842 \
     --output json
   ```

   **Expected:** `DecisionTrace` includes ingress subject and tenant; WASM-specific fields land in OTel first.

---

### 4. Classify the root cause

Map `decision_reason` / `wasm.error.kind` / `error.data.error` to action:

| Symptom | Likely cause | Where to look |
|---|---|---|
| `wasm_trap` + linker / import message at **load** | WIT or host ABI version drift | Manifest `target_wit`, `host_abi`, gateway linker pin ([reference-host-abi.md](reference-host-abi.md), `tests/wasm_host_abi.rs` `abi_versioning`) |
| `wasm_trap` + `unreachable` / guest panic at **runtime** | Logic bug or bad `params-json` parse in guest | `wasm.stack_excerpt`, guest source |
| `wasm_fuel_exhausted` | Long loop or heavy JSON work | Fuel budget ([mcp-policy-wit-sketch.md § Resource limits](mcp-policy-wit-sketch.md#resource-limits)); epoch interruption ([ADR 0025 §5](../adr/0025-wasmtime-component-pooling.md#5-memory-fuel-and-epoch-interruption)) |
| `wasm_memory_limit` | OOM / linear memory cap (default **16 MiB**) | Guest allocations; `[wasm.limits]` in manifest **(proposed)** |
| `wasm_import_limit` | More than **64** host imports per `evaluate` | Host import loop |
| `wasm_pool_exhausted` → `-32105` | Pool saturation, not a guest trap | `MCP_WASM_POOL_MAX_INSTANCES`, acquire timeout ([ADR 0025 §1](../adr/0025-wasmtime-component-pooling.md#1-lifecycle-model)) |
| `wasm_input_oversize` | `params-json` / `attributes-json` > 1 MiB | Input fixture size |

**Open question:** exact spelling of unified audit `decision_reason` strings on wire today — gateway audit struct may omit the field until header unification ([reference-audit-envelope.md §2.3](reference-audit-envelope.md#23-well-known-decision_reason-values)); treat JSON-RPC `data.error` and OTel attributes as authoritative for debugging.

---

### 5. Reproduce offline with `bundle dry-run`

Reproduce without publishing to NATS ([ADR 0030](../adr/0030-gateway-ctl-cli-surface.md) `bundle dry-run`).

1. Save the failing traffic as JSON Lines (one object per line). Bindings match [howto-write-bundle.md §5.1](howto-write-bundle.md#51-create-a-request-fixture):

   ```bash
   cat > fixtures/trap-repro.jsonl <<'EOF'
   {"nats":{"subject":"mcp.gateway.request.github.tools.call","headers":{"traceparent":"00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"}},"jwt":{"sub":"user:alice@acme.com","tenant":"acme"},"jsonrpc":{"jsonrpc":"2.0","id":"req-8842","method":"tools/call","params":{"name":"create_issue","arguments":{"owner":"acme","repo":"platform","title":"Repro"}}}}
   EOF
   ```

2. Validate WIT compatibility before execution:

   ```bash
   trogon-gateway-ctl bundle validate ./my-bundle --wit-check --output json
   # (proposed — ADR 0030)
   ```

   **Expected:** `data.result == "VALID"`. Link failures surface here for ABI drift without hitting production.

3. Dry-run the fixture:

   ```bash
   trogon-gateway-ctl bundle dry-run \
     --bundle ./my-bundle \
     --input ./fixtures/trap-repro.jsonl \
     --line 1 \
     --spicedb-endpoint "${MCP_GATEWAY_SPICEDB_ENDPOINT:-localhost:50051}" \
     --output json
   # (proposed — ADR 0030)
   ```

   **Expected on trap:** envelope `ok: false` or per-line result with `decision: "fault"`, `tier: "wasm"`, and the same `error` string as production. Success path shows `decision: "allow"` or `"deny"` without `tier: "wasm"` fault.

4. Compare **`policy_bundle_digest`** from step 2 with `./my-bundle` manifest bytes — mismatch means you are debugging the wrong artifact generation.

---

### 6. Apply a fix

#### Quick mitigations (operator / author, same digest family)

| Knob | Default | When to raise / lower | Env var **(proposed — ADR 0025)** |
|---|---|---|---|
| Fuel per `evaluate` | `5_000_000` | Guest legitimately CPU-heavy | `MCP_WASM_FUEL_EVALUATE` |
| Linear memory cap | 16 MiB (256 pages) | OOM on large params | Manifest `[wasm.limits]` **(proposed)** or tenant KV overlay |
| Pool instances | 32 per `PoolKey` | `-32105` pool exhausted | `MCP_WASM_POOL_MAX_INSTANCES` |
| Epoch arm | 10 ms before deadline | Runaway loop near NATS deadline | `MCP_WASM_EPOCH_ARM_MS` |

Rebuild the component against the pinned WIT (`trogon:mcp-policy@0.1.0`) and declare every host import in manifest `capabilities.imports` ([mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md)).

#### Permanent fix

1. Patch guest logic or manifest capabilities.
2. Bump bundle semver, re-sign, publish new digest ([howto-write-bundle.md §6](howto-write-bundle.md#step-6--sign-and-publish)).
3. Promote pointer in `mcp-gateway-config/bundle/active` ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)).
4. Re-run step 5 dry-run on CI before promotion.

---

## Verify

After mitigation or new digest promotion:

- [ ] Client `tools/call` returns allow/deny without `-32101`.
- [ ] Audit on success path: `mcp.audit.allow.request.tools` (or deny if expected); no `wasm_*` `decision_reason` on errors.
- [ ] Trace: `mcp.gateway.wasm.evaluate` **OK**, `decision` attribute `allow` or `deny`, not `fault`.
- [ ] Metric **`wasm_pool_poison_total`** **(proposed)** stops increasing for your `component_id`.
- [ ] `trogon-gateway-ctl bundle dry-run` on the saved fixture matches live behaviour **(proposed)**.
- [ ] Gateway `/readyz` reports active digest matching promotion **(proposed — ADR 0026)**.

---

## Rollback / undo

If the trap persists after promotion, revert to the last known-good digest ([failure-mode-matrix.md row 3](failure-mode-matrix.md#failure-mode-matrix), [ADR 0026 § rollback](../adr/0026-bundle-hot-swap-rollback.md#rollback-plan)).

1. Read pointer history:

   ```bash
   nats kv history mcp-gateway-config bundle/active
   ```

2. Roll back via CLI **(proposed — ADR 0030)**:

   ```bash
   trogon-gateway-ctl bundle rollback \
     --gateway-admin-url "${MCP_GATEWAY_CTL_ADMIN_URL:-http://127.0.0.1:8081}" \
     --bearer-token "${MCP_GATEWAY_CTL_BEARER_TOKEN}" \
     --output json
   ```

   Interim manual rollback — repoint `bundle/active` JSON to `previous_digest` ([howto-write-bundle.md § Rollback](howto-write-bundle.md#rollback)).

3. **Expected:** in-flight evaluations finish on the failed digest; new requests pin the restored snapshot ([ADR 0026 §3](../adr/0026-bundle-hot-swap-rollback.md#3-in-flight-request-fencing-pinned-decision)). Old digest pools drain within `MCP_WASM_POOL_DRAIN_TIMEOUT_SECS` (default **120 s** — ADR 0025).

4. Emergency WASM disable without restart: `MCP_WASM_POOL_ENABLED=0` **(proposed)** — Tier-2 CEL + SpiceDB path continues; Tier-3 and CEL `wasm.call` skipped.

Never mutate a published artifact in place; roll forward with a fixed digest after root-cause repair.

---

## Troubleshooting

| # | Symptom | Root cause | Fix |
|---|---|---|---|
| 1 | Dry-run passes; production traps | SpiceDB / cache / time non-determinism in guest | Record determinism audit per [mcp-policy-wit-sketch.md § Determinism](mcp-policy-wit-sketch.md#determinism-and-replay); stub imports in dry-run **(proposed)** |
| 2 | Trap only after hot-swap | New digest incompatible or `init()` failure | Check `mcp.control.bundle.load_failed` **(proposed)**; rollback § above; verify `init` fuel (`500_000` default) |
| 3 | `-32101` with `undeclared_host_capability` | Manifest `capabilities.imports` omits a used host import | Add import; redeploy ([reference-host-abi.md §12](reference-host-abi.md#12-capability-gating-and-static-validation)) |
| 4 | Intermittent `wasm_fuel_exhausted` | Pooled instance not reset; pathological input size | Patch guest; tune fuel; check poison metric **(proposed)** ([ADR 0025 §6 poison policy](../adr/0025-wasmtime-component-pooling.md#6-failure-handling--json-rpc)) |
| 5 | Trap with empty `wasm.stack_excerpt` | `MCP_GATEWAY_WASM_STACK_TRACES=off` **(proposed — ADR 0032)** | Enable stack traces in non-prod; use dry-run + guest logging via `host.log` |

---

## Related

- [ADR 0025](../adr/0025-wasmtime-component-pooling.md) · [ADR 0030](../adr/0030-gateway-ctl-cli-surface.md) · [ADR 0032](../adr/0032-tracing-wasm-boundary.md) · [ADR 0010](../adr/0010-bundle-format.md) · [ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)
- [reference-error-codes.md](reference-error-codes.md) · [reference-audit-envelope.md](reference-audit-envelope.md) · [reference-host-abi.md](reference-host-abi.md)
- [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) · [failure-mode-matrix.md](failure-mode-matrix.md) · [howto-write-bundle.md](howto-write-bundle.md) · [otel-wiring.md](otel-wiring.md)
- `rsworkspace/crates/trogon-mcp-gateway/tests/wasm_host_abi.rs`

---

*Document type: Diátaxis **how-to**. For WIT and host import theory, read [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md). For pool mechanics, read [ADR 0025](../adr/0025-wasmtime-component-pooling.md).*
