//! UI fixture proving `serde_json_macro` is suppressed in test files. The
//! example is named `*_tests.rs`, so the same `json!` payload that fires in
//! `serde_json_macro.rs` produces no diagnostic here. Absent a `.stderr`, the
//! harness asserts zero diagnostics.

use serde_json::{Value, json};

fn fixture_payload() -> Value {
    json!({ "error": { "code": 42, "message": "boom" } })
}

fn main() {
    let _ = fixture_payload();
}
