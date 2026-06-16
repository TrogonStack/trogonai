# Traces reference

Every gateway binary emits OTel spans on the configured OTLP/gRPC
endpoint (`TROGON_OTEL_ENDPOINT`). This page catalogs the spans that
ship in v0.1 and the fields each carries.

## MCP gateway

| Span | Fields | Notes |
|---|---|---|
| `mcp.gateway.request` | `subject_in`, `method`, `request_id`, `tenant` | Root ingress span. |
| `mcp.gateway.authz` | `decision`, `tier`, `policy_id` | Wraps Tier-1 + Tier-2 evaluation. |
| `mcp.gateway.spicedb.check` | `object_type`, `object_id`, `permission`, `caller`, `zedtoken_cached` | SpiceDB `CheckPermission`. |
| `mcp.gateway.egress` | `subject_out`, `upstream`, `duration_ms` | Forward to upstream MCP server. |
| `mcp.gateway.audit.publish` | `stream`, `audit_id`, `decision` | Audit envelope emission. |
| `mcp.gateway.plugin.call` | `plugin`, `subject` | External NATS callout plugin. |
| `mcp.gateway.wasm.evaluate` | `module`, `policy_id`, `duration_ms` | WASM policy evaluation (deferred — Tier-3 v0.2 path). |
| `nats.connect` | `server`, `cluster` | NATS connect. |
| `nats.request` | `subject` | Request/reply. |
| `nats.publish` | `subject` | Publish. |

## A2A gateway

| Span | Fields | Notes |
|---|---|---|
| `gateway.ingress.dispatch` | `method`, `caller_id`, `agent_id`, `request_id`, `decision` | Root ingress span for `message.send` and `message.stream`. |
| `a2a.server.agent_card` | `agent_id` | Resolved AgentCard read. |
| `a2a.server.message_send` | `caller_id`, `agent_id`, `task_id` | Unary RPC. |
| `a2a.server.tasks_get` / `tasks_list` / `tasks_cancel` / `tasks_resubscribe` | `task_id`, `caller_id` | Task lifecycle. |
| `a2a.server.push_notification_get` / `push_notification_list` | `caller_id`, `task_id` | Push config CRUD. |

## ACP

| Span | Fields | Notes |
|---|---|---|
| `acp.session.list` | `session_id` | Session lifecycle ops are instrumented per method. |
| `acp.client.fs.read_text_file` / `write_text_file` | `session_id`, `path` | Filesystem proxy. |
| `acp.client.terminal.*` | `session_id`, `terminal_id` | Terminal proxy. |
| `acp.client.session.update` | `session_id` | Server-initiated update. |
| `acp.logout` | `session_id` | Termination. |

## Correlation

- Every request carries a NATS `request_id` header that is reused as
  the span `request_id` field; see
  `docs/identity/reference-nats-headers.md`.
- Caller ID extraction (when JWT validation is on) lands in the
  `caller_id` field on every root span; see
  `gateway_audit_caller_attribution`.
- Audit envelope IDs (`audit_id`) link spans to the JSON envelope on
  the audit stream — useful when shipping audit to Splunk/Elastic via
  `TROGON_SIEM_FORMAT`.

## v0.2

- Audit envelope `trace_id` propagation so SIEM rows link back to the
  trace store.
- Span attribute schema published as a stable contract.
