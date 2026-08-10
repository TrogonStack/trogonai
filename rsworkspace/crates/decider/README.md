# Decider crates

Every stream here is driven by a pair of pure functions: `evolve` folds an event into state, and
`decide` takes that state plus a command and returns a decision. Neither touches I/O. The log is the
source of truth, and state is always a replay of it, from the beginning or resumed from a snapshot.
Because `decide` reads nothing outside its own arguments, it is testable without a broker and
deterministic on replay.

That logic is written once and runs two ways: compiled into a Rust process, or compiled to a WASM
component and driven by a sandboxed host. Both persist through the same storage-neutral ports, so a
decider never learns whether it is backed by an in-memory test double or NATS JetStream, nor which
path is executing it.

Eight crates keep three dependency sets apart. The domain crate compiles for `wasm32` and pulls
nothing but `thiserror`, the JetStream adapter pulls `async-nats`, and the WASM host pulls
`wasmtime`. Merged into one crate, all three would reach everyone at once: a NATS client for tests
that never touch JetStream, a wasm engine for services that never load a component, a broken
`wasm32` build for anything importing storage. Dependencies run one way, adapters onto the runtime,
runtime onto domain, domain onto nothing.

## The crates

- **[trogon-decider](./trogon-decider/README.md)** is the domain: the `Decider` trait,
  `Decision`/`Act`, `Events`, `WritePrecondition`, and the `EventEncode`/`EventDecode`/`EventType`
  codec traits. No I/O, which is what lets everything else sit on top of it.
- **[trogon-decider-runtime](./trogon-decider-runtime/README.md)** is the native execution boundary:
  `CommandExecution` over the storage-neutral `StreamRead`, `StreamAppend`, `SnapshotRead`, and
  `SnapshotWrite` ports. It picks no backend, so an adapter stays a thin translation to its own SDK.
- **[trogon-decider-nats](./trogon-decider-nats/README.md)** implements those ports against NATS
  JetStream and adds the `Projector` and `Processor` read-side primitives. It sits on the runtime
  rather than the domain, since the domain types it needs arrive through the runtime's re-exports.
- **[trogon-decider-wit](./trogon-decider-wit/README.md)** owns the `trogon:decider` WIT contract
  and generates the bindings both sides compile against, so guest and host share one set of
  generated types instead of each maintaining a mirror that can drift.
- **[trogon-decider-guest-macros](./trogon-decider-guest-macros/README.md)** is the
  `export_decider!` proc-macro, which needs its own crate because a proc-macro crate can export
  nothing else.
- **[trogon-decider-guest-sdk](./trogon-decider-guest-sdk/README.md)** bridges a `Decider`
  implementation to the WIT guest contract and re-exports `export_decider!` alongside the codec
  functions the macro's output calls into. Component authors depend on this one.
- **[trogon-decider-wasm-runtime](./trogon-decider-wasm-runtime/README.md)** is the production WASM
  host: it loads compiled components, enforces the engine budgets, and routes command types to
  modules. It drives the WIT `session` resource rather than the native `Decider` trait, because a
  guest's state lives behind a `wasmtime::Store` and a resource handle, not a value the trait's
  static `evolve`/`decide` functions could address.
- **[trogon-decider-sim](./trogon-decider-sim/README.md)** is the testing-only host, and the one
  crate that depends on both the native and WASM sides at once, which is what comparing them
  requires and what neither side may do without breaking its own boundary.

## The WASM path

The native path is simpler, so the sandbox has to pay for itself. A component is loaded by
instantiating it against an empty `wasmtime::component::Linker`, so one declaring any import fails
to load and cannot reach the host at all, where a native `Decider` is Rust linked into the process
and free to do anything that process can. Guest calls run under a fuel ceiling, an epoch-based
wall-clock backstop, and memory, table, and instance limits, so a decider that spins or
over-allocates becomes a contained trap instead of a host-process problem.

Routing is also swappable while the host runs. `DeciderRegistry` is immutable once built, but
`DeciderRegistryHandle` activates a new module version or retires a route with executions in flight,
installing the whole next routing table in one assignment so a multi-command module swaps as a unit.
Snapshot identity is keyed by module name and version, so a version bump never migrates or
invalidates its predecessor's snapshots: the new version finds nothing under its own id and falls
back to a full replay. A native decider is compiled into the host binary, so changing it means
redeploying the process.

Keeping the two paths honest is `trogon-decider-sim`'s job. Its `assert_parity` runs one scenario
through both and fails on the first divergence, catching codec, codegen, and WIT-contract drift,
where each path stays internally consistent while quietly disagreeing with the other.

## Related reading

[Decider Platform](../../../docs/architecture/decider.md) covers the full execution flow, error
taxonomy, engine budget values, and rollout semantics. Outside this directory,
[trogon-decider-test](../../cli/trogon-decider-test) is a CLI that runs YAML conformance suites
against a compiled component, and
[trogon-schedules-decider](../../wasm-components/trogon-schedules-decider) is a worked example of a
component built with these crates.
