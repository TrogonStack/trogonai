# A2A infra — TODO

Docker-only end-to-end harness for the A2A stack. Goal: `docker compose -f devops/docker/compose/compose.a2a.smoke.yml up` brings every binary online with auth-callout-minted JWTs flowing through gateway and bridge, and the smoke binary exercises the full path inside the compose network.

Code-side gaps live in [`A2A_TODO.md`](./A2A_TODO.md). This file tracks **only** the Docker/compose plumbing needed to validate that code end-to-end without a live cloud NATS.

Reference partials in-tree:

- `devops/docker/compose/compose.yml` — `trogon-gateway` stack only; not the A2A stack.
- `devops/docker/compose/compose.a2a.smoke.yml` — minimal JWT smoke profile (bootstrap → NATS 2.14 → auth-callout → gateway → bridge → echo agent → nats-server).
- `devops/docker/compose/services/*/Dockerfile` — multi-stage Rust images (shared `cargo chef cook` across A2A crates).
- `scripts/a2a-auth-callout-nats-server.conf` — NATS server config skeleton with auth-callout block.
- `scripts/a2a-nsc-bootstrap.sh` — generates the operator / account / signing keys + writes `auth-callout.env`.
- `scripts/a2a-bridge-test-nats-transport.sh` — bridge live-NATS smoke harness.
- `rsworkspace/crates/a2a-auth-callout/tests/nats_server_callout_integration.rs` — single `#[ignore]`-gated live-NATS test; pattern for compose-network integration tests.

## Phase 0 — service images

- [x] **`a2a-auth-callout` Dockerfile.** Multi-stage Rust build → `debian:bookworm-slim` runtime; entrypoint `/usr/local/bin/a2a-auth-callout`. Bake in `ca-certificates` only.
- [x] **`a2a-gateway` Dockerfile.** Same pattern; entrypoint `/usr/local/bin/a2a-gateway`.
- [x] **`a2a-bridge` Dockerfile.** Same pattern; entrypoint `/usr/local/bin/a2a-bridge`.
- [x] **`a2a-nats-server` Dockerfile.** Same pattern; entrypoint `/usr/local/bin/a2a-nats-server`.
- [x] **`a2a-nats-agent` Dockerfile** (plus echo example). Same pattern; entrypoint `/usr/local/bin/a2a-nats-agent` invoking the echo handler (`a2a-nats-agent-echo` bin). Used as the in-stack agent under test.

Conventions: pin Rust toolchain via workspace `Cargo.toml`; share dependency layers via `cargo chef` (`cargo chef cook` lists all A2A smoke binaries in every service Dockerfile). Documented in [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md).

## Phase 1 — bootstrap container

- [x] **`a2a-bootstrap` image.** Runs once at compose `up` time; on success the rest of the stack starts:
  - Generates operator + AUTH + APP + SYS accounts via `a2a-auth-callout-bootstrap.sh` and tenant ACLs via `a2a-nsc-bootstrap.sh`.
  - Generates auth-callout signing keys (current/previous file pair under `AUTH_CALLOUT_SIGNING_KEY_SOURCE=file`).
  - Renders the final `nats-server.conf` from `scripts/a2a-auth-callout-nats-server.conf` with the substituted issuer / xkey publics.
  - Renders `auth-callout.env` + `gateway.env` + `bridge.env` + `nats-server.env` + `agent.env` from the same key material.
  - Writes everything to a shared compose volume mounted by NATS, auth-callout, gateway, bridge, agent, and nats-server.
- [x] **Idempotency.** Re-running `docker compose up` after a clean compose volume must not require host-side state. Re-running against a hydrated volume short-circuits (`.bootstrap-complete` marker).

## Phase 2 — minimal smoke compose (`compose.a2a.smoke.yml`)

Proves the JWT plumbing end-to-end with no policy stack.

- [x] **`compose.a2a.smoke.yml`** wiring (in dependency order):
  1. `a2a-bootstrap` — runs to completion (init job).
  2. `nats` — `nats:2.14-alpine` with the rendered server config; depends on bootstrap completion.
  3. `a2a-auth-callout` — depends on `nats` healthy.
  4. `a2a-gateway` — depends on auth-callout subscribed (see Phase 2 health gate below).
  5. `a2a-bridge` — depends on gateway healthy.
  6. `a2a-nats-agent` (echo) — depends on auth-callout subscribed.
  7. `a2a-nats-server` — depends on agent + gateway.
