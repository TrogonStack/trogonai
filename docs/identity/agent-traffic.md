# Agent-traffic view (v1 spec)

Companion: `PENDING_TODO.md` Block 4, `MCP_GATEWAY_PLAN.md` § Audit, `docs/identity/act-chain.md`.

Implementation scaffold: `rsworkspace/crates/trogon-traffic-view`.

---

## 1. Purpose

Security engineers and on-call operators use the **agent-traffic view** to answer, in one place: **what did agent X do on behalf of user Y in the last hour, and what was blocked?** The view indexes JetStream audit envelopes that already carry optional identity fields (`agent_id`, `agent_version`, `wkl`, `purpose`, `session_id`, `act_chain`). It does not replace the legal audit stream; it is a query-optimized projection for timelines, delegation trees, top-N dashboards, and SIEM export.

---

## 2. Inputs

The durable consumer `agent-traffic-projector` subscribes to stream `MCP_AUDIT` with filter `{prefix}.audit.>` (default prefix `mcp`). Events are normalized into `agent_traffic_events` rows. Logical sources:

### 2.1 Gateway tool traffic

**Subject pattern (wire today):** `{prefix}.audit.{outcome}.{direction}.{method_root}`  
Examples: `mcp.audit.allow.request.tools`, `mcp.audit.deny.request.tools`, `mcp.audit.rewrite.callback.tools`.

Task shorthand `mcp.audit.gateway.{request,response,deny}` maps to **direction** `request` / `response` / `callback` and **outcome** `allow` / `deny` / `rewrite` on the envelope, not a literal `gateway` subject segment.

| Envelope field | Use in index |
|---|---|
| `ts`, `trace_id`, `span_id` | Timeline ordering, chain explorer key |
| `tenant` | Partitioning, top-N by tenant |
| `caller_sub` | Human/service principal |
| `agent_id`, `agent_version`, `wkl` | Caller agent identity |
| `purpose`, `session_id` | Intent and session correlation |
| `act_chain` | `originator_sub`, `chain_root`, depth, tree edges |
| `subject_in`, `subject_out` | Hop routing (gateway → backend) |
| `jsonrpc_method`, `tool` (when present) | Tool name column |
| `outcome` / `decision` | `allow` / `deny` / `rewrite` (redact) |
| `latency_us` | Timeline latency |

Denormalized at project time (from `act_chain` when present):

- `originator_sub` = `act_chain[0].sub`
- `chain_root` = `act_chain[0].sub` (same as originator for indexing; alias for dashboard queries)
- `chain_depth` = `len(act_chain)`

### 2.2 STS token exchange

**Subjects:** `mcp.audit.sts.{outcome}` where `{outcome}` ∈ `success`, `deny`, `rate_limited`.

| Envelope field | Use in index |
|---|---|
| `trace_id`, `ts` | Correlation with gateway rows |
| `request.audience`, `request.purpose`, `request.subject_sub`, `request.wkl` | Exchange context |
| `minted.sub`, `minted.agent_id`, `minted.aud`, `minted.act_chain_depth` | Callee hop |
| `minted.act_chain` (when embedded) | Full chain on mint side |
| `outcome`, `latency_us` | Decision and timing |

### 2.3 Agent registry

**Subjects:** `mcp.audit.registry.*` (e.g. `lookup.found`, `lookup.notfound`, `put`, `delete`).

| Envelope field | Use in index |
|---|---|
| `agent_id`, `tenant` | Registry lookups affecting later STS/gateway denials |
| `outcome` | Audit-only rows; optional for “registry denied before call” narratives |

Registry events are lower volume; v1 indexes them with `source = registry` for completeness, not for the primary timeline (tool + STS hops dominate).

---

## 3. Index schema

Postgres v1 (documentation DDL; implementer may add migrations in a follow-up crate or service).

```sql
-- Retention: default 30 days; operator sets TRAFFIC_INDEX_RETENTION_DAYS (min 7, max 365).
-- Enforced via pg_partman or scheduled DELETE on ts; not automatic in skeleton.
--
-- v1 implementation (`trogon-traffic-view/migrations/001_agent_traffic_events.sql`) uses
-- `event_id TEXT PRIMARY KEY` (stable hash or wire `event_id`) for replay-safe upserts via
-- `ON CONFLICT DO NOTHING`. Extended dashboard columns below remain the long-term shape;
-- the v1 migration stores the query columns called out in the task brief.

CREATE TYPE traffic_source AS ENUM ('gateway', 'sts', 'registry');
CREATE TYPE traffic_decision AS ENUM ('allow', 'deny', 'rewrite', 'rate_limited', 'unknown');

CREATE TABLE agent_traffic_events (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    ts              TIMESTAMPTZ NOT NULL,
    tenant          TEXT NOT NULL,
    trace_id        TEXT NOT NULL,
    span_id         TEXT,
    source          traffic_source NOT NULL,
    decision        traffic_decision NOT NULL,
    originator_sub  TEXT,
    chain_root      TEXT,
    caller_sub      TEXT,
    agent_id        TEXT,
    callee_agent_id TEXT,
    callee_aud      TEXT,
    tool_name       TEXT,
    jsonrpc_method  TEXT,
    purpose         TEXT,
    session_id      TEXT,
    wkl             TEXT,
    agent_version   TEXT,
    chain_depth     INT,
    act_chain       JSONB,
    subject_in      TEXT,
    subject_out     TEXT,
    latency_us      BIGINT,
    raw_envelope    JSONB NOT NULL,
    ingested_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_traffic_originator_ts ON agent_traffic_events (tenant, originator_sub, ts DESC);
CREATE INDEX idx_traffic_agent_ts ON agent_traffic_events (tenant, agent_id, ts DESC);
CREATE INDEX idx_traffic_chain_root_ts ON agent_traffic_events (tenant, chain_root, ts DESC);
CREATE INDEX idx_traffic_session_ts ON agent_traffic_events (tenant, session_id, ts DESC);
CREATE INDEX idx_traffic_trace_ts ON agent_traffic_events (trace_id, ts ASC);
```

