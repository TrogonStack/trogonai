//! Wasmtime component bindgen for `policy-bundle` (internal; stable surface is `bindings.rs`).

#![allow(
    unused,
    clippy::all,
    reason = "generated bindgen output is allowed to carry unused items"
)]

wasmtime::component::bindgen!({
    world: "policy-bundle",
    path: ["wit"],
    imports: { default: store },
    include_generated_code_from_file: true,
});
