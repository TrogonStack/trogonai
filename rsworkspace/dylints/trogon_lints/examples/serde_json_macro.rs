//! UI fixture for the `serde_json_macro` lint. Built as an example so the
//! fixture can depend on the real `serde_json`, whose `json!` macro the lint
//! resolves by owning crate.

// Cargo builds examples with a plain rustc, which does not know the repo's
// Dylint lints; only the driver compiletest runs does.
#![allow(unknown_lints)]

use serde::Serialize;
use serde_json::{Value, json};

#[derive(Serialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    code: i32,
    message: String,
}

fn inline_object(code: i32) -> Value {
    json!({ "error": { "code": code, "message": "boom" } })
}

fn qualified_invocation() -> Value {
    serde_json::json!({ "ok": true })
}

// A `json!` nested inside another macro call is still hand-written at this
// site, so it must fire on its own.
fn nested_in_macro() -> Vec<Value> {
    vec![json!({ "id": 1 })]
}

// Both the inner and the outer invocation are hand-written, so each is its own
// site.
fn nested_in_json() -> Value {
    json!({ "envelope": json!({ "id": 1 }) })
}

// The documented opt-out: an `allow` carrying the technical reason.
#[allow(
    serde_json_macro,
    reason = "the upstream document is passed through verbatim and has no fixed schema"
)]
fn opted_out(document: &str) -> Value {
    json!({ "passthrough": document })
}

fn typed_payload(code: i32) -> Value {
    serde_json::to_value(ErrorBody {
        error: ErrorDetail {
            code,
            message: "boom".to_owned(),
        },
    })
    .unwrap_or(Value::Null)
}

// Test-support modules build fixture payloads, not production ones, so the
// whole family is exempt. (`inline_module_block` is allowed here because a
// fixture is a single file.)
#[allow(inline_module_block)]
mod test_support {
    use serde_json::{Value, json};

    pub fn sample() -> Value {
        json!({ "id": "fixture" })
    }
}

fn main() {
    let _ = inline_object(1);
    let _ = qualified_invocation();
    let _ = nested_in_macro();
    let _ = nested_in_json();
    let _ = opted_out("{}");
    let _ = typed_payload(1);
    let _ = test_support::sample();
}
