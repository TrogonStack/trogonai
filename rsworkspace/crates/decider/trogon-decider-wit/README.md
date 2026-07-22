# trogon-decider-wit

Per [ADR#0002](../../../docs/adr/0002-rust-crate-boundaries.md), this crate owns the
`trogon:decider` WIT contract (`wit/world.wit`) as generated Rust bindings, compiled for both
sides that implement it: `guest` (feature `guest`, `wit_bindgen::generate!`, for `wasm32`
components) and `host` (feature `host`, `wasmtime::component::bindgen!`, for non-`wasm32`
runtimes that load and drive them). It implements neither side itself; it exists so both a
guest component and a host runtime compile against exactly the same generated types
(`AnyEnvelope`, `CommandEnvelope`, `CommandSpec`, `DecideError`, `DomainError`, `Guest`,
`GuestSession`, `ModuleDescriptor`, `WritePrecondition`) rather than hand-kept mirrors of each
other.

`WIT_DIR`, an absolute path to this crate's own `wit/` directory baked in at this crate's
compile time, is what lets `export_decider!` (in `trogon-decider-guest-macros`) resolve the WIT
contract regardless of the calling component crate's location in the workspace.

See `wit/world.wit` for the interface itself, and
[Decider Platform](../../../docs/architecture/decider.md) for how the `handler` interface's
`descriptor`/`stream-id`/`session` shape maps onto the native `Decider` trait.
