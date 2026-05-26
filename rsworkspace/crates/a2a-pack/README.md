# a2a-pack

First-party policy bundle for A2A over NATS. Peer of the MCP gateway pack; consumed by `a2a-gateway` once wired.

**`a2a-pack`** bundles JSON Schema aligned with **`a2a-types` 0.2** / **`supportedInterfaces`** for **`AgentCard`** validation at catalog boundaries—not a standalone runtime crate.

**Status: skeleton placeholder** for most modules; AgentCard validation is implemented.

## AgentCard validation

`agent_card_schema` ships a minimal draft-07 JSON Schema (`schemas/agent-card.min.json`) aligned with **`a2a-types` AgentCard**: non-empty **`name`** plus at least one **`supportedInterfaces[]`** entry with non-empty **`protocolBinding`/`protocolVersion`** and an absolute **`http`/`https` `url`**. Use `validate_agent_card_value(&serde_json::Value)` or `AgentCardJsonSchema::bundled().validate(...)` at catalog write/read boundaries before trusting KV payloads.

### Read-side enforcement (non-KV paths)

`agent_card_read` adds `validate_agent_card_on_read(value, AgentCardSource)` for materializations that bypass `KvCatalogStore::get_card`. Wired call sites:

| `AgentCardSource` | Crate / path |
|-------------------|--------------|
| `FederatedImport` | `a2a-nats::catalog::KvCatalogStore::list_cards_gated` (post SpiceDB allow) |
| `DiscoverResponse` | `a2a-nats::catalog::DiscoverService` reply shaping |
| `AgentHandler` | `a2a-nats::agent::agent_card` (`agent/getAuthenticatedExtendedCard`) |
| `GatewaySurface` | `a2a-gateway::agent_card_surface` (discover/catalog shaping at gateway edge) |

KV get/put remains validated in `KvCatalogStore`; read helpers drop invalid cards with `tracing::warn!` (batch responses omit bad entries).

## Intended contents

| Module | Purpose |
|--------|---------|
| `resource_tuples` | SpiceDB resource-tuple derivation table for A2A JSON-RPC methods (see `A2A_PLAN.md` §SpiceDB resource tuples). |
| `catalog` | Catalog shaping rules — filter AgentCards in `a2a.discover` responses by caller permission. |
| `agent_card_schema` | AgentCard JSON Schema for registration-time validation. |
| `redaction` | Schema-driven redaction over `Message.parts[*]` and `Artifact.parts[*]`, keyed by AgentCard skill id. |
| `audit` | Full A2A audit envelope schema extensions (trace_id, tenant, task_state_*, rules_fired, rewrites, etc.). |
| `rate_limit` | Default rate-limit profiles per skill kind (e.g. max concurrent streaming tasks per caller per agent). |

## Version

Bundle version is exposed as `a2a_pack::VERSION` (`0.0.0-skeleton` until real policy artifacts land).

## Related docs

- [`docs/catalog-kv-watch.md`](../../../docs/catalog-kv-watch.md) — registrar **`{prefix}.catalog.register.*`** ingress, **`KvCatalogStore`** get/put, and **`a2a-pack`** schema enforcement at the KV boundary.
- [`docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md`](../../../docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md) — NSC Account bootstrap, registrar service User ACLs, **`A2A_AGENT_CARDS`** bucket provisioning.
- [`A2A_PLAN.md`](../../../A2A_PLAN.md) — policy bundle layout, catalog write path, audit extensions.
