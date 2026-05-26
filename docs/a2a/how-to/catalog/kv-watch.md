# AgentCard catalog — KV watch (`push discovery`)

Practical guidance for callers that prefer **push-driven catalog freshness** instead of polling KV **get** or invoking **DiscoverService** request/reply.

Bucket name: **`A2A_AGENT_CARDS`** — see `./rsworkspace/crates/a2a-nats/src/catalog/nats_kv.rs` (`A2A_AGENT_CARDS`, `catalog_bucket_config`) and the registrar binary `./rsworkspace/crates/a2a-nats-discovery/`.

## Related docs

- [Subject ACL quick reference](../../reference/subject-acl-quickref.md)
- [NSC account bootstrap](../operators/nsc-account-bootstrap.md)
- [Runtime environment](../../reference/runtime-env.md)
- [A2A docs index](../../../A2A_DOCS_INDEX.md)

## Pattern

Use JetStream KV **watch** (`async_nats::jetstream::kv::Store::watch`):

1. Build a JetStream context on the cluster that hosts `A2A_AGENT_CARDS`.
2. Open the KV bucket backing `A2A_AGENT_CARDS`.
3. `watch(&agent_key)` (or iterate `watch_all` / multiple keys explicitly) → stream of **`KvEntry`** revisions keyed by `{agent_id}`.
4. On live **Put**/update payloads (skip tombstones you do not model), deserialize to `AgentCard` at the KV boundary (`KvCatalogStore::get_card` shows the deserialization contract).

Maintain **named long-lived watchers** for hot agent ids rather than spinning a watcher per cold lookup.

## Catalog write path (KV)

Agents (or automation) can publish JSON **AgentCard** documents via NATS **`request`** to **`{prefix}.catalog.register.{agent_id}`** (body = raw AgentCard JSON matching the KV value shape). The **`a2a-nats-discovery`** binary runs **`catalog::CatalogRegistrarService`**, which applies the **`a2a-pack`** JSON Schema before **`KvCatalogStore::put_card`**. Production deployments should restrict **publish** on this subject pattern to the **registrar NATS User** via ACLs.

## Limitations today

- **Keyed wildcard KV watch.** JetStream KV **watch** has no `>` wildcard analogue; curate agent ids explicitly, run multiple `watch(&agent_key)` handles, or reconcile via periodic `keys()` / policy lists.
- **Tenancy and federation.** Multi-tenant isolation is **NATS Account**–scoped subject prefixes (no `{tenant}` segment in catalog keys or watch subjects). Cross-Account catalog visibility is opt-in via operator-signed Account exports/imports — see [`./A2A_ARCHITECTURE.md`](../../explanation/architecture.md) (§8 federated discovery). KV watch callers still connect to the Account that hosts `A2A_AGENT_CARDS`; there is no built-in cross-Account KV replication in this pattern.
- **Semantics / durability** ([`./A2A_ARCHITECTURE.md`](../../explanation/architecture.md) §§2–§3): retries, quotas, SpiceDB‑gated catalogs are still future.
- **Catalog payload shape.** `KvCatalogStore` validates AgentCard payloads against the bundled **`a2a-pack` JSON Schema** on **KV get/put**. Legacy junk bytes in KV will surface as `CatalogStoreError::AgentCardSchema` on read. Non-KV read paths (federated import lists, discover replies, agent handler cards, gateway surface) re-validate via `a2a_pack::agent_card_read` — see **`a2a-pack` README** §Read-side enforcement.

See [`async-nats` KV module docs](https://docs.rs/async-nats/latest/async_nats/jetstream/kv/index.html).
