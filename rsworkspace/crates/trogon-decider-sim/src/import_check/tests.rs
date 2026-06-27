use super::*;

#[test]
fn rejects_core_module_with_imports() {
    let wasm = wat::parse_str(
        r#"
        (module
          (import "env" "memory" (memory 1))
          (func (export "noop"))
        )
        "#,
    )
    .unwrap();
    assert!(assert_zero_imports(&wasm).is_err());
}

#[test]
fn spike_fixture_has_no_external_imports() {
    let wasm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../trogon-decider-spike/../../target/wasm32-unknown-unknown/release/trogon_decider_spike.wasm");
    if !wasm_path.exists() {
        eprintln!("skip: build trogon-decider-spike for wasm32-unknown-unknown first");
        return;
    }
    let bytes = std::fs::read(wasm_path).unwrap();
    assert_zero_imports(&bytes).expect("spike must have zero world imports");
}
