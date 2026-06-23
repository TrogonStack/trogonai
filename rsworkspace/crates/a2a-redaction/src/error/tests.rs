use super::*;

#[test]
fn wasm_engine_variant_carries_underlying_message() {
    let inner = "module init failed";
    let error = RedactionError::WasmEngine(inner.to_string());
    let RedactionError::WasmEngine(detail) = &error else {
        panic!("expected WasmEngine variant");
    };
    assert_eq!(detail, inner);
    assert!(!error.to_string().is_empty());
}
