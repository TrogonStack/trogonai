# MCP gateway latency baseline (`latency_baseline`)

End-to-end added-latency benchmark comparing client requests through `trogon-mcp-gateway` against the same payload on direct `mcp-nats` server subjects. Methodology and budget placeholders are defined in [ADR 0031](../../../docs/adr/0031-latency-budget-benchmarking.md).

## Prerequisites

- Rust toolchain from `rust-toolchain.toml` at the repo root.
- `nats-server` on `PATH` **unless** you point at an existing broker via `NATS_URL`.
- JetStream enabled on the broker (the harness starts `nats-server` with a local JetStream store when it spawns the broker itself).

## Run

From `rsworkspace/`:

```bash
# Help / CLI options
cargo bench -p trogon-mcp-gateway --bench latency_baseline -- --help

# Quick smoke (writes JSON under benches/results/)
cargo bench -p trogon-mcp-gateway --bench latency_baseline -- --iterations 10 --warmup 0

# Default profile (200 warmup + 1000 recorded iterations per method/mode)
cargo bench -p trogon-mcp-gateway --bench latency_baseline
```

Optional flags:

| Flag | Default | Meaning |
|------|---------|---------|
| `--iterations` | `1000` | Recorded samples per method and measurement mode |
| `--warmup` | `200` | Discarded paired samples before recording |
| `--nats-url` / `NATS_URL` | spawn local | Broker URL; when unset the harness starts in-process `nats-server` |

## Measurement modes

| Mode | Path | Policy |
|------|------|--------|
| `direct` | Client → `{prefix}.server.{server_id}.*` echo stub | Baseline transport (no gateway) |
| `gateway-passthrough` | Client → gateway ingress → echo stub | JWT off, `AllowAllPermissionChecker`, no hierarchical bundle, audit stream init off |
| `gateway-full` | Same as passthrough through gateway | Hierarchical enforce (`MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE=1`), org allow CEL, schema cache + 4-field redaction registry, audit stream init on |

Methods exercised: `initialize`, `tools/list`, `tools/call` (single tool), `resources/read`.

## Interpreting results

**Primary metric (ADR 0031):** added latency `T_added = T_gateway - T_direct` in milliseconds, reported as **p50** and **p99** per method and gateway mode.

- **Wall times** (`direct_ms`, `gateway_passthrough_ms`, `gateway_full_ms`) are end-to-end client RTT on localhost NATS with a sub-millisecond echo stub.
- **Added latency** isolates gateway policy work; NATS RTT and JSON parse cost appear in both arms and largely cancel in the delta when the stub is identical.
- **Phase breakdown** (`phases_ms`) records span durations captured by the harness tracing subscriber. Today the gateway emits `mcp_gateway.handle_ingress`, mapped to the `ingress` phase; finer-grained `authz`, `cel`, `redaction`, and `egress` spans are **TODO** until the gateway crate instruments those steps (bench cannot modify `src/`).

Artifacts:

- JSON printed to stdout after each run (`schema: trogon.mcp.benchmark.gateway/v1`).
- Copy written to `benches/results/<utc-timestamp>.json`.

Warm samples only: discard `--warmup` iterations; no cold-start tagging yet.

## Budget thresholds (proposed — ADR 0031)

Ceilings apply to **warm added latency** on **localhost NATS** with a &lt;1 ms echo stub. Values are **(proposed)** until a reference runner baseline is committed.

| Workload (ADR id) | JSON-RPC | Warm p50 added | Warm p99 added |
|-------------------|----------|----------------|----------------|
| shadow | `tools/call` (passthrough) | ≤ 2 ms | ≤ 8 ms |
| permit | `tools/call` (CEL allow) | ≤ 3 ms | ≤ 10 ms |
| lookup | `tools/call` (SpiceDB) | ≤ 4 ms | ≤ 12 ms |
| redact | `tools/call` (full policy) | ≤ 5 ms | ≤ 15 ms |
| list-shape | `tools/list` (catalog filter) | ≤ 6 ms | ≤ 20 ms |

Map harness modes to workloads:

- `gateway-passthrough` + `tools/call` → **shadow**
- `gateway-full` + `tools/call` → **redact** (closest available; SpiceDB not wired in this harness)
- `gateway-full` + `tools/list` → **list-shape** (partial — CEL catalog filter when hierarchical engine has rules)

PR regression gate (**proposed**, not wired in CI yet): p99 added latency **> 15%** vs `main` baseline for `shadow` and `permit` workloads.

Operator design target (not measured here): gateway hot-path overhead under ~10 ms ([operator overview §6.1](../../../docs/identity/mcp-gateway-operator-overview.md)).

## NATS environment

| Variable | When | Purpose |
|----------|------|---------|
| `NATS_URL` | Optional | Reuse external broker (`nats://127.0.0.1:4222` typical) |
| `MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE` | Set by harness for `gateway-full` only | Enables hierarchical CEL merge on the gateway hot path |

If `nats-server` is missing from `PATH` and `NATS_URL` is unset, the bench exits immediately with an error message.
