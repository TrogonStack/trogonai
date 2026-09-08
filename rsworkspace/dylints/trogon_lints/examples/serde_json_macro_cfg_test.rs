// compile-flags: --test
//! UI fixture proving `serde_json_macro` is suppressed under `#[cfg(test)]`.
//! The file name is not a test-file name and `fixture_data` is not part of the
//! test-support module family, so the `#[cfg(test)]` gate is the only thing
//! that can exempt the `json!` below. The `--test` flag is what makes the
//! module reach the lint at all. Absent a `.stderr`, the harness asserts zero
//! diagnostics.

// Cargo builds examples with a plain rustc, which does not know the repo's
// Dylint lints; only the driver compiletest runs does.
#![allow(unknown_lints)]

#[allow(inline_module_block)]
#[cfg(test)]
mod fixture_data {
    use serde_json::{Value, json};

    pub fn sample() -> Value {
        json!({ "id": "fixture" })
    }
}

fn main() {}
