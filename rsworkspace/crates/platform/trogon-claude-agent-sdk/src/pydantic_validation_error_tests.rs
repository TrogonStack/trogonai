#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests compare Python reprs and validation texts of JSON values."
    )
)]

use super::*;
use serde_json::json;

fn missing(model: &'static str) -> LineError {
    LineError {
        location: vec![model, "result"],
        kind: "missing",
        message: "Field required",
        input: ErrorInput::Whole,
    }
}

#[test]
fn renders_the_header_block_and_link() {
    let text = render("T", &[missing("M")], &json!({}));
    assert_eq!(
        text,
        "1 validation error for T\nM.result\n  Field required [type=missing, input_value={}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.14/v/missing"
    );
    assert!(render("T", &[missing("M"), missing("N")], &json!({})).starts_with("2 validation errors for T\n"));
}

#[test]
fn cuts_a_long_input_around_an_ellipsis_on_character_boundaries() {
    let short = "x".repeat(50);
    assert_eq!(truncated(short.clone()), short);
    assert_eq!(
        truncated("a".repeat(60)),
        format!("{}...{}", "a".repeat(25), "a".repeat(24))
    );
    let wide = truncated("日".repeat(30));
    assert_eq!(wide, format!("{}...{}", "日".repeat(8), "日".repeat(8)));
}

#[test]
fn shows_the_failing_value_when_the_error_has_its_own_input() {
    let error = LineError {
        input: ErrorInput::Value(json!([])),
        ..missing("M")
    };
    let text = render("T", &[error], &json!({"k": 1}));
    assert!(text.contains("input_value=[], input_type=list"), "{text}");
}
