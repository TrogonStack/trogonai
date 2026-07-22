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
fn accepts_core_module_without_imports() {
    let wasm = wat::parse_str(r#"(module (func (export "noop")))"#).unwrap();
    assert!(assert_zero_imports(&wasm).is_ok());
}

#[test]
fn rejects_non_wasm_input() {
    // Not a core module (no `\0asm` magic) and not a valid component, so the wasm-tools
    // failure must surface rather than falling through to the core-module parser.
    assert!(assert_zero_imports(b"definitely not a wasm binary").is_err());
}

#[test]
fn is_core_wasm_module_detects_magic() {
    assert!(is_core_wasm_module(b"\0asm\x01\0\0\0"));
    assert!(!is_core_wasm_module(b"\0asm\x0d\0\x01\0"));
    assert!(!is_core_wasm_module(b"nope"));
}