- [x] **Auth-callout readiness gate.** `auth-callout` writes `/run/a2a-bootstrap/auth-callout-ready` after subscribing on `$SYS.REQ.USER.AUTH`; dependents use `condition: service_healthy`.
- [x] **Gateway env.** `A2A_GATEWAY_TRUST_CALLER_HEADERS=0` in the smoke profile so we exercise the JWT path, not the deprecated fallback. `A2A_GATEWAY_TIER1_SPICEDB_ENABLED=0`, `A2A_GATEWAY_TIER3_REDACTION_ENABLED=0`, `A2A_GATEWAY_AUDIT_PUBLISH=1`.
- [x] **`a2a-nats-server` env.** `A2A_USE_GATEWAY=1` + `A2A_GATEWAY_CALLER_JWT=<minted via bootstrap>` so its publishes to `{prefix}.gateway.*` carry `A2a-Caller-Jwt`.

## Phase 3 — end-to-end smoke test

- [x] **Smoke binary** (`crates/a2a-smoke-test`). One-shot: connects to compose-network NATS, runs `message/send` + `tasks/get` through gateway ingress, asserts gateway audit shows JWT-verified `caller_id` (`caller_source=jwt_header`).
- [x] **CI hook.** `make smoke` brings compose up, runs the smoke binary (`--profile smoke`), tears down. Does not gate `cargo test`.
- [x] **Live `a2a-bridge::nats_transport_harness` test.** `nats_transport_live_requires_nats_server` gated on `A2A_SMOKE_COMPOSE=1`.

## Phase 4 — full-stack compose (`compose.a2a.full.yml`, optional)

Closer to prod; slower to run. Add only when minimal smoke is green and stable.

- [x] **SpiceDB service** — `authzed/spicedb:latest` with a preloaded schema/relationships fixture covering the gateway's Tier-1 needs.
- [x] **Gateway env** — flip `A2A_GATEWAY_TIER1_SPICEDB_ENABLED=1` and point at the in-network SpiceDB.
- [x] **Tier-1 declarative bundles (optional).** Load reference bundles from `a2a-pack/policies/` and set `A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED=1`.
- [x] **Tier-3 signed bundles (optional).** Load `a2a-pack/skills/` with a smoke signing key and set `A2A_GATEWAY_TIER3_REDACTION_ENABLED=1`.
- [x] **Signed discovery exports (optional).** Provision a smoke operator key and exercise `a2a.discover.>` cross-Account flow.

## Phase 5 — docs

- [x] **`docs/A2A_DEVELOPMENT.md`** — add a "Local end-to-end" section pointing at the smoke compose: prerequisites, what each service does, how to read gateway audit logs to confirm the JWT path, common failure modes (auth-callout race, missing signing key, expired test JWT).
- [x] **`docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md`** — cross-link the bootstrap container as the reference implementation of the runbook's "generate keys + render config" steps; keep the manual flow as the prod reference.

## Shipped

1. **Phase 0 → Phase 1 → Phase 2 → Phase 3** — minimal Docker smoke (`compose.a2a.smoke.yml`, `make smoke`, `a2a-smoke-test`).
2. **Phase 4** — full-stack compose (`compose.a2a.full.yml`, `make smoke-full`, `a2a-smoke-test --profile full`): SpiceDB + seed, Tier-1 declarative bundles, Tier-3 signed WASM skills (`pii-regex-redactor`, `smoke-tier3-refuse`), operator-signed discovery gate. Tier-3 WASM build runs in the `a2a-bootstrap` image via `cargo chef` + `a2a-pack/skills/build.sh` (cold builds are slow; see [`docs/A2A_DEVELOPMENT.md`](./docs/A2A_DEVELOPMENT.md)).

## Suggested ordering

3. **Phase 5** — fold remaining auth-callout deployment cross-links into whichever phase landed last.

## Out of scope

- Production signing-key custody (Vault, KMS) — covered in `A2A_TODO.md` Phase 0 / operator runbook.
- Operator-signed policy bundle distribution pipeline — covered in `A2A_TODO.md` Phase 2.
- Cross-account NSC `push` automation — covered in `A2A_TODO.md` Phase 0.
- Anything that needs a real cloud NATS, real IdP, or real x509 chain.
