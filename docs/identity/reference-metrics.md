# Metrics reference

v0.1 ships **traces + structured audit envelopes** as the primary
observability surface; metrics are auxiliary and intentionally
narrow until v0.2 wires Prometheus pull endpoints (ADR 0034 §3).

Operators get operational signal from:

1. **OTel traces** (`docs/identity/otel-wiring.md` and
   `docs/identity/reference-traces.md`).
2. **Audit envelopes** on `MCP_AUDIT` / `A2A_AUDIT` streams —
   queryable as JSON, optionally reshaped to Splunk HEC / Elastic ECS
   via `TROGON_SIEM_FORMAT`.
3. The metric counters and histograms below.

## Counters

| Name | Unit | Labels | Emitter |
|---|---|---|---|
| `acp.requests` | request | `method`, `session_id` | `acp-nats-agent`. |
| `acp.errors` | error | `method`, `kind` | `acp-nats-agent`. |

## Histograms

| Name | Unit | Labels | Emitter |
|---|---|---|---|
| `acp.request.duration` | seconds | `method` | `acp-nats-agent`. |

## Trace-derived metrics (recommended)

The richer signal lives in spans. With an OTel collector you can
turn the following spans into Prometheus metrics via the
`spanmetrics` connector:

- `mcp.gateway.request` (root ingress).
- `gateway.ingress.dispatch` (a2a-gateway ingress).
- `mcp.gateway.spicedb.check` (authz latency).
- `mcp.gateway.audit.publish` (audit emission).
- `mcp.gateway.egress` (upstream forward).
- `nats.request` / `nats.publish` (substrate calls).

See `docs/identity/reference-traces.md`.

## v0.2

ADR 0034 §3 commits to a dedicated Prom listener and a published
metric catalog with labels for `tenant`, `method`, `decision`,
`tier`, and `region`.
