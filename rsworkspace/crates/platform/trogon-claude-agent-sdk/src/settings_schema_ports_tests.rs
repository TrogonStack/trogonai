#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::{Value, json};

use super::{aws_pair_issues, aws_pairs_across_issues, convert_request_timeout};
use crate::settings_numbers::js_value;

fn timeout(requested: Value) -> Option<f64> {
    match &convert_request_timeout(&json!({"request_timeout_ms": requested}))["timeout"] {
        Value::Number(number) => js_value(number),
        _ => None,
    }
}

#[test]
fn a_request_timeout_takes_the_smaller_value_as_math_min_does() {
    // `Math.min("-Infinity", 300000)` is `-Infinity`; `Math.min("Infinity", 300000)` is the cap.
    assert_eq!(timeout(json!("-Infinity")), Some(f64::NEG_INFINITY));
    assert_eq!(timeout(json!("Infinity")), Some(300_000.0));
}

#[test]
fn aws_pair_slots_compare_numbers_the_way_a_javascript_map_does() {
    // A `Map` keyed by `0` finds `-0`, a `Set` holding `1` has `1.0`, and `1` is not `2`.
    assert_eq!(
        aws_pair_issues(&json!({"accessKeyIdVar": 0, "secretAccessKeyVar": -0.0})),
        1
    );
    assert_eq!(
        aws_pair_issues(&json!({"accessKeyIdVar": 1, "secretAccessKeyVar": 2})),
        0
    );
    assert_eq!(
        aws_pairs_across_issues(&json!({"awsPairs": [{"accessKeyIdVar": 1}, {"secretAccessKeyVar": 1.0}]})),
        1
    );
}
