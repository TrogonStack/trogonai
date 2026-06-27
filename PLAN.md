# Plan: Honest Coverage — Remove `cfg(coverage)` Gating and Add Real Tests

## Why

CI gates Rust coverage at 95% (`/.github/workflows/ci-rust.yml`, `coverage-action`,
`threshold: 95`, `fail: true`). That number is currently inflated. Two mechanisms
shrink the measured surface:

1. **`--ignore-filename-regex`** in `rsworkspace/.cargo/config.toml` excludes
   generated code (`trogonai-proto`, `trogon-semconv` `src/gen/`). This is
   legitimate and stays.
2. **A `#[cfg(not(coverage))]` / `#[cfg(coverage)]` dual-cfg trick** — 175 gates
   across 41 files. The real implementation lives behind `cfg(not(coverage))`; a
   parallel `cfg(coverage)` twin returns `coverage_unavailable(...)` or collapses
   a match. Under coverage builds the real code disappears, so untested branches,
   loops, and helpers never count against the gate.

The goal is to make the 95% honest: test the gated code, then delete the gates so
the real implementation re-enters measurement.

## The per-item loop

For every gated item:

1. Write the test that exercises the real code path.
2. Delete the `#[cfg(not(coverage))]` attribute **and** its `#[cfg(coverage)]`
   stub twin (including any `coverage_unavailable` helper left unused).
3. Run `cargo cov nextest` (from `rsworkspace/`) and confirm the gate stays green
   with the code now measured.
4. Remove `unexpected_cfgs` `check-cfg = ['cfg(coverage)']` entries from a crate's
   `Cargo.toml` only once that crate has zero remaining `cfg(coverage)` uses.

## Scope inventory

41 files, grouped by how they should be handled. Counts are
`#[cfg(not(coverage))]` gates per file.

### Tier 1 — Pure logic swept under the gate (no infra, highest ROI)

Unit-testable today; no NATS/JetStream needed.

- [x] `trogon-aauth-verify/src/token.rs` — `jwk_compatible_with_alg` ES384/EdDSA
      arms (stub collapses to ES256)
- [x] `trogon-aauth-verify/src/nats_pop.rs`
- [x] `a2a-nats-stdio/src/io_loop.rs` — `writer_task_err` (pure `Result` mapper)
- [x] `trogon-scheduler/src/projections/schedules/mod.rs` — pure
      `From<ScheduleProjection> for ScheduleStreamState` and pure fold helpers
- [x] `a2a-gateway/src/lib.rs`
- [ ] `trogon-scheduler/src/lib.rs`

### Tier 2 — KV / JetStream-bound logic (needs integration harness)

Real coverage value; `trogon-scheduler/tests/integration.rs` already provides a
NATS-backed harness to build on (ADR 0010 testcontainers pattern).

- [ ] `trogon-scheduler/src/queries/{get,list,mod}.rs`
- [ ] `trogon-scheduler/src/store/{event_store,connect}.rs`
- [ ] `trogon-scheduler/src/kv.rs`
- [ ] `trogon-scheduler/src/nats.rs`
- [ ] `trogon-scheduler/src/projections/{mod,schedules/storage}.rs`
- [ ] `trogon-decider-nats/src/store.rs`
- [ ] `trogon-nats/src/jetstream/{mod,object_store,traits}.rs`

### Tier 3 — I/O loops and runtimes (timing-dependent + live infra)

Hardest: `tokio::select!` arms with writer-died / shutdown races plus
unreachable defenses. Need mock transports or testcontainers.

- [ ] `a2a-nats-stdio/src/{runtime,io_loop}.rs` (+ gated `*/tests.rs`)
- [ ] `a2a-nats-http/src/runtime.rs`
- [ ] `a2a-nats-server/src/runtime.rs`
- [ ] `a2a-gateway/src/push_dlq_mirror.rs`
- [ ] `trogon-gateway/src/source/{discord/mod,slack/socket_mode}.rs`

### Tier 4 — Binary entry points (`main.rs` / `bin/`)

Mostly bootstrap wiring; lowest value. Either accept exclusion under a documented
policy, or extract a testable `run()` from `main()` and add thin smoke tests.

- [ ] `trogon-gateway/src/main.rs` (15 — largest)
- [ ] `a2a-nats-server/src/main.rs` (11)
- [ ] `a2a-auth-callout/src/main.rs` (9)
- [ ] `a2a-bridge/src/main.rs` (9)
- [ ] `a2a-redaction/src/bin/sign-bundle.rs` (7)
- [ ] `mcp-nats-stdio/src/main.rs` (5)
- [ ] `a2a-gateway/src/main.rs` (4)
- [ ] `acp-nats-server/src/main.rs`, `acp-nats-stdio/src/main.rs`,
      `mcp-nats-server/src/main.rs`, `a2a-nats-http/src/main.rs`,
      `a2a-nats-stdio/src/main.rs`

## Sequencing

1. **Tier 1 first** — pure functions, immediate honest coverage, and it proves
   the test → remove-both-twins → gate-stays-green loop end to end.
2. **Tier 2** — the scheduler integration harness already exists.
3. **Tier 3** — invest in mock transports / testcontainers as needed.
4. **Tier 4** — decide policy (smoke-test vs. documented exclusion) last.

## Keep excluded (out of scope)

- Generated code via `--ignore-filename-regex` (`trogonai-proto`,
  `trogon-semconv` `src/gen/`). Do not test generated output.

## Validation

- `cargo cov nextest --cobertura --output-path coverage.xml` from `rsworkspace/`,
  matching CI.
- Each PR keeps the 95% gate green with strictly more real code measured (no use
  of the `rust:coverage-baseline-reset` label as a shortcut).
- Track progress by checking off the boxes above as gates are removed.

## References

- `/.github/workflows/ci-rust.yml` — coverage gate
- `rsworkspace/.cargo/config.toml` — `cov` alias and ignore-regex
- `docs/adr/0010-*` — testcontainers / real-NATS integration testing
