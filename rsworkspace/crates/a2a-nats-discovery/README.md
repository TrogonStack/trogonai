# a2a-nats-discovery

Process that provisions the JetStream KV bucket **`A2A_AGENT_CARDS`**, serves **`{prefix}.discover.{agent_id}`** (AgentCard payloads), and **`{prefix}.catalog.register.{agent_id}`** (NATS **`request`**: body = raw JSON **AgentCard**, JSON-RPC reply envelope from the registrar). Mirrors module docs in **`a2a_nats::catalog`**. Catalog reads/writes through **`KvCatalogStore`** apply the **`a2a-pack` AgentCard JSON Schema**.

For KV **`watch`** patterns and client ergonomics see [`docs/catalog-kv-watch.md`](../../../docs/catalog-kv-watch.md).

Multi-tenant isolation is enforced by **separate NATS Accounts** (one per tenant), not a `{tenant}` subject segment. Each Account gets its own JetStream KV bucket **`A2A_AGENT_CARDS`** with the same name and shape; discovery subjects are `{prefix}.discover.{agent_id}` inside that Account namespace. See [`docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md`](../../../docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md) for operator provisioning.

## Run

From `rsworkspace/`:

```bash
cargo run -p a2a-nats-discovery
```

CLI / env:

- **`--nats-url`** / **`NATS_URL`** — comma-separated servers (default `localhost:4222`).
- **`--prefix`** / **`A2A_PREFIX`** — NATS prefix (default `a2a`).

Aggregated env reference: [`docs/A2A_RUNTIME_ENV.md`](../../../docs/A2A_RUNTIME_ENV.md) (**`a2a-nats-discovery`**). Docs hub: [`docs/A2A_DOCS_INDEX.md`](../../../docs/A2A_DOCS_INDEX.md).

Authentication (**`trogon_nats::NatsConfig`**) inherits the usual NATS_* / credential env knobs used elsewhere in Trogon crates.
