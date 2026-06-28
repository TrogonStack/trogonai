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
