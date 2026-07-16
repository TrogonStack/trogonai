# trogon-decider

The `trogon-*` prefix (per [ADR 0002](../../../docs/adr/0002-rust-crate-boundaries.md)) marks
first-party platform crates whose public contract does not depend on this repository's own
product assumptions; this crate is the pure decision-logic domain: the `Decider` trait
(`stream_id`, `initial_state`, `evolve`, `decide`, `decide_error_code`), the `Decision` enum
(`Decision::Events` and the `Decision::act()` multi-step builder), `WritePrecondition`, and the
`EventEncode`/`EventType`/`EventDecode` codec traits events must implement to be replayed and
persisted. It knows nothing about storage, transport, or WASM; those live in
`trogon-decider-runtime`, `trogon-decider-nats`, and `trogon-decider-wasm-runtime` respectively.

It also ships `testing` (feature `test-support`): `TestCase`, a given/when/then typestate
builder for asserting a `Decider`'s behavior directly against a `History` of prior events,
without any storage adapter.

See the crate's own doc comment (`src/lib.rs`) for a complete `Decider` implementation, and
[Decider Platform](../../../docs/architecture/decider.md) for how this trait's output flows
through the native and WASM execution paths.
