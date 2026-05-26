# A2A over NATS — developer guide

Local setup and day-to-day commands for working on the A2A-over-NATS binding in this repository. For architecture and phased delivery, start with [A2A architecture](../explanation/architecture.md) and the navigation hub [`./README.md`](../README.md).

## Related links

| Document | Purpose |
|----------|---------|
| [Runtime env](../reference/runtime-env.md) | Env vars and CLI flags for every A2A binary and `a2a_nats` embedders |
| [`./README.md`](../README.md) | Full documentation index — operators, gateway, auth sketches |
| [A2A architecture](../explanation/architecture.md) | Open engineering work and suggested ordering |

---

## Workspace layout

All Rust crates live under **`rsworkspace/`**. That directory is the Cargo workspace root; run build and test commands from there unless a crate README says otherwise.

```bash
cd rsworkspace
```

The workspace uses **`resolver = "2"`** and **`warnings = "deny"`** at the workspace level. Clippy is configured with **`all = "deny"`** — treat clippy warnings as build failures.

---

## Common commands

### Run tests for the core library

Most protocol binding logic lives in **`a2a-nats`**. Run its test suite in isolation:

```bash
cd rsworkspace
cargo test -p a2a-nats
```

Add `-- --nocapture` when debugging a failing integration test. Other crates (`a2a-gateway`, `a2a-nats-agent`, …) have their own `#[cfg(test)]` modules; scope with `-p <crate>` the same way.

### Lint

Run clippy across the workspace before opening a PR:

```bash
cd rsworkspace
cargo clippy --workspace --all-targets
```

Fix or allow lints locally; do not weaken workspace lint levels for convenience.

### Run a binary locally

Examples (require a reachable NATS server — see env vars in [Runtime env](../reference/runtime-env.md)):

```bash
cargo run -p a2a-gateway
cargo run -p a2a-nats-agent    # requires A2A_AGENT_ID
cargo run -p a2a-nats-server   # HTTP JSON-RPC adapter; requires A2A_AGENT_ID
cargo run -p a2a-nats-stdio    # line-delimited JSON-RPC on stdin/stdout
cargo run -p a2a-nats-discovery
```

### Full workspace test pass

```bash
cd rsworkspace
cargo test --workspace
```

Use this before large refactors that touch shared types in `a2a-nats` or `a2a-pack`.

---

## Key crates

| Crate | Role |
|-------|------|
| **`a2a-nats`** | Core protocol binding — subject layout, `Client`, agent `Bridge`, catalog, push dispatch, audit emitter, JetStream helpers. Peer of `mcp-nats`. |
| **`a2a-gateway`** | NATS ingress on `{prefix}.gateway.>` — opaque forward to `{prefix}.agent.{id}.{method}` today; auth, policy, and audit per [Gateway roadmap](../explanation/gateway-roadmap.md). |
| **`a2a-pack`** | Policy bundle skeleton — AgentCard JSON Schema, future SpiceDB tuples, redaction, and audit schema extensions consumed by the gateway. |
| **`a2a-nats-stdio`** | MCP-style **stdio** helper — line-delimited JSON-RPC over stdin/stdout using `a2a_nats::Client` (no HTTP). Useful for editor and subprocess integrations. |
| **`a2a-nats-agent`** | Daemon shell wrapping a user-supplied `A2aHandler`; subscribes on `{prefix}.agent.{agent_id}.*`. |
| **`a2a-nats-server`** | Local axum HTTP/SSE adapter over `a2a_nats::Client` — narrower than the future [`a2a-bridge`](../explanation/bridge-sketch.md) HTTPS sidecar. |
| **`a2a-nats-discovery`** | Catalog registrar and `{prefix}.discover.*` service; provisions `A2A_AGENT_CARDS` KV. |

Shared infrastructure: **`trogon-nats`** (connection management, test mocks via `test-support`), **`a2a-types`** (generated A2A wire types).

---

## Configuration during development

Binaries read NATS connection settings from **`trogon_nats`** env vars (`NATS_URL`, `NATS_CREDS`, …) and A2A-specific knobs from **`a2a_nats::Config`** overrides (`A2A_PREFIX`, timeouts, push DLQ caller segment). The consolidated reference is [Runtime env](../reference/runtime-env.md) — use it instead of hunting per-crate `runtime.rs` files.

Typical local loop:

1. Start NATS (with JetStream enabled for streaming and catalog tests).
2. Export `NATS_URL` and, when testing gateway routing, run `a2a-gateway` in one terminal.
3. Run an agent (`a2a-nats-agent`) and a client adapter (`a2a-nats-server`, `a2a-nats-stdio`, or an embedder test harness).

Set `A2A_USE_GATEWAY=1` on **`a2a-nats-server`** to route unary traffic via `{prefix}.gateway.*` instead of direct `{prefix}.agent.*` publish.

---

## Local end-to-end (Docker smoke)

The opt-in compose stack at [`../devops/docker/compose/compose.a2a.smoke.yml`](../../../devops/docker/compose/compose.a2a.smoke.yml) proves auth-callout minting, gateway JWT verification, and `tasks/get` through the echo agent without cloud NATS. Service images use **cargo-chef**: each Dockerfile runs `cargo chef cook` for all A2A binaries (`a2a-auth-callout`, `a2a-gateway`, `a2a-bridge`, `a2a-nats-server`, `a2a-nats-agent`, `a2a-smoke-test`) so dependency layers stay warm across rebuilds.

