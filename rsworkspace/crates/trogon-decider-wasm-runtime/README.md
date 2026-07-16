# trogon-decider-wasm-runtime

The `-runtime` suffix (per [ADR 0002](../../../docs/adr/0002-rust-crate-boundaries.md)) marks
execution machinery, here for deciders compiled to WASM components rather than linked in
natively. `WasmDeciderModule::load` compiles a component, structurally enforces it declares
zero imports (by instantiating it against an empty `wasmtime::component::Linker`), and probes
its `descriptor()` once. `WasmDeciderEngine`/`WasmEngineConfig` configure one shared wasmtime
`Engine` per process with fuel metering and epoch-based interruption, giving every guest call a
bounded instruction count and a bounded wall-clock deadline. `WasmCommandExecution` is this
crate's analog of `trogon-decider-runtime`'s `CommandExecution`, driving the WIT `session`
resource through replay, decide, event-folding, and snapshot instead of a native `Decider`
implementation directly. `DeciderRegistry`/`DeciderRegistryHandle` route command types to
modules and support activating a new module version or retiring a route while executions are
in flight, with snapshot identity (`WasmSnapshotId`) keyed by module name and version so a
version swap never has to migrate or invalidate a prior version's snapshots.

This crate reuses `trogon-decider-runtime`'s storage-neutral ports (`StreamRead`,
`StreamAppend`, `SnapshotRead`, `SnapshotWrite`) rather than depending on any specific backend.

See [Decider Platform](../../../docs/architecture/decider.md) for the exact engine budget
values, the decide/fold/snapshot call ordering, and registry rollout semantics in full.