**Retention policy:** default **30 days** per tenant (configurable `TRAFFIC_INDEX_RETENTION_DAYS`). Older partitions dropped or archived to object storage; legal record remains on JetStream `MCP_AUDIT` per stream limits.

---

## 4. Timeline view

One row per indexed hop (gateway tool call or STS exchange), sorted by `ts` ascending within a filter window.

| Column | Source |
|---|---|
| Time | `ts` (RFC3339) |
| Caller agent | `agent_id` or `caller_sub` when no agent |
| Callee | `callee_agent_id` or parsed `callee_aud` / `subject_out` backend |
| Tool | `tool_name` or `jsonrpc_method` |
| Decision | `allow` / `deny` / `rewrite` (shown as **redact**) |
| Latency | `latency_us` → ms in UI |
| Purpose | `purpose` |

**Sample render** (`agctl traffic timeline --agent acme/oncall-agent --window 1h`):

```text
TIME (UTC)           CALLER              CALLEE                 TOOL           DECISION  LATENCY  PURPOSE
2026-05-27T14:02:01  acme/triage-router  acme/oncall-agent      (sts)          allow     12ms     incident.triage
2026-05-27T14:02:02  acme/oncall-agent   urn:…:backend:fs       db_query       allow     1.8s     incident.response
2026-05-27T14:02:04  acme/oncall-agent   urn:…:backend:fs       secrets_read   deny      4ms      incident.response
```

---

## 5. Chain explorer

Given `trace_id`, load all rows with that `trace_id`, order by `ts`, and build a **delegation tree**: each `act_chain` entry is a node; gateway/STS rows attach as leaves on the deepest matching hop.

**Tree node payload:**

```json
{
  "hop_index": 2,
  "sub": "agent:acme/oncall-agent",
  "agent_id": "acme/oncall-agent",
  "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
  "iat": 1748341203,
  "decision": "allow",
  "tool": "db_query",
  "latency_ms": 1.82,
  "children": []
}
```

**ASCII example** (`trace_id = 0af7651916cd43dd8448eb211c80319c`):

```text
[user:alice@acme.com]  (originator, wkl: human)
    └── [acme/triage-router]  STS success 14:02:00
            └── [acme/oncall-agent]  STS success 14:02:01
                    ├── tools/call db_query  ALLOW  14:02:02
                    └── tools/call secrets_read  DENY  14:02:04
```

---

## 6. Top-N dashboards

All queries scoped by `tenant` and time window `[$start, $end)`.

| Dashboard | SQL sketch |
|---|---|
| Most-active agents | `SELECT agent_id, COUNT(*) AS n FROM agent_traffic_events WHERE tenant = $1 AND ts >= $2 AND ts < $3 AND agent_id IS NOT NULL GROUP BY agent_id ORDER BY n DESC LIMIT $k` |
| Most-denied agents | Same with `AND decision = 'deny'` |
| Deepest chains | `SELECT agent_id, MAX(chain_depth) AS depth FROM … GROUP BY agent_id ORDER BY depth DESC LIMIT $k` |
| Longest chains by tenant | `SELECT tenant, trace_id, MAX(chain_depth) AS depth FROM … GROUP BY tenant, trace_id ORDER BY depth DESC LIMIT $k` |

---

## 7. SIEM export (pinned: OCSF v1)

