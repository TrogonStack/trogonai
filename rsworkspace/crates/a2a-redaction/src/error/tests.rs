use super::*;

#[test]
fn wasm_engine_variant_carries_underlying_message() {
    let inner = "module init failed";
    let RedactionError::WasmEngine(detail) = RedactionError::WasmEngine(inner.to_string()) else {
        panic!("expected WasmEngine variant");
    };
    assert_eq!(detail, inner);
}
