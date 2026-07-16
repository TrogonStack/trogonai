# trogon-decider-guest-macros

The `-macros` suffix (per [ADR 0002](../../../docs/adr/0002-rust-crate-boundaries.md)) marks a
proc-macro crate; this one owns exactly one function-like macro, `export_decider!`, which turns
one or more `Decider` implementations sharing a `State`/`Event` into a compiled WASM
component's WIT glue: a hidden `wit_bindgen::generate!` bindings module, a `Component` unit
struct implementing the WIT `Guest` trait, and a `Session` struct implementing `GuestSession`
(`new`, `evolve`, `decide`, `snapshot`) around the native decider's own `evaluate_decision`
logic. It reads `trogon_decider_wit::WIT_DIR` at this crate's own compile time so the generated
`wit_bindgen::generate!` call's `path:` stays an absolute path, independent of the calling
component crate's directory depth.

This crate is a macro-expansion detail; application code depends on it indirectly through
`trogon-decider-guest-sdk`, which re-exports `export_decider!` alongside the trait/codec glue
the macro's generated code calls into.

See [`trogon-decider-guest-sdk`'s README](../trogon-decider-guest-sdk/README.md) for the
`export_decider!` call shape, and
[Decider Platform](../../../docs/architecture/decider.md) for how the generated component fits
into the WASM execution path.
