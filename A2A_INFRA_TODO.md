# A2A infra — TODO

Docker-only end-to-end harness for the A2A stack. Goal: `docker compose -f devops/docker/compose/compose.a2a.yml up` brings every binary online with auth-callout-minted JWTs flowing through gateway and bridge, and `cargo test` (or a smoke binary) exercises the full path inside the compose network.

Code-side gaps live in [`A2A_TODO.md`](./A2A_TODO.md). This file tracks **only** the Docker/compose plumbing needed to validate that code end-to-end without a live cloud NATS.

Reference partials in-tree:

- `devops/docker/compose/compose.yml` — `trogon-gateway` stack only; not the A2A stack.
- `devops/docker/compose/services/trogon-gateway/Dockerfile` — the existing pattern for a Rust binary image.
- `scripts/a2a-auth-callout-compose.yaml` — reference compose for NATS 2.14 + auth-callout; requires a host-built release binary and pre-rendered NSC keys.
- `scripts/a2a-auth-callout-nats-server.conf` — NATS server config skeleton with auth-callout block.
- `scripts/a2a-nsc-bootstrap.sh` — generates the operator / account / signing keys + writes `auth-callout.env`.
- `scripts/a2a-bridge-test-nats-transport.sh` — bridge live-NATS smoke harness.
- `rsworkspace/crates/a2a-auth-callout/tests/nats_server_callout_integration.rs` — single `#[ignore]`-gated live-NATS test; pattern for compose-network integration tests.

## Phase 0 — service images

- [ ] **`a2a-auth-callout` Dockerfile.** Multi-stage Rust build → `debian:bookworm-slim` runtime; entrypoint `/usr/local/bin/a2a-auth-callout`. Bake in `ca-certificates` only.
- [ ] **`a2a-gateway` Dockerfile.** Same pattern; entrypoint `/usr/local/bin/a2a-gateway`.
- [ ] **`a2a-bridge` Dockerfile.** Same pattern; entrypoint `/usr/local/bin/a2a-bridge`.
- [ ] **`a2a-nats-server` Dockerfile.** Same pattern; entrypoint `/usr/local/bin/a2a-nats-server`.
- [ ] **`a2a-nats-agent` Dockerfile** (plus echo example). Same pattern; entrypoint `/usr/local/bin/a2a-nats-agent` invoking the echo example handler. Used as the in-stack agent under test.

Conventions: pin Rust toolchain via `rust-toolchain.toml`, share a single workspace-cached build stage via `cargo chef` or buildkit `--mount=type=cache` so the five images don't rebuild dependencies from scratch.

## Phase 1 — bootstrap container

- [ ] **`a2a-bootstrap` image.** Runs once at compose `up` time; on success the rest of the stack starts:
  - Generates operator + AUTH + APP + SYS accounts via `a2a-nsc-bootstrap.sh`.
  - Generates auth-callout signing keys (current/previous file pair under `AUTH_CALLOUT_SIGNING_KEY_SOURCE=file`).
  - Renders the final `nats-server.conf` from `scripts/a2a-auth-callout-nats-server.conf` with the substituted issuer / xkey publics.
  - Renders `auth-callout.env` + `gateway.env` + `bridge.env` + `nats-server-client.creds` from the same key material.
  - Writes everything to a shared compose volume mounted by NATS, auth-callout, gateway, bridge, and `a2a-nats-server`.
- [ ] **Idempotency.** Re-running `docker compose up` after a clean compose volume must not require host-side state. Re-running against a hydrated volume short-circuits.

## Phase 2 — minimal smoke compose (`compose.a2a.smoke.yml`)

Proves the JWT plumbing end-to-end with no policy stack.

- [ ] **`compose.a2a.smoke.yml`** wiring (in dependency order):
  1. `a2a-bootstrap` — runs to completion (init job).
  2. `nats` — `nats:2.14-alpine` with the rendered server config; depends on bootstrap completion.
  3. `a2a-auth-callout` — depends on `nats` healthy.
  4. `a2a-gateway` — depends on auth-callout subscribed (see Phase 2 health gate below).
  5. `a2a-bridge` — depends on gateway healthy.
  6. `a2a-nats-agent` (echo) — depends on auth-callout subscribed.