**Format:** [Open Cybersecurity Schema Framework](https://schema.ocsf.io/) (OCSF) **1.x** as the v1 export encoding. CEF and raw JSON remain available as future adapters; the JetStream SIEM sidecar emits OCSF only in v1.

OCSF is vendor-neutral, versioned, and maps cleanly to mesh identity: **Authentication** (class 3002) for STS exchanges, **API Activity** (class 6003) for gateway tool calls. Splunk, Microsoft Sentinel, and Google Chronicle all ship OCSF ingest paths, so operators avoid a one-off Trogon JSON dialect.

**Sample OCSF event — STS exchange** (`mcp.audit.sts.success`):

```json
{
  "activity_id": 1,
  "activity_name": "Create",
  "category_uid": 3,
  "category_name": "Identity & Access Management",
  "class_uid": 3002,
  "class_name": "Authentication",
  "type_uid": 300201,
  "type_name": "Authentication: Create",
  "severity_id": 1,
  "severity": "Informational",
  "time": 1748341201000,
  "metadata": {
    "version": "1.3.0",
    "product": { "name": "trogon-mcp-gateway", "vendor_name": "TrogonStack" },
    "profiles": ["host"]
  },
  "actor": {
    "user": { "uid": "agent:acme/triage-router", "name": "acme/triage-router" }
  },
  "src_endpoint": { "name": "spiffe://acme.local/ns/prod/sa/triage-router" },
  "auth_protocol": "OAuth 2.0 Token Exchange",
  "service": { "name": "trogon-sts" },
  "status_id": 1,
  "status": "Success",
  "unmapped": {
    "trogon.trace_id": "0af7651916cd43dd8448eb211c80319c",
    "trogon.audience": "urn:trogon:a2a:agent:acme:oncall-agent",
    "trogon.purpose": "incident.response",
    "trogon.act_chain_depth": 3
  }
}
```

---

## 8. UI substrate (pinned: CLI-first `agctl traffic`)

**v1:** `agctl traffic` subcommands reading the Postgres index (or a thin gRPC/HTTP reader in front of it). **Web UI deferred** — a read-only dashboard can subscribe to the same reader API later without changing the index schema.

CLI is fastest to ship, scriptable for incident response (`agctl traffic timeline … | jq`), and matches operator habits from `kubectl` / `zed`. No browser dependency in air-gapped environments.

**Planned surface:**

```bash
agctl traffic timeline --agent acme/oncall-agent --window 1h [--tenant acme] [--originator user:alice]
agctl traffic chain --trace-id 0af7651916cd43dd8448eb211c80319c
agctl traffic top --metric active-agents|denied-agents|deepest-chains --window 24h --limit 20
agctl traffic export --format ocsf --since 1h --out ./siem-batch.ndjson
```

**Shipped in v1 (`agctl traffic`):**

```bash
# Postgres URL via --database-url or TROGON_PG_URL
agctl traffic query --since 1h --tenant acme --agent-id acme/agent1 [--limit 100] [--json]
agctl traffic tail --tenant acme [--since 5m] [--limit 50] [--json]
```

`query` filters on `tenant`, optional `agent_id` (matches `caller_sub`), and a relative `--since`
duration (`1h`, `30m`, `24h`). `tail` streams recent rows for a tenant in ascending time order.
Both commands default to a fixed-width table; pass `--json` for raw indexed rows.

---

## 9. Consumer architecture

```text
 JetStream MCP_AUDIT
        │
        ▼
 ┌──────────────────────┐
 │ agent-traffic-       │  durable: agent-traffic-projector
 │ projector            │  (queue worker, trogon-traffic-view)
 └──────────┬───────────┘
            │ normalized TrafficEvent
            ▼
 ┌──────────────────────┐
 │ indexer              │  trait TrafficIndex
 │ (Postgres v1)        │
 └──────────┬───────────┘
            │
     ┌──────┴──────┐
     ▼             ▼
 agctl traffic   SIEM exporter (OCSF)
 reader          trait SiemExporter
```

- **Stream → projector:** NATS JetStream pull/push consumer `agent-traffic-projector`, ack after successful `put_event`.
- **Projector → indexer:** `TrafficIndex::put_event` (batch optional v2).
- **Reader:** `TrafficIndex::query_timeline`, `explore_chain` backing `agctl traffic`.
- **Storage:** Postgres v1; **ClickHouse** considered for v2 if row volume exceeds ~1B events/month per cluster.

---

## 10. Open questions

Deliberately unspecified in v1 (resolve in implementation PRs):

| Topic | Notes |
|---|---|
| Retention beyond 30 days | Cold storage format, legal hold per tenant |
| Multi-tenant isolation | Row-level `tenant` vs separate DB/schema per tenant |
| Alerting hooks | Push to PagerDuty when deny rate spikes; not in skeleton |
| Hash-chained audit verification | Projector trusts JetStream; optional envelope digest replay |
| Partial `act_chain` in shadow mode | Index rows with `chain_depth` null vs inferred from single hop |
| Registry row volume | Include in timeline or separate “control plane” tab |
| Real-time vs batch SIEM | v1 batch `export`; streaming consumer optional |
| ClickHouse cutover | Partitioning and migration from Postgres |

---

## References

- Audit envelope optional fields: `trogon-mcp-gateway` `audit.rs`, `MCP_GATEWAY_PLAN.md` § Audit.
- `act_chain` semantics: `docs/identity/act-chain.md`.
- STS audit shape: `docs/identity/sts-exchange.md` § Audit emission.
