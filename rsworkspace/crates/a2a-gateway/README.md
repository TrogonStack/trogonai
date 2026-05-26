# a2a-gateway

Pass-through ingress seam for future A2A gateway authz and policy. **Forwarding only** — opaque request/reply relay on `{prefix}.gateway.>`; **no push delivery and no DLQ**. Terminal push DLQ publishes live on the agent **`Bridge`** in `a2a-nats`, not on this gateway.

**Related:** [Gateway roadmap](../../../docs/A2A_GATEWAY_ROADMAP.md) · [Auth callout design](../../../docs/A2A_AUTH_CALLOUT_DESIGN.md) · [A2A_PLAN.md](../../../A2A_PLAN.md)

## Role

Optional queue-group subscriber on `{prefix}.gateway.>`. Maps `{prefix}.gateway.{agent_id}.{method…}` → `{prefix}.agent.{agent_id}.{method…}` and relays payload + headers via `publish_with_reply_and_headers` (caller reply inbox preserved). Invalid ingress shapes get JSON-RPC `-32600` on the reply inbox when present; messages without a reply inbox are ignored (logged). Subject parsing is in **`a2a_nats::gateway_ingress`**.

Multi-tenancy is per **NATS Account** — no `{tenant}.{agent_id}` segment on ingress subjects.

## Run

From `rsworkspace/`:

```bash
cargo run -p a2a-gateway
```

| Variable | Default | CLI flag |
|----------|---------|----------|
| `NATS_URL` | `localhost:4222` | `--nats-url` |
| `A2A_PREFIX` | `a2a` | `--prefix` |
| `A2A_GATEWAY_QUEUE_GROUP` | _(unset → ephemeral subscriber)_ | `--queue-group` |

CLI flags override env when passed explicitly.

## Observability

Span `gateway.ingress.dispatch`: **`gateway_ingress.subject`**, `ingress.reply_present`, `caller_id` (empty until JWT), `agent_subject`, `routing_outcome` (`forwarded`, `ingress_error`, `ignored_no_reply`, `forward_failed`). Example: `RUST_LOG=a2a_gateway=debug`.

## Limitations (today)

No auth callout / JWT validation, policy engine, or ingress audit. Streaming back-pressure across the gateway (pull consumer egress, JetStream policy alignment) remains future — see [`../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md`](../../../docs/A2A_STREAMING_BACKPRESSURE_OPS.md). See roadmap links above for the full backlog.
