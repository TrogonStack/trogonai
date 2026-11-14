#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use crate::sdk_mcp_js;
use serde_json::json;

#[test]
fn embedded_call_tool_result_schema_builds_the_bundled_zod_checks() {
    assert!(!CALL_TOOL_RESULT.is_null());
    let valid = json!({"content": [{"type": "text", "text": "hi"}]});
    assert!(call_tool_result().parse(Input::Json(&valid)).issues.is_empty());
    let invalid = json!({"content": "x"});
    assert!(!call_tool_result().parse(Input::Json(&invalid)).issues.is_empty());
}

#[test]
fn meta_accepts_a_progress_token_and_rejects_a_boolean_one() {
    let valid = json!({"progressToken": 7});
    assert!(meta().parse(Input::Json(&valid)).issues.is_empty());
    let invalid = json!({"progressToken": true});
    assert!(!meta().parse(Input::Json(&invalid)).issues.is_empty());
}

#[test]
fn js_helpers_follow_javascript_semantics() {
    assert_eq!(sdk_js_trim(" \u{a0}a b\u{feff}"), "a b");
    assert_eq!(sdk_mcp_js::utf16_length("a\u{1f600}"), 3);
    assert!(sdk_mcp_js::forgiving_base64("YQ"));
    assert!(!sdk_mcp_js::forgiving_base64("Y"));
    assert_eq!(sdk_mcp_js::prototype_function_length("toString"), Some(0));
    assert_eq!(sdk_mcp_js::number_text(-0.0), "0");
}

fn sdk_js_trim(text: &str) -> &str {
    sdk_mcp_js::trim(text)
}
