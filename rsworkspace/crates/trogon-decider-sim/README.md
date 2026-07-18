# trogon-decider-sim

Per [ADR#0002](../../../docs/adr/0002-rust-crate-boundaries.md), this crate is a testing-only
adapter around the same wasmtime host machinery `trogon-decider-wasm-runtime` uses in
production, so a decider's guest behavior can be exercised in-process without a real
deployment. `SimHost`/`SimInstance` load a component and run every guest call against the same
production resource budget (`WasmEngineConfig`) `trogon-decider-wasm-runtime` enforces, so a
guest that would trap or exceed its deadline in production does the same in a test rather than
running unbounded. `SimScenario` is a fluent given/when/then builder for driving one guest
session through an ordered sequence of commands. `ScenarioIr`/`WireEnvelope` (`ir.rs`) express a
decider-agnostic scenario in wire form so the same scenario can run against both a wasm
component and, with the `test-support` feature, a native `Decider` implementation
(`native.rs`), letting `assert_parity` (`parity.rs`) fail a test on the first divergence
between the two.

`assert_zero_imports` (`import_check.rs`) checks a compiled component declares no imports via
`wasm-tools`, matching the same requirement `WasmDeciderModule::load` enforces structurally at
load time.

See `tests/schedules.rs` for a worked example, and
[Decider Platform](../../../docs/architecture/decider.md) for how this harness relates to
`TestCase` and the `decider-test` CLI.
