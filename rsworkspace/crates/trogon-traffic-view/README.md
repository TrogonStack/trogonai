# trogon-traffic-view

**Spec + skeleton only.** v1 implementation is tracked in `PENDING_TODO.md` Block 4.

## Spec

Full schema, timeline/chain views, OCSF export, and consumer architecture:

**[`docs/identity/agent-traffic.md`](../../../docs/identity/agent-traffic.md)**

## Scope

This crate defines trait surfaces for:

| Module | Trait | Role |
|---|---|---|
| `projector` | `AuditConsumer` | JetStream durable `agent-traffic-projector` → normalize envelopes |
| `indexer` | `TrafficIndex` | Postgres (v1) `agent_traffic_events` read/write |
| `siem` | `SiemExporter` | OCSF v1 export |

`UnimplementedProjector`, `UnimplementedIndex`, and `UnimplementedOcsfExporter` are `todo!` stubs pointing at the spec sections.

## Getting started (next implementer)

1. **Projector** — Add `src/projector/` (or `src/bin/agent-traffic-projector.rs`) that:
   - Connects to NATS JetStream stream `MCP_AUDIT` with durable consumer `agent-traffic-projector`.
   - Parses gateway (`mcp.audit.{outcome}.{direction}.*`), STS (`mcp.audit.sts.*`), and registry (`mcp.audit.registry.*`) subjects per spec §2.
   - Maps JSON envelopes into `TrafficEvent` and calls `TrafficIndex::put_event`.

2. **Indexer** — Implement `TrafficIndex` for Postgres using the DDL in spec §3 (`sqlx` or your preferred client). Wire retention via `TRAFFIC_INDEX_RETENTION_DAYS`.

3. **Reader / CLI** — `agctl traffic` lives outside this crate initially; it should call `query_timeline` and `explore_chain` on the same `TrafficIndex` impl (or a thin HTTP wrapper).

4. **SIEM** — Implement `SiemExporter` for OCSF §7; optional sidecar process subscribing to the same stream or reading from the index.

```bash
cargo build -p trogon-traffic-view
cargo clippy -p trogon-traffic-view --all-targets -- -D warnings
cargo test -p trogon-traffic-view
```
