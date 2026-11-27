# passthrough-guest

The guest module the worker's end-to-end tests and examples load. It exports `echo_double(i32) -> i32`,
`fill(i32)`, which returns that many bytes, and the `malloc`/`free` pair hyperlight-wasm needs for byte
returns. It imports nothing from the host.

Build it for `wasm32-unknown-unknown`, then compile the module ahead of time with the hyperlight-wasm AOT
compiler from the same hyperlight-wasm revision the worker pins:

```text
cargo build --release --manifest-path tests/fixtures/passthrough-guest/Cargo.toml
hyperlight-wasm-aot compile \
    tests/fixtures/passthrough-guest/target/wasm32-unknown-unknown/release/passthrough_guest.wasm \
    passthrough_guest.aot
```

Point `HYPERLIGHT_TEST_MODULE` at the `.aot` file to run the ignored hypervisor tests.
