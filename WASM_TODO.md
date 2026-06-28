# WASM Decider — Remaining Work

Goal: deciders compile to WASM components (WIT-defined interface) so they can be
deployed/distributed independently of the host runtime.

v1 (M0 spike + M1 YAML test runner + scheduler WASM bundle) is **complete and merged**.
Design discovery, decisions, benchmarks, and the v1 implementation log have been removed —
they live in git history. What follows is only the work that has **not** been done.

## Deferred past v1 (P2)

- [ ] **Typed cross-language codec + proto→WIT emitter.** Publish a per-stream-type codec
      library component (`encode`/`decode` of version-faithful typed events) with WIT types
      generated from the same proto source (Nth codegen target beside Rust/Elixir). Must map
      proto open enums → WIT closed variant with an explicit `unknown` case; handle field
      presence, maps, well-known types, recursion. v1 cross-language consumers decode bytes
      with their own native proto library instead.
- [ ] **Platform service.** Host service embedding wasmtime: NATS command subjects in,
      JetStream persistence (replay/OCC/snapshots host-owned), components pulled from a
      registry. Untrusted-ready: fuel/epoch limits, memory caps, per-execution timeouts,
      instance isolation.
- [ ] **Registry / OCI distribution.** PR → CI builds artifact → registry → platform pulls
      by digest (digest pinning from the start; signing/provenance later).
- [ ] **Browser jco host.** jco-transpiled components running in-browser for unit testing
      and AI-sandbox simulation; reuse the in-memory sim host contract.
- [ ] **`Decision::Act` support.** Currently v1 is events-only (scheduler never uses `Act`).
      If needed, extend `decide`'s return with a variant for host-driven iteration — additive,
      does not break events-only modules.
- [ ] **StateModel / Command split (optional refactor).** Split the fused `Decider` trait
      into `StateModel` (State/Event/initial_state/evolve) + `Command` (decide), so bundle
      state-agreement becomes a type fact instead of macro-emitted bounds.

## Ideas (unscheduled)

- [ ] Expose the sim host as an MCP tool (via mcp-nats) so agents can load/test/simulate
      deciders over the wire.
- [ ] Conformance gate at registry upload: run the YAML given/when/then suites on push.