- [ ] **Auth-callout readiness gate.** `auth-callout` exposes a readiness signal (subject ping, file marker, or healthcheck endpoint) that means "subscribed on `$SYS.REQ.USER.AUTH`." Dependent services must `condition: service_healthy` against it — otherwise their first connect attempt races the callout subscription.
- [ ] **Gateway env.** `A2A_GATEWAY_TRUST_CALLER_HEADERS=0` in the smoke profile so we exercise the JWT path, not the deprecated fallback. `A2A_GATEWAY_TIER1_SPICEDB_ENABLED=0`, `A2A_GATEWAY_TIER3_REDACTION_ENABLED=0`.
- [ ] **`a2a-nats-server` env.** `A2A_USE_GATEWAY=1` + `A2A_GATEWAY_CALLER_JWT=<minted via bootstrap>` so its publishes to `{prefix}.gateway.*` carry `A2a-Caller-Jwt`.

## Phase 3 — end-to-end smoke test

- [ ] **Smoke binary** (new `crates/a2a-smoke-test` or `xtask` subcommand). One-shot: connects to the compose-network NATS, calls `tasks/get` through `a2a-nats-server` → gateway → agent, asserts the response and that gateway audit shows a JWT-verified `caller_id` (not the `_` fallback).
- [ ] **CI hook.** Optional `make smoke` / `cargo xtask smoke` target that brings the compose up, runs the smoke binary, and tears down. Don't gate `cargo test` on Docker; keep the smoke explicitly opt-in (matches the existing `#[ignore]` live-NATS test convention).
- [ ] **Live `a2a-bridge::nats_transport_harness` test.** Re-enable the existing `#[ignore]`-gated live-NATS smoke under the compose network so the bridge path is exercised too.

## Phase 4 — full-stack compose (`compose.a2a.full.yml`, optional)

Closer to prod; slower to run. Add only when minimal smoke is green and stable.

- [ ] **SpiceDB service** — `authzed/spicedb:latest` with a preloaded schema/relationships fixture covering the gateway's Tier-1 needs.
- [ ] **Gateway env** — flip `A2A_GATEWAY_TIER1_SPICEDB_ENABLED=1` and point at the in-network SpiceDB.
- [ ] **Tier-1 declarative bundles (optional).** Load reference bundles from `a2a-pack/policies/` and set `A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED=1`.
- [ ] **Tier-3 signed bundles (optional).** Load `a2a-pack/skills/` with a smoke signing key and set `A2A_GATEWAY_TIER3_REDACTION_ENABLED=1`.
- [ ] **Signed discovery exports (optional).** Provision a smoke operator key and exercise `a2a.discover.>` cross-Account flow.

## Phase 5 — docs

- [ ] **`docs/A2A_DEVELOPMENT.md`** — add a "Local end-to-end" section pointing at the smoke compose: prerequisites, what each service does, how to read gateway audit logs to confirm the JWT path, common failure modes (auth-callout race, missing signing key, expired test JWT).
- [ ] **`docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md`** — cross-link the bootstrap container as the reference implementation of the runbook's "generate keys + render config" steps; keep the manual flow as the prod reference.

## Suggested ordering

1. **Phase 0 → Phase 1 → Phase 2 → Phase 3** as one swarm task (minimal smoke). This is the highest-value deliverable: it validates the entire signed-header JWT plumbing without any operator infra. Most of the remaining `A2A_TODO.md` "Remaining: signing-key custody + publisher rollout" handwringing collapses once we can prove the path locally.
2. **Phase 4** — only after the smoke is green. Adds policy and federation surface area.
3. **Phase 5** — fold docs into whichever phase landed last.

## Out of scope

- Production signing-key custody (Vault, KMS) — covered in `A2A_TODO.md` Phase 0 / operator runbook.
- Operator-signed policy bundle distribution pipeline — covered in `A2A_TODO.md` Phase 2.
- Cross-account NSC `push` automation — covered in `A2A_TODO.md` Phase 0.
- Anything that needs a real cloud NATS, real IdP, or real x509 chain.