```bash
make smoke          # up, run a2a-smoke-test, down
make smoke-build    # images only
```

Prerequisites: Docker with Compose v2, enough RAM for parallel Rust image builds (first build is slow; later builds reuse chef layers). The `a2a-bootstrap` init container runs [`scripts/a2a-nsc-bootstrap.sh`](../../../scripts/a2a-nsc-bootstrap.sh) and [`scripts/a2a-auth-callout-bootstrap.sh`](../../../scripts/a2a-auth-callout-bootstrap.sh), renders `nats-server.conf`, and mints a long-lived smoke caller JWT into the shared volume. Confirm JWT attribution in gateway audit subjects `{prefix}.audit.ok.tasks.get` — `caller_source` must be `jwt_header`, not `_` or header-trust fallbacks (`A2A_GATEWAY_TRUST_CALLER_HEADERS=0` in the smoke profile).

Common failures: starting gateway before `auth-callout-ready` (race on `$SYS.REQ.USER.AUTH`), missing signing key files on the volume, or an expired smoke JWT (regenerate with `docker compose run --rm a2a-bootstrap` after `make smoke-down`).

---

## Full-stack smoke (Docker)

[`../devops/docker/compose/compose.a2a.full.yml`](../../../devops/docker/compose/compose.a2a.full.yml) includes the minimal smoke stack and adds policy + federation plumbing:

| Service | Role |
|---------|------|
| `spicedb` | In-memory Authzed/SpiceDB (`serve --grpc-preshared-key=devkey`) |
| `a2a-spicedb-seed` | One-shot `zed schema write` + `zed import` of [`schema.zed`](../../../devops/docker/compose/services/spicedb/schema.zed) and [`relationships.yaml`](../../../devops/docker/compose/services/spicedb/relationships.yaml); writes `.spicedb-seed-complete` on the shared volume. Re-runs on every `up` because SpiceDB uses an in-memory datastore (schema is lost when the container restarts). |
| `a2a-bootstrap` (`A2A_COMPOSE_PROFILE=full`) | Copies Tier-1 `per-method-allowlist.tier1.toml`, stages Tier-3 WASM + manifests (`$.params.message.parts[0].text` matches protobuf JSON), signs skills with `a2a-sign-bundle`, writes gateway env (`A2A_GATEWAY_TIER1_SPICEDB_*`, declarative/Tier-3 dirs, `A2A_DISCOVERY_OPERATOR_KEYS`) |

```bash
make smoke-full       # down -v, up --build, run a2a-smoke-test --profile full, down -v
make smoke-full-down  # compose down -v (smoke-full profile)
```

`a2a-smoke-test --profile full` checks: Tier-1 declarative deny (`agent.card` → `-32801`), Tier-3 refusal (`SMOKE_T3_REFUSE_ME` → `-32802` + audit `refusal_skill`), allow-path `message/send` audit (`rules_fired` includes `gateway.tier1.spicedb_allowed` and `gateway.tier3.evaluated_allow` / `gateway.tier3.redacted`), and operator-signed `SignedDiscoveryExport` verification.

**Reading audits:** Gateway publishes `{prefix}.audit.ok|err.{method-dots}` (slashes become dots). Policy decisions appear in `rules_fired` (for example `gateway.tier1.declarative.denied.*`, `gateway.tier3.refused.*`). There are no separate `tier1_decision` / `tier3_decision` fields on [`AuditEnvelope`](../../../rsworkspace/crates/a2a-nats/src/audit/envelope.rs).

**Tier-3 WASM build cost:** The bootstrap image compiles `pii-regex-redactor` and `smoke-tier3-refuse` for `wasm32-unknown-unknown` via [`a2a-pack/skills/build.sh`](../../../rsworkspace/crates/a2a-pack/skills/build.sh). First `make smoke-full` can take several minutes; later rebuilds reuse the shared `cargo chef` layer from service images.

**Common failures:**

- SpiceDB schema missing after restart — in-memory SpiceDB loses data when the container is recreated; `a2a-spicedb-seed` must run before gateway (compose ordering). If Tier-1 denies with `object definition not found`, run `make smoke-full` (it clears volumes) or `make smoke-full-down` then `make smoke-full`.
- SpiceDB schema/relationship mismatch — edit `schema.zed` / `relationships.yaml`, then `make smoke-full-down` and `make smoke-full`.
- WASM build failure — missing `wasm32-unknown-unknown` in the bootstrap build stage; inspect `a2a-bootstrap` build logs.
- Signature verification failure — `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` must match the seed used by `a2a-sign-bundle` in bootstrap; regenerate volume with `make smoke-full-down`.

---

## Where to go next

- **Operators / NSC bootstrap:** [NSC account bootstrap](../how-to/operators/nsc-account-bootstrap.md)
- **Subject ACL quick reference:** [Subject ACL quickref](../reference/subject-acl-quickref.md)
- **HTTPS interop sidecar (future):** [Bridge sketch](../explanation/bridge-sketch.md)
- **Open work tracker:** [A2A architecture](../explanation/architecture.md)
